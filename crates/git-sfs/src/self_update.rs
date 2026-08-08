//! `git-sfs self update`.
//!
//! This path downloads executable code and replaces binaries on disk, so it is
//! intentionally self-contained rather than delegated to a general updater
//! crate. The core rule is the same as object storage: verify bytes first,
//! publish with a temp-file rename last.

use std::fmt;
use std::io::{Cursor, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use flate2::read::GzDecoder;
use git_sfs_core::{Error, Result};
use sha2::{Digest as _, Sha256};
use ureq::ResponseExt as _;

use crate::version;

const DEFAULT_REPO: &str = "Red-Eyed/git-sfs";
const DEFAULT_RCLONE_BASE_URL: &str = "https://downloads.rclone.org";

/// Run `git-sfs self update`.
pub fn run(quiet: bool) -> Result<()> {
    let env = SelfUpdateEnv::from_process_env();
    let fetcher = HttpFetcher::new(&env)?;
    let exe = current_executable()?;
    let install_dir = exe
        .parent()
        .ok_or_else(|| Error::Unavailable(format!("could not find parent of {}", exe.display())))?;

    update_git_sfs(&env, &fetcher, &exe, quiet)?;
    update_rclone(&env, &fetcher, &install_dir.join("rclone"), quiet)?;
    Ok(())
}

#[derive(Debug, Clone)]
struct SelfUpdateEnv {
    release_base_url: String,
    release_latest_url: String,
    rclone_base_url: String,
    ca_file: Option<PathBuf>,
    insecure_tls: bool,
}

impl SelfUpdateEnv {
    fn from_process_env() -> Self {
        let repo = non_empty_env("GIT_SFS_REPO").unwrap_or_else(|| DEFAULT_REPO.to_owned());
        Self {
            release_base_url: non_empty_env("GIT_SFS_RELEASE_BASE_URL")
                .unwrap_or_else(|| format!("https://github.com/{repo}/releases/download")),
            release_latest_url: non_empty_env("GIT_SFS_RELEASE_LATEST_URL")
                .unwrap_or_else(|| format!("https://github.com/{repo}/releases/latest")),
            rclone_base_url: non_empty_env("GIT_SFS_RCLONE_BASE_URL")
                .unwrap_or_else(|| DEFAULT_RCLONE_BASE_URL.to_owned()),
            ca_file: ca_file_from_env(),
            insecure_tls: std::env::var("GIT_SFS_INSECURE_TLS").is_ok_and(|value| value == "1"),
        }
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn ca_file_from_env() -> Option<PathBuf> {
    ["GIT_SFS_SSL_CERT_FILE", "SSL_CERT_FILE", "CURL_CA_BUNDLE"]
        .into_iter()
        .find_map(non_empty_env)
        .map(PathBuf::from)
}

trait Fetcher {
    fn get_bytes(&self, url: &str) -> std::result::Result<Vec<u8>, UpdateError>;
    fn final_url(&self, url: &str) -> std::result::Result<String, UpdateError>;
}

struct HttpFetcher {
    agent: ureq::Agent,
}

impl HttpFetcher {
    fn new(env: &SelfUpdateEnv) -> Result<Self> {
        let mut tls = ureq::tls::TlsConfig::builder();
        if env.insecure_tls {
            eprintln!("warning: GIT_SFS_INSECURE_TLS=1 disables TLS certificate verification");
            tls = tls.disable_verification(true);
        }
        if let Some(ca_file) = &env.ca_file {
            eprintln!("using TLS CA bundle from {}", ca_file.display());
            let cert = std::fs::read(ca_file)
                .map_err(|err| Error::Unavailable(format!("reading CA bundle: {err}")))
                .and_then(|pem| {
                    ureq::tls::Certificate::from_pem(&pem).map_err(|err| {
                        Error::Unavailable(format!(
                            "parsing CA bundle {}: {err}",
                            ca_file.display()
                        ))
                    })
                })?;
            tls = tls.root_certs(ureq::tls::RootCerts::from(vec![cert]));
        }

        let config = ureq::Agent::config_builder()
            .tls_config(tls.build())
            .build();
        Ok(Self {
            agent: ureq::Agent::new_with_config(config),
        })
    }
}

impl Fetcher for HttpFetcher {
    fn get_bytes(&self, url: &str) -> std::result::Result<Vec<u8>, UpdateError> {
        if let Some(path) = file_url_path(url) {
            return std::fs::read(path).map_err(UpdateError::from);
        }
        let response = self.agent.get(url).call().map_err(UpdateError::from)?;
        response
            .into_body()
            .read_to_vec()
            .map_err(UpdateError::from)
    }

    fn final_url(&self, url: &str) -> std::result::Result<String, UpdateError> {
        if file_url_path(url).is_some() {
            return Ok(url.to_owned());
        }
        let response = self.agent.get(url).call().map_err(UpdateError::from)?;
        Ok(response.get_uri().to_string())
    }
}

fn file_url_path(url: &str) -> Option<PathBuf> {
    url.strip_prefix("file://").map(PathBuf::from)
}

#[derive(Debug)]
enum UpdateError {
    Io(std::io::Error),
    Network(ureq::Error),
    Archive(String),
    Invalid(String),
}

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Network(err) => write!(f, "{err}"),
            Self::Archive(msg) | Self::Invalid(msg) => f.write_str(msg),
        }
    }
}

