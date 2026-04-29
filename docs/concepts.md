# Concepts

`merk` is a thin layer over Git and the filesystem.

Git tracks:

- `dataset.yaml`
- relative symlinks for large files

Git does not track:

- large file bytes
- local cache paths
- `.ds/`
- remote state
- temporary state

## File Model

When you add a large file:

```text
data/train-000.tar.zst
```

`merk` hashes the bytes and stores them in the cache:

```text
<cache>/files/sha256/ab/<hash>
```

The original file becomes a Git-tracked symlink:

```text
data/train-000.tar.zst -> ../.ds/worktree/sha256/ab/<hash>
```

The local `.ds/worktree` symlink points to the cached file:

```text
.ds/worktree/sha256/ab/<hash> -> <cache>/files/sha256/ab/<hash>
```

Opening `data/train-000.tar.zst` reads the cached bytes through the symlink
chain.

## Source Of Truth

The Git tree is the file list.

There is no manifest file, lock file, database, branch, server protocol, or
hidden metadata format. Directories are normal Git directories. Large files are
normal Git symlinks.

## Design Rules

- Keep state visible.
- Keep local paths out of Git.
- Use relative Git symlinks.
- Use plain files for cached bytes.
- Use well-known file movers for remotes.
- Hash-verify bytes before accepting them.
- Make interrupted operations safe to retry.
