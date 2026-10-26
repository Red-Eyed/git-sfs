//! `import` ingests external files into the cache and creates git-sfs symlinks
//! at destination paths inside the repository.
//!
//! Each source is hashed, cached (`Store::store` for a copy, `Store::adopt`
//! for `--move`), and symlinked before the next source is processed. That keeps
//! at most one file in flight at a time: after any interruption, completed
//! entries have symlinks and the current entry is either still at its source or
//! already reachable through the cache.
//!
//! `Store::adopt` (not a plain copy) is still the `--move` primitive
//! deliberately: for the large datasets this project targets, requiring a
//! full second copy of every file before deleting the original would need
//! roughly double the free disk space to move data that already fits once —
//! exactly the scenario `--move` exists for.
//!
//! Destination-collision validation happens across the whole source batch before
//! any source file is moved or copied. A rejected import is therefore a no-op.
//! Like `add`, this command requires an already-bound cache and keeps progress
//! rendering out of core.

use std::collections::BTreeMap;

use camino::{Utf8Path, Utf8PathBuf};
use thiserror::Error;

use super::repo_relative;
use crate::cancel::Cancel;
use crate::domain::hash::Sha256;
use crate::domain::symlink::{NoRelativePath, cache_link_file, clean_utf8, git_link_target};
use crate::error::Error;
use crate::ports::repo::should_skip;
use crate::ports::{Store, StoreError, hash_file};

/// One file `import` finished ingesting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedFile {
    /// The external path that was hashed and cached (after resolving any
    /// `-L`-followed symlink).
    pub src: Utf8PathBuf,
    /// Repo-relative destination path.
    pub dst: Utf8PathBuf,
    /// The hash it is now tracked under.
    pub hash: Sha256,
}

/// What an [`import`] run produced, whether or not it ultimately succeeded.
#[derive(Debug, Default)]
pub struct ImportOutcome {
    /// Every file successfully ingested, in the order they were processed.
    pub imported: Vec<ImportedFile>,
    /// Source candidates whose own filename is not valid UTF-8 — skipped,
    /// not ingested, but reported rather than silently dropped, just like
    /// [`crate::ports::repo::FoundEntry::Unrepresentable`]).
    pub unrepresentable: Vec<String>,
}