impl From<std::io::Error> for UpdateError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<ureq::Error> for UpdateError {
    fn from(err: ureq::Error) -> Self {
        Self::Network(err)
    }
}

fn update_git_sfs(
    env: &SelfUpdateEnv,
    fetcher: &impl Fetcher,
    exe: &Path,
    quiet: bool,
) -> Result<()> {
    note(quiet, "checking git-sfs version");
    let latest = latest_git_sfs_version(fetcher, &env.release_latest_url)?;
    let current = version::VERSION;
    if current == latest {
        println!("git-sfs {current} already up to date");
        return Ok(());
    }

    let asset = format!("git-sfs-{latest}-{}-{}.tar.gz", os_name()?, arch_name()?);
    let sums_url = format!("{}/{latest}/SHA256SUMS", env.release_base_url);
    let asset_url = format!("{}/{latest}/{asset}", env.release_base_url);

    let sums = fetcher
        .get_bytes(&sums_url)
        .map_err(|err| unavailable("git-sfs", &sums_url, err))?;
    let expected =
        parse_sha256_sums(&sums, &asset).map_err(|err| unavailable_msg("git-sfs", err))?;
    note(quiet, &format!("downloading git-sfs {latest}"));
    let archive = fetcher
        .get_bytes(&asset_url)
        .map_err(|err| unavailable("git-sfs", &asset_url, err))?;
    verify_sha256(&archive, &expected).map_err(|err| unavailable_msg("git-sfs", err))?;
    let binary =
        extract_tar_gz(&archive, "git-sfs").map_err(|err| unavailable_msg("git-sfs", err))?;
    atomic_replace(exe, &binary)
        .map_err(|err| unavailable("git-sfs", &exe.display().to_string(), err))?;
    println!("git-sfs {current} -> {latest}");
    Ok(())
}

fn update_rclone(
    env: &SelfUpdateEnv,
    fetcher: &impl Fetcher,
    rclone_path: &Path,
    quiet: bool,
) -> Result<()> {
    note(quiet, "checking rclone version");
    let latest = latest_rclone_version(fetcher, &env.rclone_base_url)?;
    let current = current_rclone_version(rclone_path);
    if current.as_deref() == Some(latest.as_str()) {
        println!("rclone {latest} already up to date");
        return Ok(());
    }

    let asset = format!("rclone-{latest}-{}-{}.zip", rclone_os_name()?, arch_name()?);
    let asset_url = format!("{}/{latest}/{asset}", env.rclone_base_url);
    let hash_url = format!("{asset_url}.sha256");

    let hash_file = fetcher
        .get_bytes(&hash_url)
        .map_err(|err| unavailable("rclone", &hash_url, err))?;
    let expected = first_field(&hash_file)
        .ok_or_else(|| Error::Unavailable(format!("rclone: empty checksum file at {hash_url}")))?;
    note(quiet, &format!("downloading rclone {latest}"));
    let archive = fetcher
        .get_bytes(&asset_url)
        .map_err(|err| unavailable("rclone", &asset_url, err))?;
    verify_sha256(&archive, &expected).map_err(|err| unavailable_msg("rclone", err))?;
    let binary = extract_zip(&archive, "rclone").map_err(|err| unavailable_msg("rclone", err))?;
    atomic_replace(rclone_path, &binary)
        .map_err(|err| unavailable("rclone", &rclone_path.display().to_string(), err))?;

    match current {
        Some(current) => println!("rclone {current} -> {latest}"),
        None => println!("rclone {latest} installed"),
    }
    Ok(())
}

fn unavailable(component: &str, where_: &str, err: UpdateError) -> Error {
    Error::Unavailable(format!("{component}: {where_}: {err}"))
}

fn unavailable_msg(component: &str, err: UpdateError) -> Error {
    Error::Unavailable(format!("{component}: {err}"))
}

fn note(quiet: bool, message: &str) {
    if !quiet {
        eprintln!("{message}...");
    }
}

