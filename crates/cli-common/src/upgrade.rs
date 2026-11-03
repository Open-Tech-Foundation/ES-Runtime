//! `<binary> upgrade` — replace the running binary with the latest release.
//!
//! The same outcome as re-running `install.sh`, built in: find the newest
//! GitHub release tagged for *this* binary, download the archive for this
//! platform, check it against the release's `checksums.txt`, and swap the
//! executable in place (DECISIONS.md D198).
//!
//! It is shared because both binaries are installed the same way — `install.sh`
//! fetches `esrun` and `esdev` from the same releases, under the same asset
//! naming — so the machinery for updating one is the machinery for updating the
//! other, and two copies of it would be two places for the tag format to drift.
//! What differs per binary is one word: its name.

use std::cmp::Ordering;
use std::io::Read;
use std::path::{Path, PathBuf};

use es_runtime_default_providers::TrustStore;
use es_runtime_default_providers::http_client::{HttpClient, Outbound};
use sha2::{Digest, Sha256};

/// The repository releases are published from.
const REPO: &str = "Open-Tech-Foundation/ES-Runtime";

/// The release listing. Newest-first, 100 per page — the API's maximum, and
/// far more history than an upgrade needs.
fn releases_url() -> String {
    format!("https://api.github.com/repos/{REPO}/releases?per_page=100")
}

/// The most of each download read: the listing, `checksums.txt`, the archive,
/// and the binary inside it. Each is far above anything a release publishes,
/// and each stops a server that never stops sending.
const LISTING_LIMIT: usize = 32 * 1024 * 1024;
const CHECKSUMS_LIMIT: usize = 1024 * 1024;
const ARCHIVE_LIMIT: usize = 512 * 1024 * 1024;
const BINARY_LIMIT: u64 = 1024 * 1024 * 1024;

/// One release of this binary: its version, and its assets by name.
#[derive(Debug)]
struct Release {
    version: Version,
    assets: Vec<(String, String)>,
}

impl Release {
    fn asset(&self, name: &str) -> Option<&str> {
        self.assets
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, url)| url.as_str())
    }
}

/// The bare semver a release tag carries, or `None` if the tag is not
/// `binary`'s.
///
/// Each binary is released under its own tag — `esrun@0.24.0`, `esdev@0.3.0`
/// — so `/releases/latest` answers with whichever was published last, and a
/// tag has to be matched to its binary before its version means anything.
/// `esrun` also answers to the pre-0.24 bare `v<version>` (release.toml's
/// `legacy_tag_formats`), so an old binary can still upgrade out of it; no
/// other binary has ever been tagged that way.
fn version_of<'a>(binary: &str, tag: &'a str) -> Option<&'a str> {
    tag.strip_prefix(binary)
        .and_then(|rest| rest.strip_prefix('@'))
        .or_else(|| {
            (binary == "esrun")
                .then(|| {
                    tag.strip_prefix('v')
                        .filter(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
                })
                .flatten()
        })
}

/// `binary`'s releases from a GitHub listing, newest version first. A tag that
/// is not semver after its prefix is skipped: one malformed release should not
/// make `upgrade` unusable.
fn parse_releases(binary: &str, listing: &[u8]) -> Result<Vec<Release>, String> {
    let listing: serde_json::Value =
        serde_json::from_slice(listing).map_err(|e| format!("the release listing: {e}"))?;
    let mut releases: Vec<Release> = listing
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["draft"] != true && entry["prerelease"] != true)
        .filter_map(|entry| {
            let version = version_of(binary, entry["tag_name"].as_str()?)?;
            let version = Version::parse(version)?;
            let assets = entry["assets"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|asset| {
                    Some((
                        asset["name"].as_str()?.to_string(),
                        asset["browser_download_url"].as_str()?.to_string(),
                    ))
                })
                .collect();
            Some(Release { version, assets })
        })
        .collect();
    releases.sort_by(|a, b| b.version.cmp(&a.version));
    Ok(releases)
}

/// The `<os>-<arch>` token this platform's release assets are named with —
/// the same token `install.sh` builds from `uname`.
fn target() -> String {
    let os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86-64"
    };
    format!("{os}-{arch}")
}