/// Why `import` failed.
#[derive(Debug, Error)]
pub enum ImportError {
    /// `dst` does not resolve to a location inside the repository.
    #[error("destination must be inside repository: {dest}")]
    DestinationOutsideRepo {
        /// The destination argument as given.
        dest: Utf8PathBuf,
    },
    /// `dst` resolves inside `.git-sfs`, which `import` must never write
    /// into directly.
    #[error("destination must not be inside .git-sfs: {dest}")]
    DestinationInsideGitSfs {
        /// The destination argument as given.
        dest: Utf8PathBuf,
    },
    /// Git's own metadata is never an import destination.
    #[error("destination must not be inside .git: {dest}")]
    DestinationInsideGit {
        /// The rejected destination.
        dest: Utf8PathBuf,
    },
    /// A destination directory or one of its ancestors is not a real directory.
    #[error("destination exists and is not a directory: {dest}")]
    DestinationNotADirectory {
        /// The destination argument as given.
        dest: Utf8PathBuf,
    },
    /// Something is already at a destination path this import would write.
    #[error("destination already exists: {dest}")]
    DestinationAlreadyExists {
        /// The path that was already occupied.
        dest: Utf8PathBuf,
    },
    /// Two sources would write the same path or a file and its descendant.
    #[error("multiple sources conflict at destination: {dest}")]
    DestinationCollision {
        /// The conflicting destination.
        dest: Utf8PathBuf,
    },
    /// The caller supplied no source paths.
    #[error("import requires at least one source")]
    NoSources,
    /// A symlink was reached as (or within) the source without `-L`.
    /// Refusing by default is the safe choice, since a symlink in an incoming
    /// tree may point anywhere on the machine.
    #[error("source symlinks are not supported without -L: {path}")]
    SourceSymlinkRequiresFollow {
        /// The symlink that was reached.
        path: Utf8PathBuf,
    },
    /// A symlink found *while walking a source directory* resolved to
    /// something other than a regular file. (A directory symlink is only
    /// permitted as the top-level `src` argument itself, not nested within
    /// one.
    #[error("source symlink must resolve to a regular file: {path}")]
    SourceSymlinkTargetNotRegular {
        /// The symlink whose target was rejected.
        path: Utf8PathBuf,
    },
    /// A source entry is neither a regular file, a directory, nor (with
    /// `-L`) a symlink resolving to one — a device, socket, or similar.
    #[error("source must be a regular file or directory: {path}")]
    UnsupportedSourceType {
        /// The unsupported entry.
        path: Utf8PathBuf,
    },
    /// Resolving the top-level `src` argument's own symlink produced a path
    /// that is not valid UTF-8. Distinct from a nested entry hitting the
    /// same condition mid-walk (reported as `unrepresentable` and skipped
    /// instead): `src` is the one argument defining the whole operation, so
    /// there is no "rest of the import" to continue with if it can't be
    /// resolved.
    #[error("{path}: symlink target is not valid UTF-8")]
    NonUtf8Path {
        /// The path whose resolution produced non-UTF-8 bytes.
        path: Utf8PathBuf,
    },
    /// Walking a source directory failed.
    #[error(transparent)]
    Walk(#[from] walkdir::Error),
    /// `dst` and the repository root disagree on absolute-vs-relative, so no
    /// symlink target could be computed. Not reachable in practice: both
    /// are always absolute by the time they reach here.
    #[error("{path}: {source}")]
    NoRelativePath {
        /// The destination path being computed for.
        path: Utf8PathBuf,
        /// The underlying error.
        #[source]
        source: NoRelativePath,
    },
    /// Hashing or caching `path` failed.
    #[error("{path}: {source}")]
    Store {
        /// The source file being processed.
        path: Utf8PathBuf,
        /// Why caching it failed.
        #[source]
        source: StoreError,
    },
    /// `path` was cached, but its object is not reachable through the
    /// repository's own `.git-sfs/cache` symlink. Mirrors `add`'s identical
    /// sanity check (see `AddError::CacheLinkUnreachable`).
    #[error("{path}: cache object for {hash} is not reachable through .git-sfs/cache")]
    CacheLinkUnreachable {
        /// The source file being processed.
        path: Utf8PathBuf,
        /// The hash it was cached under.
        hash: Sha256,
    },
    /// A filesystem operation failed.
    #[error("{path}: {source}")]
    Io {
        /// The path the failing operation was on.
        path: Utf8PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The caller asked to stop.
    #[error("canceled")]
    Canceled,
}

impl From<ImportError> for Error {
    fn from(err: ImportError) -> Self {
        match &err {
            ImportError::Canceled
            | ImportError::Store {
                source: StoreError::Canceled,
                ..
            } => Error::Canceled,
            ImportError::Store {
                source: StoreError::HashMismatch { .. },
                ..
            } => Error::Integrity(err.to_string()),
            ImportError::NoRelativePath { .. } | ImportError::CacheLinkUnreachable { .. } => {
                Error::Config(err.to_string())
            }
            ImportError::DestinationOutsideRepo { .. }
            | ImportError::DestinationInsideGitSfs { .. }
            | ImportError::DestinationInsideGit { .. }
            | ImportError::DestinationNotADirectory { .. }
            | ImportError::DestinationAlreadyExists { .. }
            | ImportError::DestinationCollision { .. }
            | ImportError::NoSources
            | ImportError::SourceSymlinkRequiresFollow { .. }
            | ImportError::SourceSymlinkTargetNotRegular { .. }
            | ImportError::UnsupportedSourceType { .. }
            | ImportError::NonUtf8Path { .. } => Error::Usage(err.to_string()),
            ImportError::Store { .. } | ImportError::Walk(_) | ImportError::Io { .. } => {
                Error::Unavailable(err.to_string())
            }
        }
    }
}

/// [`import`]'s outcome-so-far, together with why it stopped — returned
/// instead of a bare [`ImportError`] so a caller can still report files
/// already ingested when a later one fails.
#[derive(Debug)]
pub struct ImportFailure {
    /// Every file successfully ingested before `error` stopped the run.
    pub outcome: ImportOutcome,
    /// Why it stopped. Boxed for the same reason as `AddFailure::error`.
    pub error: Box<ImportError>,
}

impl ImportFailure {
    fn new(outcome: ImportOutcome, error: ImportError) -> Self {
        Self {
            outcome,
            error: Box::new(error),
        }
    }
}

/// One file this import will ingest: its resolved source, its destination,
/// and the key used to detect it is the same underlying file as another
/// pair (two different source paths — e.g. two `-L`-followed symlinks —
/// can resolve to one canonical file).
#[derive(Debug)]
pub struct ImportFile {
    /// Resolved external source path.
    pub src: Utf8PathBuf,
    /// Repository-relative destination path.
    pub dst: Utf8PathBuf,
    /// Logical source size at planning time; execution still verifies bytes.
    pub size: u64,
    key: Utf8PathBuf,
}

/// The validated shape of one `import` invocation, computed entirely
/// read-only before anything is touched. Not a
/// `crate::plan` type despite the name: unlike that module's pure functions,
/// this performs real filesystem I/O (`lstat`, `canonicalize`, directory
/// walks) to validate `src`/`dst`, so it lives here in `exec` instead.
#[derive(Debug, Default)]
struct SourcePlan {
    pairs: Vec<ImportFile>,
    /// Source directories to remove if left empty by `--move`, deepest-ish
    /// first with `src` itself
    /// last.
    dirs: Vec<Utf8PathBuf>,
    /// Followed (`-L`) symlinks to remove under `--move` — distinct from
    /// `dirs`/the regular files they resolve to, which `import_one` already
    /// consumes.
    source_links: Vec<Utf8PathBuf>,
    unrepresentable: Vec<String>,
}

/// A read-only validated batch bound to its repository and copy/move options.
/// Execute this exact plan with [`execute_import`], or inspect it for a dry run.
#[derive(Debug)]
pub struct ImportPlan {
    repo: Utf8PathBuf,
    options: ImportOptions,
    sources: SourcePlan,
}

impl ImportPlan {
    /// Source mappings in deterministic destination order.
    pub fn files(&self) -> &[ImportFile] {
        &self.sources.pairs
    }

    /// Source names skipped because they cannot be represented as UTF-8.
    pub fn unrepresentable(&self) -> &[String] {
        &self.sources.unrepresentable
    }

    /// Whether execution will consume the sources rather than copy them.
    pub fn moves_sources(&self) -> bool {
        self.options.move_source
    }

    /// Total logical bytes across destination files, including shared sources.
    pub fn total_bytes(&self) -> u128 {
        self.files().iter().map(|file| u128::from(file.size)).sum()
    }
}

/// The two independent flags `import` takes, grouped because they are read
/// together at nearly every call site below. Keeping the names at the call
/// site is clearer than a bare `(bool, bool)`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ImportOptions {
    /// Delete each source after it is safely cached and symlinked (see the
    /// module doc for why this is still `Store::adopt`, not a plain copy).
    pub move_source: bool,
    /// Follow a symlink reached as (or within) the source instead of
    /// refusing it.
    pub follow_symlinks: bool,
}

/// Ingests `src` (resolved against `cwd`, because it is external to the repo)
/// into the cache, creating a git-sfs symlink at `dst` (resolved against
/// `repo`, like other repository path arguments).
///
/// # Errors
///
/// Returns the files ingested so far bundled with the first [`ImportError`]
/// encountered, so a caller can report partial progress even on failure. A
/// failure during validation (before any file is touched) reports an empty
/// outcome because a rejected import is a no-op.
pub fn import(
    store: &dyn Store,
    repo: &Utf8Path,
    cwd: &Utf8Path,
    src: &Utf8Path,
    dst: &Utf8Path,
    options: ImportOptions,
    cancel: &Cancel,
) -> Result<ImportOutcome, ImportFailure> {
    let plan = plan_import(repo, cwd, &[src.to_owned()], dst, options, cancel)
        .map_err(|error| ImportFailure::new(ImportOutcome::default(), error))?;
    execute_import(store, plan, cancel)
}

/// Ingests a validated plan, reporting completed files if execution fails.
/// Storage verifies source bytes, and symlink creation refuses occupied paths;
/// the plan does not snapshot file contents.
pub fn execute_import(
    store: &dyn Store,
    plan: ImportPlan,
    cancel: &Cancel,
) -> Result<ImportOutcome, ImportFailure> {
    let ImportPlan {
        repo,
        options,
        sources: plan,
    } = plan;

    let mut outcome = ImportOutcome {
        unrepresentable: plan.unrepresentable,
        ..ImportOutcome::default()
    };
    let mut hashes: BTreeMap<Utf8PathBuf, Sha256> = BTreeMap::new();

    for pair in plan.pairs {
        if cancel.is_canceled() {
            return Err(ImportFailure::new(outcome, ImportError::Canceled));
        }

        let hash = match hashes.get(&pair.key) {
            Some(&hash) => hash,
            None => match import_one(store, &repo, &pair.src, options.move_source, cancel) {
                Ok(hash) => {
                    hashes.insert(pair.key.clone(), hash);
                    hash
                }
                Err(error) => return Err(ImportFailure::new(outcome, error)),
            },
        };

        if let Err(error) = publish(&repo, &repo.join(&pair.dst), hash) {
            return Err(ImportFailure::new(outcome, error));
        }
        outcome.imported.push(ImportedFile {
            src: pair.src,
            dst: pair.dst,
            hash,
        });
    }

    if options.move_source {
        for link in &plan.source_links {
            #[allow(
                clippy::let_underscore_must_use,
                reason = "best-effort cleanup of a followed symlink; the regular file it resolved to has already been safely cached and symlinked by this point"
            )]
            let _ = std::fs::remove_file(link.as_std_path());
        }
        for dir in &plan.dirs {
            #[allow(
                clippy::let_underscore_must_use,
                reason = "best-effort empty-directory cleanup; a directory that is not actually empty is left alone, not a failure"
            )]
            let _ = std::fs::remove_dir(dir.as_std_path());
        }
    }

    Ok(outcome)
}