fn latest_git_sfs_version(fetcher: &impl Fetcher, latest_url: &str) -> Result<String> {
    let final_url = fetcher
        .final_url(latest_url)
        .map_err(|err| unavailable("git-sfs", latest_url, err))?;
    version_from_url(&final_url).ok_or_else(|| {
        Error::Unavailable(format!("git-sfs: could not parse version from {final_url}"))
    })
}

fn latest_rclone_version(fetcher: &impl Fetcher, base_url: &str) -> Result<String> {
    let url = format!("{base_url}/version.txt");
    let bytes = fetcher
        .get_bytes(&url)
        .map_err(|err| unavailable("rclone", &url, err))?;
    std::str::from_utf8(&bytes)
        .ok()
        .and_then(|text| text.split_whitespace().find(|field| field.starts_with('v')))
        .map(str::to_owned)
        .ok_or_else(|| Error::Unavailable(format!("rclone: could not parse version from {url}")))
}

fn version_from_url(url: &str) -> Option<String> {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|version| version.starts_with('v') && version.len() > 1)
        .map(str::to_owned)
}

fn parse_sha256_sums(data: &[u8], filename: &str) -> std::result::Result<String, UpdateError> {
    let text = std::str::from_utf8(data)
        .map_err(|err| UpdateError::Invalid(format!("SHA256SUMS is not UTF-8: {err}")))?;
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(hash) = fields.next() else {
            continue;
        };
        if fields.next() == Some(filename) {
            return Ok(hash.to_owned());
        }
    }
    Err(UpdateError::Invalid(format!(
        "SHA256SUMS has no entry for {filename}"
    )))
}

fn first_field(data: &[u8]) -> Option<String> {
    std::str::from_utf8(data)
        .ok()
        .and_then(|text| text.split_whitespace().next())
        .map(str::to_owned)
}

fn verify_sha256(data: &[u8], expected: &str) -> std::result::Result<(), UpdateError> {
    let got = hex::encode(Sha256::digest(data));
    if got == expected {
        return Ok(());
    }
    Err(UpdateError::Invalid(format!(
        "SHA256 mismatch: expected {expected}, got {got}"
    )))
}

fn extract_tar_gz(data: &[u8], binary_name: &str) -> std::result::Result<Vec<u8>, UpdateError> {
    let gz = GzDecoder::new(Cursor::new(data));
    let mut archive = tar::Archive::new(gz);
    for entry in archive
        .entries()
        .map_err(|err| archive_err("reading archive", err))?
    {
        let mut entry = entry.map_err(|err| archive_err("reading archive entry", err))?;
        let path = entry
            .path()
            .map_err(|err| archive_err("reading archive path", err))?;
        if path.file_name().and_then(|name| name.to_str()) != Some(binary_name) {
            continue;
        }
        let mut out = Vec::new();
        std::io::copy(&mut entry, &mut out)
            .map_err(|err| archive_err("extracting archive entry", err))?;
        return Ok(out);
    }
    Err(UpdateError::Archive(format!(
        "{binary_name} not found in archive"
    )))
}

fn extract_zip(data: &[u8], binary_name: &str) -> std::result::Result<Vec<u8>, UpdateError> {
    let reader = Cursor::new(data);
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|err| UpdateError::Archive(err.to_string()))?;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|err| UpdateError::Archive(err.to_string()))?;
        let Some(name) = Path::new(file.name())
            .file_name()
            .and_then(|name| name.to_str())
        else {
            continue;
        };
        if name != binary_name || file.is_dir() {
            continue;
        }
        let mut out = Vec::new();
        std::io::copy(&mut file, &mut out)
            .map_err(|err| archive_err("extracting zip entry", err))?;
        return Ok(out);
    }
    Err(UpdateError::Archive(format!(
        "{binary_name} not found in archive"
    )))
}

fn archive_err(action: &str, err: std::io::Error) -> UpdateError {
    UpdateError::Archive(format!("{action}: {err}"))
}

fn atomic_replace(dest: &Path, data: &[u8]) -> std::result::Result<(), UpdateError> {
    let parent = dest.parent().ok_or_else(|| {
        UpdateError::Invalid(format!("could not find parent of {}", dest.display()))
    })?;
    std::fs::create_dir_all(parent)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".git-sfs-update-")
        .tempfile_in(parent)?;
    tmp.write_all(data)?;
    set_executable(tmp.path())?;
    tmp.as_file().sync_all()?;
    let persisted = tmp
        .persist(dest)
        .map_err(|err| UpdateError::Io(err.error))?;
    drop(persisted);
    sync_parent(parent)?;
    Ok(())
}