/// This platform's archive of `binary`: `esrun-linux-x86-64.tar.gz`,
/// `esrun-windows-x86-64.zip` (release.toml's naming).
fn archive_name(binary: &str) -> String {
    let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
    format!("{binary}-{}.{ext}", target())
}

/// The binary's name inside the archive, which holds it at its root.
fn binary_file(binary: &str) -> String {
    if cfg!(windows) {
        format!("{binary}.exe")
    } else {
        binary.to_string()
    }
}

/// A semantic version (semver.org 2.0.0), compared by its precedence rules.
/// Build metadata is accepted and ignored, as precedence ignores it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Version {
    core: [u64; 3],
    pre: Vec<Identifier>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Identifier {
    Numeric(u64),
    Alpha(String),
}

impl Version {
    fn parse(text: &str) -> Option<Version> {
        let text = text.split_once('+').map_or(text, |(version, build)| {
            // Build metadata must still be well formed to be ignored.
            if build
                .split('.')
                .all(|id| !id.is_empty() && id.bytes().all(is_ident_byte))
            {
                version
            } else {
                ""
            }
        });
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut parts = core.split('.');
        let mut numbers = [0u64; 3];
        for slot in &mut numbers {
            *slot = numeric(parts.next()?)?;
        }
        if parts.next().is_some() {
            return None;
        }
        let pre = match pre {
            None => Vec::new(),
            Some(pre) => pre
                .split('.')
                .map(|id| {
                    if id.is_empty() || !id.bytes().all(is_ident_byte) {
                        None
                    } else if id.bytes().all(|b| b.is_ascii_digit()) {
                        numeric(id).map(Identifier::Numeric)
                    } else {
                        Some(Identifier::Alpha(id.to_string()))
                    }
                })
                .collect::<Option<Vec<_>>>()?,
        };
        Some(Version { core: numbers, pre })
    }
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-'
}

/// A numeric identifier: digits, no leading zero unless it is `0`.
fn numeric(text: &str) -> Option<u64> {
    if text.is_empty()
        || !text.bytes().all(|b| b.is_ascii_digit())
        || (text.len() > 1 && text.starts_with('0'))
    {
        return None;
    }
    text.parse().ok()
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.core.cmp(&other.core).then_with(|| {
            match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                // A pre-release precedes its release.
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => {
                    for (a, b) in self.pre.iter().zip(&other.pre) {
                        let order = match (a, b) {
                            (Identifier::Numeric(a), Identifier::Numeric(b)) => a.cmp(b),
                            (Identifier::Numeric(_), Identifier::Alpha(_)) => Ordering::Less,
                            (Identifier::Alpha(_), Identifier::Numeric(_)) => Ordering::Greater,
                            (Identifier::Alpha(a), Identifier::Alpha(b)) => a.cmp(b),
                        };
                        if order != Ordering::Equal {
                            return order;
                        }
                    }
                    self.pre.len().cmp(&other.pre.len())
                }
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [major, minor, patch] = self.core;
        write!(f, "{major}.{minor}.{patch}")?;
        for (i, id) in self.pre.iter().enumerate() {
            f.write_str(if i == 0 { "-" } else { "." })?;
            match id {
                Identifier::Numeric(n) => write!(f, "{n}")?,
                Identifier::Alpha(s) => f.write_str(s)?,
            }
        }
        Ok(())
    }
}

/// Whether `latest` is newer than `current`. An unparseable version on either
/// side is never newer: it is reported, not acted on.
fn is_newer(current: &str, latest: &Version) -> bool {
    Version::parse(current).is_some_and(|current| *latest > current)
}

/// What a check reports: `current` against the newest listed release.
///
/// `latest` is the newest release's version and whether it published an
/// archive for this platform — `None` when no release of this binary is
/// listed at all. Pure so it is unit-testable; the network stays in [`check`].
fn describe(binary: &str, current_version: &str, latest: Option<(&Version, bool)>) -> String {
    let Some((version, has_asset)) = latest else {
        return format!("no {binary} releases found; nothing to upgrade to.");
    };
    if !is_newer(current_version, version) {
        // Equal, newer (a build ahead of the releases), or unparseable.
        return format!("{binary} is already up to date ({current_version}).");
    }
    let mut message =
        format!("{binary} {current_version} can upgrade to {version}: run `{binary} upgrade`.");
    if !has_asset {
        message.push_str(&format!(
            " (this release published no {} build yet)",
            target()
        ));
    }
    message
}

/// GETs `url`, following redirects, refusing more than `limit` bytes.
async fn download(client: &HttpClient, url: &str, limit: usize) -> Result<Vec<u8>, String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("{url}: {e}"))?;
    let mut request = Outbound::get(parsed);
    // GitHub rejects a request with no user agent.
    request.headers = vec![(
        "user-agent".to_string(),
        concat!("es-runtime/", env!("CARGO_PKG_VERSION")).to_string(),
    )];
    let response = client
        .send(request)
        .await
        .map_err(|e| format!("{url}: {e}"))?;
    if !response.status.is_success() {
        return Err(format!("{url} returned {}", response.status));
    }
    response
        .bytes(limit)
        .await
        .map_err(|e| format!("{url}: {e}"))
}