/// Hashes and caches `src` (`store()` to copy, `adopt()` to move — see the
/// module doc for why `--move` still uses `adopt`'s rename-with-fallback
/// rather than always copying), then confirms the result is reachable
/// through `.git-sfs/cache`, exactly like `add`'s own sanity check.
fn import_one(
    store: &dyn Store,
    repo: &Utf8Path,
    src: &Utf8Path,
    move_source: bool,
    cancel: &Cancel,
) -> Result<Sha256, ImportError> {
    let hash = hash_file(src, cancel).map_err(|source| ImportError::Io {
        path: src.to_owned(),
        source,
    })?;

    let cached = if move_source {
        store.adopt(src, hash, cancel)
    } else {
        store.store(src, hash, cancel)
    };
    cached.map_err(|source| ImportError::Store {
        path: src.to_owned(),
        source,
    })?;

    if !cache_link_file(repo, hash).is_file() {
        return Err(ImportError::CacheLinkUnreachable {
            path: src.to_owned(),
            hash,
        });
    }
    Ok(hash)
}

/// Creates the destination symlink for `hash` at `dst`, making the object
/// just cached reachable from the working tree.
fn publish(repo: &Utf8Path, dst: &Utf8Path, hash: Sha256) -> Result<(), ImportError> {
    let target =
        git_link_target(repo, dst, hash).map_err(|source| ImportError::NoRelativePath {
            path: dst.to_owned(),
            source,
        })?;
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent.as_std_path()).map_err(|source| ImportError::Io {
            path: parent.to_owned(),
            source,
        })?;
    }
    std::os::unix::fs::symlink(target.as_std_path(), dst.as_std_path()).map_err(|source| {
        ImportError::Io {
            path: dst.to_owned(),
            source,
        }
    })
}

/// Validates every source and destination without hashing or modifying files.
/// Multiple sources are placed under `dst` by basename. A single directory
/// source retains its contents-only merge behavior; a single file uses its
/// basename when `dst` is an existing directory or ends with `/`.
pub fn plan_import(
    repo: &Utf8Path,
    cwd: &Utf8Path,
    sources: &[Utf8PathBuf],
    dst: &Utf8Path,
    options: ImportOptions,
    cancel: &Cancel,
) -> Result<ImportPlan, ImportError> {
    if sources.is_empty() {
        return Err(ImportError::NoSources);
    }
    check_canceled(cancel)?;
    let dst_abs = absolute(repo, dst);
    validate_destination_parents(repo, &dst_abs)?;
    let directory_destination = sources.len() > 1
        || dst.as_str().ends_with('/')
        || path_metadata(&dst_abs)?.is_some_and(|metadata| metadata.is_dir());
    if directory_destination {
        require_directory_if_present(&dst_abs)?;
    }

    let mut batch = SourcePlan::default();
    for src in sources {
        check_canceled(cancel)?;
        let source_abs = absolute(cwd, src);
        let destination = if sources.len() > 1 {
            let name =
                source_abs
                    .file_name()
                    .ok_or_else(|| ImportError::UnsupportedSourceType {
                        path: source_abs.clone(),
                    })?;
            dst_abs.join(name)
        } else {
            dst_abs.clone()
        };
        let mut source = plan_source(
            cwd,
            src,
            &destination,
            directory_destination && sources.len() == 1,
            options.follow_symlinks,
            cancel,
        )?;
        batch.pairs.append(&mut source.pairs);
        batch.dirs.append(&mut source.dirs);
        batch.source_links.append(&mut source.source_links);
        batch.unrepresentable.append(&mut source.unrepresentable);
    }
    batch.pairs.sort_by(|a, b| a.dst.cmp(&b.dst));
    let mut destinations = std::collections::BTreeSet::new();
    for pair in &mut batch.pairs {
        check_canceled(cancel)?;
        validate_destination_parents(repo, &pair.dst)?;
        if !destinations.insert(pair.dst.clone()) {
            return Err(ImportError::DestinationCollision {
                dest: pair.dst.clone(),
            });
        }
        pair.dst = repo_relative(repo, &pair.dst);
    }
    for path in &destinations {
        check_canceled(cancel)?;
        if path
            .ancestors()
            .skip(1)
            .any(|parent| destinations.contains(parent))
        {
            return Err(ImportError::DestinationCollision { dest: path.clone() });
        }
    }
    batch
        .dirs
        .sort_by_key(|dir| std::cmp::Reverse(dir.as_str().len()));
    batch.dirs.dedup();
    Ok(ImportPlan {
        repo: repo.to_owned(),
        options,
        sources: batch,
    })
}