fn sync_parent(parent: &Path) -> std::io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(unix)]
fn set_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn current_executable() -> Result<PathBuf> {
    let path = std::env::current_exe()
        .map_err(|err| Error::Unavailable(format!("resolving executable path: {err}")))?;
    std::fs::canonicalize(&path).map_err(|err| {
        Error::Unavailable(format!(
            "resolving executable symlinks for {}: {err}",
            path.display()
        ))
    })
}

fn current_rclone_version(path: &Path) -> Option<String> {
    let output = Command::new(path).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .map(str::to_owned)
}

fn os_name() -> Result<&'static str> {
    match std::env::consts::OS {
        "linux" => Ok("linux"),
        "macos" => Ok("darwin"),
        other => Err(Error::Unavailable(format!("unsupported os: {other}"))),
    }
}

fn rclone_os_name() -> Result<&'static str> {
    match std::env::consts::OS {
        "linux" => Ok("linux"),
        "macos" => Ok("osx"),
        other => Err(Error::Unavailable(format!("unsupported os: {other}"))),
    }
}

fn arch_name() -> Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("amd64"),
        "aarch64" => Ok("arm64"),
        other => Err(Error::Unavailable(format!("unsupported arch: {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use flate2::Compression;
    use flate2::write::GzEncoder;

    use super::*;

    #[derive(Default)]
    struct FakeFetcher {
        bytes: HashMap<String, Vec<u8>>,
        final_urls: HashMap<String, String>,
    }

    impl Fetcher for FakeFetcher {
        fn get_bytes(&self, url: &str) -> std::result::Result<Vec<u8>, UpdateError> {
            self.bytes
                .get(url)
                .cloned()
                .ok_or_else(|| UpdateError::Invalid(format!("missing fake URL {url}")))
        }

        fn final_url(&self, url: &str) -> std::result::Result<String, UpdateError> {
            self.final_urls
                .get(url)
                .cloned()
                .ok_or_else(|| UpdateError::Invalid(format!("missing fake redirect {url}")))
        }
    }

    #[test]
    fn parses_latest_version_from_final_release_url() {
        let mut fetcher = FakeFetcher::default();
        fetcher.final_urls.insert(
            "https://example.test/latest".to_owned(),
            "https://example.test/releases/tag/v2.1.0".to_owned(),
        );

        let latest = latest_git_sfs_version(&fetcher, "https://example.test/latest").unwrap();

        assert_eq!(latest, "v2.1.0");
    }

    #[test]
    fn parses_sha256sum_entry_for_the_exact_asset() {
        let sums = b"aaa other.tar.gz\nbbb git-sfs-v2.1.0-linux-amd64.tar.gz\n";

        let parsed = parse_sha256_sums(sums, "git-sfs-v2.1.0-linux-amd64.tar.gz").unwrap();

        assert_eq!(parsed, "bbb");
    }

    #[test]
    fn sha256_mismatch_is_rejected() {
        let err = verify_sha256(b"payload", "not-the-hash").unwrap_err();

        assert!(err.to_string().contains("SHA256 mismatch"));
    }

    #[test]
    fn extracts_git_sfs_from_tar_gz_by_basename() {
        let data = tar_gz_with_file("dir/git-sfs", b"binary");

        let extracted = extract_tar_gz(&data, "git-sfs").unwrap();

        assert_eq!(extracted, b"binary");
    }

    #[test]
    fn extracts_rclone_from_zip_by_basename() {
        let data = zip_with_file("rclone-v1.0.0-linux-amd64/rclone", b"rclone-bin");

        let extracted = extract_zip(&data, "rclone").unwrap();

        assert_eq!(extracted, b"rclone-bin");
    }

    #[test]
    fn atomic_replace_never_publishes_until_the_final_rename() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tool");
        std::fs::write(&path, b"old").unwrap();

        atomic_replace(&path, b"new").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert!(
            std::fs::read_dir(dir.path())
                .unwrap()
                .all(|entry| entry.unwrap().file_name() == "tool")
        );
    }

    fn tar_gz_with_file(path: &str, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let gz = GzEncoder::new(&mut out, Compression::default());
            let mut archive = tar::Builder::new(gz);
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            archive.append_data(&mut header, path, data).unwrap();
            let gz = archive.into_inner().unwrap();
            gz.finish().unwrap();
        }
        out
    }

    fn zip_with_file(path: &str, data: &[u8]) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        {
            let mut archive = zip::ZipWriter::new(&mut out);
            archive
                .start_file(path, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(data).unwrap();
            archive.finish().unwrap();
        }
        out.into_inner()
    }
}