/// The SHA-256 `checksums.txt` lists for `name`, as lowercase hex.
///
/// Lines are `sha256sum`'s output, `<hex>  <name>` (or `<hex> *<name>` in
/// binary mode).
fn listed_checksum(checksums: &str, name: &str) -> Option<String> {
    checksums.lines().find_map(|line| {
        let (hash, file) = line.trim().split_once(char::is_whitespace)?;
        let file = file.trim_start().trim_start_matches('*');
        (file == name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The file named `name` at the root of a `.tar.gz`, read whole. Only a
/// regular file counts: a link of that name is not the binary.
fn from_tar_gz(archive: &[u8], name: &str) -> Result<Vec<u8>, String> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let entries = tar.entries().map_err(|e| format!("the archive: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("the archive: {e}"))?;
        let path = entry.path().map_err(|e| format!("the archive: {e}"))?;
        let at_root = path
            .strip_prefix(".")
            .unwrap_or(&path)
            .to_str()
            .is_some_and(|p| p == name);
        if !at_root {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            return Err(format!("the archive's {name} is not a regular file"));
        }
        return read_capped(entry, name);
    }
    Err(format!("the archive has no {name} at its root"))
}

/// The file named `name` at the root of a `.zip`, read whole.
fn from_zip(archive: &[u8], name: &str) -> Result<Vec<u8>, String> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))
        .map_err(|e| format!("the archive: {e}"))?;
    let file = zip
        .by_name(name)
        .map_err(|_| format!("the archive has no {name} at its root"))?;
    if !file.is_file() {
        return Err(format!("the archive's {name} is not a regular file"));
    }
    read_capped(file, name)
}

fn read_capped(reader: impl Read, name: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(BINARY_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("the archive's {name}: {e}"))?;
    if bytes.len() as u64 > BINARY_LIMIT {
        return Err(format!("the archive's {name} is larger than any release"));
    }
    Ok(bytes)
}

/// Writes `binary` beside `exe` and renames it into place.
///
/// The rename is what makes this safe to interrupt: until it happens the old
/// binary is untouched, and after it the new one is whole. On Windows a
/// running executable cannot be replaced but can be renamed, so it is moved
/// aside first, and the copy left behind is removed by the next upgrade.
fn install(exe: &Path, binary: &[u8]) -> Result<(), String> {
    let dir = exe
        .parent()
        .ok_or_else(|| format!("{} has no directory", exe.display()))?;
    let name = exe
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{} has no file name", exe.display()))?;
    let staged = dir.join(format!(".{name}.upgrade-{}", std::process::id()));
    let write = || -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::File::create(&staged)?;
        file.write_all(binary)?;
        file.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
        }
        Ok(())
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&staged);
        return Err(format!("cannot write {}: {e}", staged.display()));
    }
    let swapped = swap(exe, &staged);
    if swapped.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    swapped
}

#[cfg(not(windows))]
fn swap(exe: &Path, staged: &Path) -> Result<(), String> {
    std::fs::rename(staged, exe).map_err(|e| format!("cannot replace {}: {e}", exe.display()))
}