/// Validates one source and walks directory contents without modifying files.
fn plan_source(
    cwd: &Utf8Path,
    src_arg: &Utf8Path,
    dst_abs: &Utf8Path,
    append_file_basename: bool,
    follow_symlinks: bool,
    cancel: &Cancel,
) -> Result<SourcePlan, ImportError> {
    check_canceled(cancel)?;
    let src_abs = absolute(cwd, src_arg);

    let mut source_links = Vec::new();
    let metadata =
        std::fs::symlink_metadata(src_abs.as_std_path()).map_err(|source| ImportError::Io {
            path: src_abs.clone(),
            source,
        })?;

    let (effective_src, effective_metadata) = if metadata.is_symlink() {
        if !follow_symlinks {
            return Err(ImportError::SourceSymlinkRequiresFollow {
                path: src_arg.to_owned(),
            });
        }
        let resolved = resolve_symlink(&src_abs)
            .map_err(|source| ImportError::Io {
                path: src_abs.clone(),
                source,
            })?
            .ok_or_else(|| ImportError::NonUtf8Path {
                path: src_arg.to_owned(),
            })?;
        let resolved_metadata =
            std::fs::symlink_metadata(resolved.as_std_path()).map_err(|source| {
                ImportError::Io {
                    path: resolved.clone(),
                    source,
                }
            })?;
        source_links.push(src_abs.clone());
        (resolved, resolved_metadata)
    } else {
        (src_abs.clone(), metadata)
    };

    if effective_metadata.is_file() {
        let destination = if append_file_basename {
            let name = src_abs
                .file_name()
                .ok_or_else(|| ImportError::UnsupportedSourceType {
                    path: src_abs.clone(),
                })?;
            dst_abs.join(name)
        } else {
            dst_abs.to_owned()
        };
        if path_metadata(&destination)?.is_some() {
            return Err(ImportError::DestinationAlreadyExists { dest: destination });
        }
        let key = canonical_key(&effective_src);
        return Ok(SourcePlan {
            pairs: vec![ImportFile {
                src: effective_src,
                dst: destination,
                size: effective_metadata.len(),
                key,
            }],
            dirs: Vec::new(),
            source_links,
            unrepresentable: Vec::new(),
        });
    }

    if !effective_metadata.is_dir() {
        return Err(ImportError::UnsupportedSourceType {
            path: effective_src,
        });
    }
    require_directory_if_present(dst_abs)?;

    walk_source_dir(
        &effective_src,
        dst_abs,
        follow_symlinks,
        source_links,
        cancel,
    )
}

/// Walks a directory source, collecting one [`ImportFile`] per regular file
/// (or `-L`-followed symlink resolving to one). Rejects up front, touching
/// nothing, if any computed destination already exists.
fn walk_source_dir(
    src_abs: &Utf8Path,
    dst_abs: &Utf8Path,
    follow_symlinks: bool,
    mut source_links: Vec<Utf8PathBuf>,
    cancel: &Cancel,
) -> Result<SourcePlan, ImportError> {
    let mut pairs = Vec::new();
    let mut dirs = Vec::new();
    let mut unrepresentable = Vec::new();

    for entry in walkdir::WalkDir::new(src_abs.as_std_path()) {
        check_canceled(cancel)?;
        let entry = entry?;
        if entry.path() == src_abs.as_std_path() {
            continue;
        }
        let Some(abs) = Utf8Path::from_path(entry.path()) else {
            unrepresentable.push(entry.path().to_string_lossy().into_owned());
            continue;
        };
        let abs = abs.to_owned();

        if entry.file_type().is_dir() {
            dirs.push(abs);
            continue;
        }

        let file_src = if entry.file_type().is_symlink() {
            if !follow_symlinks {
                return Err(ImportError::SourceSymlinkRequiresFollow { path: abs });
            }
            let resolved = match resolve_symlink(&abs) {
                Ok(Some(resolved)) => resolved,
                Ok(None) => {
                    unrepresentable.push(abs.to_string());
                    continue;
                }
                Err(source) => return Err(ImportError::Io { path: abs, source }),
            };
            let resolved_metadata =
                std::fs::symlink_metadata(resolved.as_std_path()).map_err(|source| {
                    ImportError::Io {
                        path: resolved.clone(),
                        source,
                    }
                })?;
            if !resolved_metadata.is_file() {
                return Err(ImportError::SourceSymlinkTargetNotRegular { path: abs });
            }
            source_links.push(abs.clone());
            resolved
        } else if entry.file_type().is_file() {
            abs.clone()
        } else {
            return Err(ImportError::UnsupportedSourceType { path: abs });
        };

        let rel = abs.strip_prefix(src_abs).unwrap_or(&abs);
        let out = dst_abs.join(rel);
        if path_metadata(&out)?.is_some() {
            return Err(ImportError::DestinationAlreadyExists { dest: out });
        }

        let key = canonical_key(&file_src);
        let size = std::fs::metadata(&file_src)
            .map_err(|source| ImportError::Io {
                path: file_src.clone(),
                source,
            })?
            .len();
        pairs.push(ImportFile {
            src: file_src,
            dst: out,
            size,
            key,
        });
    }

    dirs.push(src_abs.to_owned());

    Ok(SourcePlan {
        pairs,
        dirs,
        source_links,
        unrepresentable,
    })
}

/// Missing destinations are allowed; other metadata errors must remain visible.
fn path_metadata(path: &Utf8Path) -> Result<Option<std::fs::Metadata>, ImportError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ImportError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

/// Existing destination ancestors must be real directories, not symlinks.
fn require_directory_if_present(path: &Utf8Path) -> Result<(), ImportError> {
    if path_metadata(path)?.is_some_and(|metadata| !metadata.is_dir()) {
        return Err(ImportError::DestinationNotADirectory {
            dest: path.to_owned(),
        });
    }
    Ok(())
}

/// Rejects repository escapes and blocked parents before any source is consumed.
fn validate_destination_parents(repo: &Utf8Path, dst: &Utf8Path) -> Result<(), ImportError> {
    let rel = dst
        .strip_prefix(repo)
        .map_err(|_| ImportError::DestinationOutsideRepo {
            dest: dst.to_owned(),
        })?;
    if rel
        .components()
        .any(|component| component.as_str() == ".git")
    {
        return Err(ImportError::DestinationInsideGit {
            dest: dst.to_owned(),
        });
    }
    if should_skip(rel) {
        return Err(ImportError::DestinationInsideGitSfs {
            dest: dst.to_owned(),
        });
    }
    for parent in dst
        .ancestors()
        .skip(1)
        .take_while(|parent| parent.starts_with(repo))
    {
        require_directory_if_present(parent)?;
    }
    Ok(())
}

/// Planning can walk large trees, so it observes the same cancellation as ingest.
fn check_canceled(cancel: &Cancel) -> Result<(), ImportError> {
    if cancel.is_canceled() {
        Err(ImportError::Canceled)
    } else {
        Ok(())
    }
}

/// `path`, made absolute against `cwd` if it is not already.
fn absolute(cwd: &Utf8Path, path: &Utf8Path) -> Utf8PathBuf {
    if path.is_absolute() {
        clean_utf8(path)
    } else {
        clean_utf8(&cwd.join(path))
    }
}

/// Resolves `path`'s symlink target, returning `None` rather than erroring
/// if the resolved path is not valid UTF-8 -- callers decide whether that is
/// fatal (the top-level `src` argument) or a skip-and-continue candidate (an
/// entry found mid-walk), matching this crate's established non-UTF-8
/// convention (`ports::repo::ScannedEntry::Unrepresentable`).
fn resolve_symlink(path: &Utf8Path) -> std::io::Result<Option<Utf8PathBuf>> {
    let resolved = std::fs::canonicalize(path.as_std_path())?;
    Ok(Utf8PathBuf::from_path_buf(resolved).ok())
}