#[cfg(windows)]
fn swap(exe: &Path, staged: &Path) -> Result<(), String> {
    let aside = set_aside(exe);
    let _ = std::fs::remove_file(&aside);
    std::fs::rename(exe, &aside)
        .map_err(|e| format!("cannot move {} aside: {e}", exe.display()))?;
    if let Err(e) = std::fs::rename(staged, exe) {
        let _ = std::fs::rename(&aside, exe);
        return Err(format!("cannot replace {}: {e}", exe.display()));
    }
    // Gone now unless it is still running, in which case the next upgrade
    // removes it.
    let _ = std::fs::remove_file(&aside);
    Ok(())
}

/// Where Windows keeps the binary an upgrade replaced, until the next one.
#[cfg_attr(not(windows), allow(dead_code))]
fn set_aside(exe: &Path) -> PathBuf {
    let mut name = exe.as_os_str().to_owned();
    name.push(".old");
    PathBuf::from(name)
}

/// The installed binary: the file behind `current_exe`, symlinks resolved, so
/// a link in `PATH` keeps pointing at the upgraded file rather than being
/// replaced by a copy.
fn installed_exe() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this binary: {e}"))?;
    es_runtime_default_providers::path::canonicalize(&exe)
        .map_err(|e| format!("cannot resolve {}: {e}", exe.display()))
}

/// What one upgrade attempt did.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Current(String),
    Upgraded(Version),
}

/// The upgrade, against a listing at `listing_url`, installing over `exe`.
/// Separate from [`run`] so tests can point it at a local server and a
/// scratch file.
async fn upgrade(
    client: &HttpClient,
    listing_url: &str,
    binary: &str,
    current_version: &str,
    exe: &Path,
) -> Result<Outcome, String> {
    #[cfg(windows)]
    let _ = std::fs::remove_file(set_aside(exe));
    let listing = download(client, listing_url, LISTING_LIMIT).await?;
    let releases = parse_releases(binary, &listing)?;
    let Some(latest) = releases.first() else {
        return Ok(Outcome::Current(format!(
            "no {binary} releases found; nothing to upgrade to."
        )));
    };
    if !is_newer(current_version, &latest.version) {
        return Ok(Outcome::Current(format!(
            "{binary} is already up to date ({current_version})."
        )));
    }
    let version = &latest.version;
    let name = archive_name(binary);
    let archive_url = latest
        .asset(&name)
        .ok_or_else(|| format!("{binary} {version} published no {} build", target()))?;
    // Required, not best-effort: this replaces a binary that already works,
    // so there is no reason to accept one nobody can vouch for.
    let checksums_url = latest.asset("checksums.txt").ok_or_else(|| {
        format!("{binary} {version} publishes no checksums.txt; refusing to install it unverified")
    })?;
    let checksums = download(client, checksums_url, CHECKSUMS_LIMIT).await?;
    let expected = listed_checksum(&String::from_utf8_lossy(&checksums), &name)
        .ok_or_else(|| format!("checksums.txt does not list {name}; refusing to install it"))?;
    eprintln!("Downloading {binary} {version} ({})...", target());
    let archive = download(client, archive_url, ARCHIVE_LIMIT).await?;
    let actual = sha256_hex(&archive);
    if actual != expected {
        return Err(format!(
            "{name} does not match checksums.txt (expected SHA-256 {expected}, got {actual}); \
             nothing was installed"
        ));
    }
    let file = binary_file(binary);
    let bytes = if name.ends_with(".zip") {
        from_zip(&archive, &file)?
    } else {
        from_tar_gz(&archive, &file)?
    };
    install(exe, &bytes)?;
    Ok(Outcome::Upgraded(version.clone()))
}

fn client() -> HttpClient {
    HttpClient::new(&TrustStore::bundled())
}

fn block_on<T>(future: impl std::future::Future<Output = T>) -> Result<T, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map(|runtime| runtime.block_on(future))
        .map_err(|e| format!("cannot start the upgrade: {e}"))
}

/// Asks whether `binary` has a newer release, changing nothing.
///
/// The read half of [`run`]: the same release listing, but it stops at
/// comparing versions instead of downloading. Returns the line to print.
pub fn check(binary: &'static str, current_version: &str) -> Result<String, String> {
    let listing = block_on(async { download(&client(), &releases_url(), LISTING_LIMIT).await })??;
    let releases = parse_releases(binary, &listing)?;
    let latest = releases.first().map(|release| {
        (
            &release.version,
            release.asset(&archive_name(binary)).is_some(),
        )
    });
    Ok(describe(binary, current_version, latest))
}

/// Upgrades `binary` from `current_version` to the newest release, in place.
///
/// Returns the line to print. Runs on a runtime of its own, so a caller
/// already inside one calls it from another thread ([`run_and_exit`]).
pub fn run(binary: &'static str, current_version: &str) -> Result<String, String> {
    let exe = installed_exe()?;
    let outcome = block_on(async {
        upgrade(&client(), &releases_url(), binary, current_version, &exe).await
    })??;
    Ok(match outcome {
        Outcome::Current(message) => message,
        Outcome::Upgraded(version) => format!("Upgraded {binary} to {version}."),
    })
}

/// Runs [`run`] on a thread of its own and prints the outcome, exiting the
/// process either way.
///
/// Both binaries dispatch `upgrade` from inside `#[tokio::main]`, where a
/// second runtime cannot be started on the same thread, so the thread hop
/// lives here with the reason it exists rather than being remembered twice.
pub fn run_and_exit(binary: &'static str, current_version: &'static str) -> ! {
    let result = std::thread::spawn(move || run(binary, current_version))
        .join()
        .unwrap_or_else(|_| Err("the upgrade thread panicked".to_string()));
    match result {
        Ok(message) => {
            println!("{message}");
            std::process::exit(0)
        }
        Err(e) => {
            eprintln!("error: upgrade failed: {e}");
            std::process::exit(1)
        }
    }
}