/// The key used to detect that two source paths are the same underlying
/// file. Best-effort: falls back to `path` itself if it cannot be
/// canonicalized, since this is a deduplication aid, not a correctness-critical
/// value.
fn canonical_key(path: &Utf8Path) -> Utf8PathBuf {
    std::fs::canonicalize(path.as_std_path())
        .ok()
        .and_then(|resolved| Utf8PathBuf::from_path_buf(resolved).ok())
        .unwrap_or_else(|| path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::FsStore;

    /// A repository with a bound cache, plus a separate directory standing
    /// in for "somewhere external" (an incoming drive, a scratch mount) --
    /// `import`'s source is never inside the repo.
    struct Fixture {
        _repo_dir: tempfile::TempDir,
        _external_dir: tempfile::TempDir,
        repo: Utf8PathBuf,
        cache: Utf8PathBuf,
        external: Utf8PathBuf,
    }

    fn fixture() -> Fixture {
        let repo_dir = tempfile::tempdir().unwrap();
        let repo = Utf8PathBuf::from_path_buf(repo_dir.path().to_owned()).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let cache = repo.join(".git-sfs/cache-real");
        std::fs::create_dir_all(&cache).unwrap();
        std::os::unix::fs::symlink(
            cache.as_std_path(),
            repo.join(".git-sfs/cache").as_std_path(),
        )
        .unwrap();

        let external_dir = tempfile::tempdir().unwrap();
        let external = Utf8PathBuf::from_path_buf(external_dir.path().to_owned()).unwrap();

        Fixture {
            _repo_dir: repo_dir,
            _external_dir: external_dir,
            repo,
            cache,
            external,
        }
    }

    fn opts(move_source: bool, follow_symlinks: bool) -> ImportOptions {
        ImportOptions {
            move_source,
            follow_symlinks,
        }
    }

    #[test]
    fn batch_preview_reports_mapping_and_sizes_without_changing_any_files() {
        let f = fixture();
        let a = f.external.join("a.zip");
        let b = f.external.join("b.zip");
        std::fs::write(&a, b"first").unwrap();
        std::fs::write(&b, b"second").unwrap();
        let permissions = std::fs::metadata(&a).unwrap().permissions();
        let plan = plan_import(
            &f.repo,
            &f.external,
            &[b.clone(), a.clone()],
            Utf8Path::new("new/"),
            opts(true, false),
            &Cancel::new(),
        )
        .unwrap();

        let mappings: Vec<_> = plan
            .files()
            .iter()
            .map(|file| (file.src.clone(), file.dst.as_str(), file.size))
            .collect();
        assert_eq!(
            mappings,
            [(a.clone(), "new/a.zip", 5), (b.clone(), "new/b.zip", 6)]
        );
        assert_eq!(plan.total_bytes(), 11);
        assert!(plan.moves_sources());
        assert!(!f.repo.join("new").exists());
        assert!(std::fs::read_dir(&f.cache).unwrap().next().is_none());
        assert_eq!(std::fs::read(&a).unwrap(), b"first");
        assert_eq!(std::fs::read(&b).unwrap(), b"second");
        assert_eq!(std::fs::metadata(&a).unwrap().permissions(), permissions);
    }

    #[test]
    fn executes_the_previewed_batch_and_publishes_relative_cache_links() {
        let f = fixture();
        let sources: Vec<_> = ["a.zip", "b.zip"]
            .iter()
            .map(|name| f.external.join(name))
            .collect();
        for src in &sources {
            std::fs::write(src, src.as_str()).unwrap();
        }
        let plan = plan_import(
            &f.repo,
            &f.external,
            &sources,
            Utf8Path::new("new"),
            opts(false, false),
            &Cancel::new(),
        )
        .unwrap();
        let destinations: Vec<_> = plan.files().iter().map(|file| file.dst.clone()).collect();
        let outcome = execute_import(&FsStore::new(f.cache.clone()), plan, &Cancel::new()).unwrap();
        assert_eq!(
            outcome
                .imported
                .iter()
                .map(|file| file.dst.clone())
                .collect::<Vec<_>>(),
            destinations
        );
        for file in outcome.imported {
            assert_eq!(
                std::fs::read(f.repo.join(&file.dst)).unwrap(),
                std::fs::read(&file.src).unwrap()
            );
            let hash = file.hash.to_hex();
            assert_eq!(
                std::fs::read_link(f.repo.join(&file.dst)).unwrap(),
                std::path::PathBuf::from(format!(
                    "../.git-sfs/cache/files/sha256/{}/{hash}",
                    file.hash.prefix()
                ))
            );
        }
    }

    #[test]
    fn single_file_accepts_existing_and_new_directory_destinations() {
        for existing in [false, true] {
            let f = fixture();
            let source = f.external.join("file.txt");
            std::fs::write(&source, b"content").unwrap();
            let destination = if existing {
                std::fs::create_dir(f.repo.join("data")).unwrap();
                Utf8Path::new("data")
            } else {
                Utf8Path::new("data/")
            };
            let outcome = import(
                &FsStore::new(f.cache.clone()),
                &f.repo,
                &f.external,
                &source,
                destination,
                opts(false, false),
                &Cancel::new(),
            )
            .unwrap();
            assert_eq!(outcome.imported[0].dst, "data/file.txt");
            assert_eq!(
                std::fs::read(f.repo.join("data/file.txt")).unwrap(),
                b"content"
            );
        }
    }

    #[test]
    fn duplicate_batch_basenames_are_rejected_before_move() {
        let f = fixture();
        let sources: Vec<_> = ["one/file.zip", "two/file.zip"]
            .iter()
            .map(|name| f.external.join(name))
            .collect();
        for src in &sources {
            std::fs::create_dir_all(src.parent().unwrap()).unwrap();
            std::fs::write(src, b"untouched").unwrap();
        }
        let error = plan_import(
            &f.repo,
            &f.external,
            &sources,
            Utf8Path::new("new/"),
            opts(true, false),
            &Cancel::new(),
        )
        .unwrap_err();
        assert!(matches!(error, ImportError::DestinationCollision { .. }));
        for src in &sources {
            assert_eq!(std::fs::read(src).unwrap(), b"untouched");
        }
        assert!(!f.repo.join("new").exists());
        assert!(std::fs::read_dir(&f.cache).unwrap().next().is_none());
    }

    #[test]
    fn a_late_existing_destination_rejects_the_whole_move_batch() {
        let f = fixture();
        let sources: Vec<_> = ["a.zip", "z.zip"]
            .iter()
            .map(|name| f.external.join(name))
            .collect();
        for src in &sources {
            std::fs::write(src, b"source").unwrap();
        }
        std::fs::create_dir(f.repo.join("data")).unwrap();
        std::fs::write(f.repo.join("data/z.zip"), b"existing").unwrap();
        let error = plan_import(
            &f.repo,
            &f.external,
            &sources,
            Utf8Path::new("data"),
            opts(true, false),
            &Cancel::new(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ImportError::DestinationAlreadyExists { .. }
        ));
        for src in &sources {
            assert_eq!(std::fs::read(src).unwrap(), b"source");
        }
        assert!(!f.repo.join("data/a.zip").exists());
        assert_eq!(
            std::fs::read(f.repo.join("data/z.zip")).unwrap(),
            b"existing"
        );
        assert!(std::fs::read_dir(&f.cache).unwrap().next().is_none());
    }

    #[test]
    fn rejects_a_planned_file_that_would_be_another_destinations_parent() {
        let f = fixture();
        std::fs::create_dir_all(f.external.join("directory/tree")).unwrap();
        std::fs::create_dir(f.external.join("file")).unwrap();
        let file = f.external.join("file/tree");
        let directory = f.external.join("directory/tree");
        std::fs::write(&file, b"file").unwrap();
        std::fs::write(directory.join("nested"), b"nested").unwrap();
        let error = plan_import(
            &f.repo,
            &f.external,
            &[file.clone(), directory.clone()],
            Utf8Path::new("data/"),
            opts(true, false),
            &Cancel::new(),
        )
        .unwrap_err();
        assert!(matches!(error, ImportError::DestinationCollision { .. }));
        assert!(file.exists());
        assert!(directory.join("nested").exists());
        assert!(!f.repo.join("data").exists());
    }

    #[test]
    fn refuses_destination_parent_symlinks_without_touching_the_source() {
        let f = fixture();
        let source = f.external.join("source.txt");
        std::fs::write(&source, b"content").unwrap();
        std::os::unix::fs::symlink(&f.external, f.repo.join("escape")).unwrap();
        let error = plan_import(
            &f.repo,
            &f.external,
            std::slice::from_ref(&source),
            Utf8Path::new("escape/renamed.txt"),
            opts(true, false),
            &Cancel::new(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ImportError::DestinationNotADirectory { .. }
        ));
        assert!(source.exists());
        assert!(!f.external.join("renamed.txt").exists());
    }

    #[test]
    fn cancellation_stops_both_planning_and_execution_before_move() {
        let f = fixture();
        let source = f.external.join("source.txt");
        std::fs::write(&source, b"content").unwrap();
        let cancel = Cancel::new();
        let plan = plan_import(
            &f.repo,
            &f.external,
            std::slice::from_ref(&source),
            Utf8Path::new("data/"),
            opts(true, false),
            &cancel,
        )
        .unwrap();
        cancel.cancel();
        let error = plan_import(
            &f.repo,
            &f.external,
            std::slice::from_ref(&source),
            Utf8Path::new("data/"),
            opts(true, false),
            &cancel,
        )
        .unwrap_err();
        assert!(matches!(error, ImportError::Canceled));
        let failure = execute_import(&FsStore::new(f.cache.clone()), plan, &cancel).unwrap_err();
        assert!(matches!(*failure.error, ImportError::Canceled));
        assert!(source.exists());
        assert!(!f.repo.join("data").exists());
        assert!(std::fs::read_dir(&f.cache).unwrap().next().is_none());
    }

    #[test]
    fn multiple_directories_preserve_their_names_and_nested_layout() {
        let f = fixture();
        let sources: Vec<_> = ["one", "two"]
            .iter()
            .map(|name| f.external.join(name))
            .collect();
        for source in &sources {
            std::fs::create_dir_all(source.join("nested")).unwrap();
            std::fs::write(source.join("nested/part.zip"), b"content").unwrap();
        }
        let plan = plan_import(
            &f.repo,
            &f.external,
            &sources,
            Utf8Path::new("data/"),
            opts(true, false),
            &Cancel::new(),
        )
        .unwrap();
        let outcome = execute_import(&FsStore::new(f.cache.clone()), plan, &Cancel::new()).unwrap();
        assert_eq!(
            outcome
                .imported
                .iter()
                .map(|file| file.dst.as_str())
                .collect::<Vec<_>>(),
            ["data/one/nested/part.zip", "data/two/nested/part.zip"]
        );
        for file in &outcome.imported {
            assert_eq!(std::fs::read(f.repo.join(&file.dst)).unwrap(), b"content");
        }
        for source in &sources {
            assert!(!source.exists());
        }
    }

    #[test]
    fn batch_move_deduplicates_followed_sources_before_consuming_them() {
        let f = fixture();
        let target = f.external.join("target.zip");
        std::fs::write(&target, b"shared").unwrap();
        let sources: Vec<_> = ["first.zip", "second.zip"]
            .iter()
            .map(|name| f.external.join(name))
            .collect();
        for source in &sources {
            std::os::unix::fs::symlink(&target, source).unwrap();
        }
        let plan = plan_import(
            &f.repo,
            &f.external,
            &sources,
            Utf8Path::new("data/"),
            opts(true, true),
            &Cancel::new(),
        )
        .unwrap();
        let outcome = execute_import(&FsStore::new(f.cache.clone()), plan, &Cancel::new()).unwrap();
        assert_eq!(outcome.imported.len(), 2);
        for file in outcome.imported {
            assert_eq!(std::fs::read(f.repo.join(&file.dst)).unwrap(), b"shared");
        }
        assert!(!target.exists());
        for source in &sources {
            assert!(std::fs::symlink_metadata(source).is_err());
        }
    }

    #[test]
    fn rejects_git_metadata_destinations_before_ingestion() {
        let f = fixture();
        let source = f.external.join("source.txt");
        std::fs::write(&source, b"content").unwrap();
        let error = plan_import(
            &f.repo,
            &f.external,
            std::slice::from_ref(&source),
            Utf8Path::new(".git/hooks/new"),
            opts(true, false),
            &Cancel::new(),
        )
        .unwrap_err();
        assert!(matches!(error, ImportError::DestinationInsideGit { .. }));
        assert!(source.exists());
        assert!(!f.repo.join(".git/hooks").exists());
    }

    #[test]
    fn imports_a_single_file_and_leaves_the_source_intact_by_default() {
        let f = fixture();
        let src = f.external.join("photo.bin");
        std::fs::write(&src, b"dataset bytes").unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        let outcome = import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("photo.bin"),
            Utf8Path::new("dest.bin"),
            opts(false, false),
            &cancel,
        )
        .unwrap();

        assert_eq!(outcome.imported.len(), 1);
        assert_eq!(outcome.imported[0].dst, "dest.bin");
        assert!(
            src.exists(),
            "copy semantics must leave the source in place"
        );
        let dest = f.repo.join("dest.bin");
        assert!(
            std::fs::symlink_metadata(&dest)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"dataset bytes");
    }

    #[test]
    fn move_flag_removes_the_source_only_after_the_destination_is_published() {
        let f = fixture();
        let src = f.external.join("photo.bin");
        std::fs::write(&src, b"dataset bytes").unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("photo.bin"),
            Utf8Path::new("dest.bin"),
            opts(true, false),
            &cancel,
        )
        .unwrap();

        assert!(!src.exists(), "--move must remove the original source");
        assert_eq!(
            std::fs::read(f.repo.join("dest.bin")).unwrap(),
            b"dataset bytes"
        );
    }

    #[test]
    fn a_directory_source_merges_its_contents_directly_into_an_existing_destination() {
        let f = fixture();
        std::fs::create_dir_all(f.external.join("data/nested")).unwrap();
        std::fs::write(f.external.join("data/one.bin"), b"one").unwrap();
        std::fs::write(f.external.join("data/nested/two.bin"), b"two").unwrap();
        std::fs::create_dir_all(f.repo.join("dest")).unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        let outcome = import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("data"),
            Utf8Path::new("dest"),
            opts(false, false),
            &cancel,
        )
        .unwrap();

        // Merged directly into "dest", not nested under "dest/data" -- this
        // is the one place import's placement rule differs from mv's.
        let mut dsts: Vec<&str> = outcome.imported.iter().map(|f| f.dst.as_str()).collect();
        dsts.sort_unstable();
        assert_eq!(dsts, ["dest/nested/two.bin", "dest/one.bin"]);
        assert_eq!(std::fs::read(f.repo.join("dest/one.bin")).unwrap(), b"one");
        assert_eq!(
            std::fs::read(f.repo.join("dest/nested/two.bin")).unwrap(),
            b"two"
        );
    }

    #[test]
    fn refuses_a_destination_outside_the_repository() {
        let f = fixture();
        std::fs::write(f.external.join("f.bin"), b"x").unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        let failure = import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("f.bin"),
            Utf8Path::new("../outside.bin"),
            opts(false, false),
            &cancel,
        )
        .unwrap_err();

        assert!(matches!(
            *failure.error,
            ImportError::DestinationOutsideRepo { .. }
        ));
        assert!(failure.outcome.imported.is_empty());
    }

    #[test]
    fn refuses_a_destination_inside_git_sfs() {
        let f = fixture();
        std::fs::write(f.external.join("f.bin"), b"x").unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        let failure = import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("f.bin"),
            Utf8Path::new(".git-sfs/sneaky.bin"),
            opts(false, false),
            &cancel,
        )
        .unwrap_err();

        assert!(matches!(
            *failure.error,
            ImportError::DestinationInsideGitSfs { .. }
        ));
    }

    #[test]
    fn refuses_an_existing_destination_and_touches_nothing() {
        let f = fixture();
        let src = f.external.join("f.bin");
        std::fs::write(&src, b"x").unwrap();
        std::fs::write(f.repo.join("dest.bin"), b"already here").unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        let failure = import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("f.bin"),
            Utf8Path::new("dest.bin"),
            opts(true, false),
            &cancel,
        )
        .unwrap_err();

        assert!(matches!(
            *failure.error,
            ImportError::DestinationAlreadyExists { .. }
        ));
        assert!(src.exists(), "a rejected import must be a no-op");
        assert_eq!(
            std::fs::read(f.repo.join("dest.bin")).unwrap(),
            b"already here"
        );
    }

    #[test]
    fn refuses_a_symlink_source_without_follow_and_leaves_both_link_and_target_untouched() {
        let f = fixture();
        let target = f.external.join("real.bin");
        std::fs::write(&target, b"x").unwrap();
        let link = f.external.join("link.bin");
        std::os::unix::fs::symlink(target.as_std_path(), link.as_std_path()).unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        let failure = import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("link.bin"),
            Utf8Path::new("dest.bin"),
            opts(false, false),
            &cancel,
        )
        .unwrap_err();

        assert!(matches!(
            *failure.error,
            ImportError::SourceSymlinkRequiresFollow { .. }
        ));
        assert!(
            std::fs::symlink_metadata(&link).is_ok(),
            "link must survive"
        );
        assert!(target.exists(), "target must survive");
    }

    #[test]
    fn follows_a_top_level_symlink_source_with_follow_symlinks() {
        let f = fixture();
        let target = f.external.join("real.bin");
        std::fs::write(&target, b"resolved content").unwrap();
        let link = f.external.join("link.bin");
        std::os::unix::fs::symlink(target.as_std_path(), link.as_std_path()).unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("link.bin"),
            Utf8Path::new("dest.bin"),
            opts(false, true),
            &cancel,
        )
        .unwrap();

        assert_eq!(
            std::fs::read(f.repo.join("dest.bin")).unwrap(),
            b"resolved content"
        );
    }

    #[test]
    fn deduplicates_two_symlinks_that_resolve_to_the_same_underlying_file() {
        let f = fixture();
        std::fs::create_dir_all(f.external.join("data")).unwrap();
        let target = f.external.join("data/real.bin");
        std::fs::write(&target, b"shared content").unwrap();
        std::os::unix::fs::symlink(
            target.as_std_path(),
            f.external.join("data/a.bin").as_std_path(),
        )
        .unwrap();
        std::os::unix::fs::symlink(
            target.as_std_path(),
            f.external.join("data/b.bin").as_std_path(),
        )
        .unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        let outcome = import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("data"),
            Utf8Path::new("dest"),
            opts(false, true),
            &cancel,
        )
        .unwrap();

        let hashes: std::collections::BTreeSet<_> =
            outcome.imported.iter().map(|f| f.hash).collect();
        // "real.bin" itself, plus the two symlinks resolving to it -- three
        // destinations, but the same underlying file was only hashed and
        // cached once (asserted indirectly: both symlink-derived pairs share
        // its hash).
        assert_eq!(outcome.imported.len(), 3);
        assert_eq!(hashes.len(), 1);
    }

    #[test]
    fn a_missing_source_reports_an_io_error() {
        let f = fixture();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        let failure = import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("does-not-exist.bin"),
            Utf8Path::new("dest.bin"),
            opts(false, false),
            &cancel,
        )
        .unwrap_err();

        assert!(matches!(*failure.error, ImportError::Io { .. }));
    }

    #[test]
    fn a_nested_symlink_must_resolve_to_a_regular_file_not_a_directory() {
        let f = fixture();
        std::fs::create_dir_all(f.external.join("data/real_dir")).unwrap();
        std::os::unix::fs::symlink(
            f.external.join("data/real_dir").as_std_path(),
            f.external.join("data/link_dir").as_std_path(),
        )
        .unwrap();
        let store = FsStore::new(f.cache.clone());
        let cancel = Cancel::new();

        let failure = import(
            &store,
            &f.repo,
            &f.external,
            Utf8Path::new("data"),
            Utf8Path::new("dest"),
            opts(false, true),
            &cancel,
        )
        .unwrap_err();

        assert!(matches!(
            *failure.error,
            ImportError::SourceSymlinkTargetNotRegular { .. }
        ));
    }
}