/// Runs [`check`] on a thread of its own and prints the outcome, exiting the
/// process either way. The same thread hop as [`run_and_exit`].
pub fn check_and_exit(binary: &'static str, current_version: &'static str) -> ! {
    let result = std::thread::spawn(move || check(binary, current_version))
        .join()
        .unwrap_or_else(|_| Err("the upgrade thread panicked".to_string()));
    match result {
        Ok(message) => {
            println!("{message}");
            std::process::exit(0)
        }
        Err(e) => {
            eprintln!("error: upgrade check failed: {e}");
            std::process::exit(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|| panic!("{text} should parse"))
    }

    #[test]
    fn a_tag_belongs_to_one_binary() {
        assert_eq!(version_of("esrun", "esrun@0.24.0"), Some("0.24.0"));
        assert_eq!(version_of("esdev", "esdev@0.3.0"), Some("0.3.0"));
        // esdev's release is not an esrun release, whichever was published last.
        assert_eq!(version_of("esrun", "esdev@0.3.0"), None);
        assert_eq!(version_of("esdev", "esrun@0.24.0"), None);
    }

    #[test]
    fn only_esrun_answers_to_the_legacy_tags() {
        assert_eq!(version_of("esrun", "v0.23.0"), Some("0.23.0"));
        assert_eq!(version_of("esdev", "v0.23.0"), None);
        assert_eq!(version_of("esrun", "vnext"), None);
        assert_eq!(version_of("esrun", "nightly"), None);
    }

    // semver.org §11's own example, in order.
    #[test]
    fn versions_order_by_semver_precedence() {
        let ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.1.0",
            "2.0.0",
            "10.0.0",
        ];
        for pair in ordered.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
        }
        assert_eq!(v("1.0.0+build.5"), v("1.0.0"), "build metadata is ignored");
        assert_eq!(v("1.2.3-rc.1").to_string(), "1.2.3-rc.1");
    }

    #[test]
    fn malformed_versions_are_refused() {
        for bad in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "01.2.3",
            "1.02.3",
            "1.2.03",
            "1.2.3-",
            "1.2.3-01",
            "1.2.3-a..b",
            "1.2.3+",
            "1.2.3+a..b",
            "1.2.x",
            "v1.2.3",
            "1.2.3-é",
            " 1.2.3",
        ] {
            assert!(Version::parse(bad).is_none(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn check_names_the_newer_release() {
        assert_eq!(
            describe("esdev", "0.7.0", Some((&v("0.8.0"), true))),
            "esdev 0.7.0 can upgrade to 0.8.0: run `esdev upgrade`."
        );
        assert!(describe("esdev", "0.7.0", Some((&v("0.8.0"), false))).contains("0.8.0"));
    }

    #[test]
    fn check_is_quiet_when_there_is_nothing_to_move_to() {
        assert_eq!(
            describe("esdev", "0.7.0", Some((&v("0.7.0"), true))),
            "esdev is already up to date (0.7.0)."
        );
        // A build ahead of the releases is current, never "upgraded" backwards.
        assert_eq!(
            describe("esdev", "0.8.0", Some((&v("0.7.0"), true))),
            "esdev is already up to date (0.8.0)."
        );
        assert_eq!(
            describe("esdev", "not-a-version", Some((&v("0.7.0"), true))),
            "esdev is already up to date (not-a-version)."
        );
        assert_eq!(
            describe("esdev", "0.7.0", None),
            "no esdev releases found; nothing to upgrade to."
        );
    }

    #[test]
    fn the_listing_is_sorted_by_version_not_by_order() {
        let listing = serde_json::json!([
            { "tag_name": "esrun@0.9.0", "assets": [] },
            { "tag_name": "esrun@0.10.0", "assets": [] },
            { "tag_name": "esdev@9.0.0", "assets": [] },
            { "tag_name": "esrun@0.11.0-rc.1", "assets": [], "prerelease": true },
            { "tag_name": "esrun@0.12.0", "assets": [], "draft": true },
            { "tag_name": "esrun@garbage", "assets": [] },
        ]);
        let releases = parse_releases("esrun", listing.to_string().as_bytes()).unwrap();
        let versions: Vec<String> = releases.iter().map(|r| r.version.to_string()).collect();
        assert_eq!(versions, ["0.10.0", "0.9.0"]);
    }

    #[test]
    fn reads_sha256sum_output() {
        let hash = "a".repeat(64);
        let text = format!(
            "{hash}  esrun-linux-x86-64.tar.gz\n{}  other.zip\n",
            "b".repeat(64)
        );
        assert_eq!(
            listed_checksum(&text, "esrun-linux-x86-64.tar.gz"),
            Some(hash.clone())
        );
        assert_eq!(
            listed_checksum(&format!("{hash} *x.zip"), "x.zip"),
            Some(hash.clone())
        );
        assert_eq!(
            listed_checksum(&text, "esrun-linux-x86-64"),
            None,
            "no prefix match"
        );
        assert_eq!(
            listed_checksum(&format!("{}  x.zip", "a".repeat(63)), "x.zip"),
            None
        );
        assert_eq!(
            listed_checksum(&format!("{}  x.zip", "g".repeat(64)), "x.zip"),
            None
        );
    }

    /// A `.tar.gz` holding `entries` (name, contents); a `None` content is a
    /// symlink of that name.
    fn tar_gz(entries: &[(&str, Option<&[u8]>)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (name, contents) in entries {
            let mut header = tar::Header::new_gnu();
            match contents {
                Some(bytes) => {
                    header.set_size(bytes.len() as u64);
                    header.set_mode(0o755);
                    header.set_entry_type(tar::EntryType::Regular);
                    header.set_cksum();
                    builder.append_data(&mut header, name, *bytes).unwrap();
                }
                None => {
                    header.set_size(0);
                    header.set_entry_type(tar::EntryType::Symlink);
                    builder
                        .append_link(&mut header, name, "/etc/passwd")
                        .unwrap();
                }
            }
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, contents) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(contents).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn reads_only_the_binary_at_the_archive_root() {
        let archive = tar_gz(&[("README", Some(b"docs")), ("esrun", Some(b"BINARY"))]);
        assert_eq!(from_tar_gz(&archive, "esrun").unwrap(), b"BINARY");
        let dotted = tar_gz(&[("./esrun", Some(b"BINARY"))]);
        assert_eq!(from_tar_gz(&dotted, "esrun").unwrap(), b"BINARY");
        let nested = tar_gz(&[("bin/esrun", Some(b"BINARY"))]);
        assert!(
            from_tar_gz(&nested, "esrun")
                .unwrap_err()
                .contains("no esrun")
        );
        let missing = tar_gz(&[("other", Some(b"x"))]);
        assert!(
            from_tar_gz(&missing, "esrun")
                .unwrap_err()
                .contains("no esrun")
        );
        assert!(from_tar_gz(b"not gzip", "esrun").is_err());

        let zip = zip_of(&[("esrun.exe", b"WINBINARY")]);
        assert_eq!(from_zip(&zip, "esrun.exe").unwrap(), b"WINBINARY");
        assert!(
            from_zip(&zip, "esdev.exe")
                .unwrap_err()
                .contains("no esdev.exe")
        );
        assert!(from_zip(b"not a zip", "esrun.exe").is_err());
    }

    // A link named like the binary would install whatever it points at.
    #[test]
    fn a_link_in_place_of_the_binary_is_refused() {
        let archive = tar_gz(&[("esrun", None)]);
        let err = from_tar_gz(&archive, "esrun").unwrap_err();
        assert!(err.contains("not a regular file"), "{err}");
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("esrun-upgrade-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn install_replaces_the_file_and_leaves_nothing_behind() {
        let dir = scratch("install");
        let exe = dir.join("esrun");
        std::fs::write(&exe, b"OLD").unwrap();
        install(&exe, b"NEW").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"NEW");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, ["esrun"], "a staging file was left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Windows refuses to overwrite a running executable but lets it be
    // renamed, which is what the swap relies on: the running copy moves
    // aside, the new one takes its name, and the next upgrade removes the old.
    #[cfg(windows)]
    #[test]
    fn a_running_executable_is_replaced_on_windows() {
        let dir = scratch("running");
        let exe = dir.join("running.exe");
        // A copy of this test binary, kept running by the ignored test below.
        std::fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        let mut child = std::process::Command::new(&exe)
            .args(["--ignored", "--exact", "upgrade::tests::stays_running"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        let installed = install(&exe, b"NEW");
        let _ = child.kill();
        let _ = child.wait();
        installed.unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"NEW");
        // The next upgrade clears the set-aside copy before swapping again.
        install(&exe, b"NEWER").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"NEWER");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Not a test: the process `a_running_executable_is_replaced_on_windows`
    /// keeps running while it replaces the file.
    #[test]
    #[ignore = "a helper process for the Windows replacement test"]
    fn stays_running() {
        std::thread::sleep(std::time::Duration::from_secs(30));
    }

    /// A release server on loopback: `GET <path>` answers with `files[path]`,
    /// anything else with a 404.
    async fn release_server(files: HashMap<String, Vec<u8>>) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let files = Arc::new(files);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let files = files.clone();
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut buf = [0u8; 1024];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        let Ok(n) = socket.read(&mut buf).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        head.extend_from_slice(&buf[..n]);
                    }
                    let head = String::from_utf8_lossy(&head);
                    let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let (status, body) = match files.get(&path) {
                        Some(body) => ("200 OK", body.clone()),
                        None => ("404 Not Found", Vec::new()),
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                });
            }
        });
        port
    }

    /// A release of `esrun` 9.9.9 whose archive holds `binary`, published with
    /// `checksums` (`None`: no checksums.txt at all).
    struct Published {
        archive: Vec<u8>,
        checksums: Option<String>,
    }

    impl Published {
        fn honest(binary: &[u8]) -> Published {
            let archive = if cfg!(windows) {
                zip_of(&[("esrun.exe", binary)])
            } else {
                tar_gz(&[("esrun", Some(binary))])
            };
            let checksums = Some(format!(
                "{}  {}\n",
                sha256_hex(&archive),
                archive_name("esrun")
            ));
            Published { archive, checksums }
        }
    }

    /// Runs an upgrade from `current` against `published`, over a scratch
    /// binary holding `OLD`. Returns the outcome and the binary's contents after.
    async fn upgrade_against(
        published: Published,
        current: &str,
    ) -> (Result<Outcome, String>, Vec<u8>) {
        let mut files = HashMap::new();
        files.insert("/archive".to_string(), published.archive);
        let port_placeholder = "PORT";
        let mut assets = vec![serde_json::json!({
            "name": archive_name("esrun"),
            "browser_download_url": format!("http://127.0.0.1:{port_placeholder}/archive"),
        })];
        if let Some(checksums) = published.checksums {
            files.insert("/checksums.txt".to_string(), checksums.into_bytes());
            assets.push(serde_json::json!({
                "name": "checksums.txt",
                "browser_download_url": format!("http://127.0.0.1:{port_placeholder}/checksums.txt"),
            }));
        }
        let listing =
            serde_json::json!([{ "tag_name": "esrun@9.9.9", "assets": assets }]).to_string();
        // The port is known only once the server is up, so the listing is
        // served from a second server that knows it.
        let port = release_server(files).await;
        let mut listing_files = HashMap::new();
        listing_files.insert(
            "/releases".to_string(),
            listing.replace("PORT", &port.to_string()).into_bytes(),
        );
        let listing_port = release_server(listing_files).await;

        let dir = scratch(&format!("flow-{port}"));
        let exe = dir.join("esrun");
        std::fs::write(&exe, b"OLD").unwrap();
        let client = HttpClient::new(&TrustStore::bundled());
        let url = format!("http://127.0.0.1:{listing_port}/releases");
        let outcome = upgrade(&client, &url, "esrun", current, &exe).await;
        let after = std::fs::read(&exe).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        (outcome, after)
    }

    #[tokio::test]
    async fn a_verified_release_is_installed() {
        let (outcome, after) = upgrade_against(Published::honest(b"NEW"), "0.1.0").await;
        assert_eq!(outcome, Ok(Outcome::Upgraded(v("9.9.9"))));
        assert_eq!(after, b"NEW");
    }

    #[tokio::test]
    async fn an_archive_that_does_not_match_its_checksum_is_not_installed() {
        let mut published = Published::honest(b"NEW");
        published.checksums = Some(format!("{}  {}\n", "0".repeat(64), archive_name("esrun")));
        let (outcome, after) = upgrade_against(published, "0.1.0").await;
        let err = outcome.unwrap_err();
        assert!(err.contains("does not match checksums.txt"), "{err}");
        assert_eq!(after, b"OLD", "the binary was replaced anyway");
    }

    #[tokio::test]
    async fn a_tampered_archive_is_not_installed() {
        let honest = Published::honest(b"NEW");
        let tampered = Published {
            archive: Published::honest(b"EVIL").archive,
            checksums: honest.checksums,
        };
        let (outcome, after) = upgrade_against(tampered, "0.1.0").await;
        assert!(
            outcome
                .unwrap_err()
                .contains("does not match checksums.txt")
        );
        assert_eq!(after, b"OLD");
    }

    #[tokio::test]
    async fn a_release_without_checksums_is_not_installed() {
        let mut published = Published::honest(b"NEW");
        published.checksums = None;
        let (outcome, after) = upgrade_against(published, "0.1.0").await;
        let err = outcome.unwrap_err();
        assert!(err.contains("publishes no checksums.txt"), "{err}");
        assert_eq!(after, b"OLD");
    }

    #[tokio::test]
    async fn checksums_that_do_not_list_the_archive_are_refused() {
        let mut published = Published::honest(b"NEW");
        published.checksums = Some(format!("{}  something-else.tar.gz\n", "a".repeat(64)));
        let (outcome, after) = upgrade_against(published, "0.1.0").await;
        let err = outcome.unwrap_err();
        assert!(err.contains("does not list"), "{err}");
        assert_eq!(after, b"OLD");
    }

    #[tokio::test]
    async fn a_current_binary_downloads_nothing() {
        let (outcome, after) = upgrade_against(Published::honest(b"NEW"), "9.9.9").await;
        assert_eq!(
            outcome,
            Ok(Outcome::Current(
                "esrun is already up to date (9.9.9).".to_string()
            ))
        );
        assert_eq!(after, b"OLD");
        let (outcome, after) = upgrade_against(Published::honest(b"NEW"), "10.0.0").await;
        assert!(
            matches!(outcome, Ok(Outcome::Current(_))),
            "a newer build was downgraded"
        );
        assert_eq!(after, b"OLD");
    }
}
