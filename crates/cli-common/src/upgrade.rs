//! `<binary> upgrade` — replace the running binary with the latest release.
//!
//! The same outcome as re-running `install.sh`, built in: find the newest
//! GitHub release tagged for *this* binary, download the archive for this
//! platform, and swap the executable in place. HTTPS via rustls.
//!
//! It is shared because both binaries are installed the same way — `install.sh`
//! fetches `esrun` and `esdev` from the same releases, under the same asset
//! naming — so the machinery for updating one is the machinery for updating the
//! other, and two copies of it would be two places for the tag format to drift.
//! What differs per binary is one word: its name.

/// The repository releases are published from.
const REPO: &str = "Open-Tech-Foundation/ES-Runtime";

/// This repository's releases, filtered to one binary's.
///
/// A [`self_update::ReleaseSource`] rather than self_update's built-in github
/// backend, and it is the tag format that forces it. Each binary is released
/// under its own tag — `esrun@0.24.0`, `esdev@0.3.0` — so:
///
/// - `/releases/latest` answers with whichever binary was published most
///   recently. It returned `esdev@0.1.0` the day esdev first shipped, which is
///   an esrun upgrade looking for an esrun archive in an esdev release.
/// - the built-in backend derives a version by stripping a leading `v` from the
///   tag, so `esrun@0.24.0` fails to parse as semver. In a *listing* that means
///   the tag is silently skipped — leaving only the pre-0.24 `v0.23.0` tags
///   visible, so `upgrade` would offer a **downgrade**.
///
/// Both are fixed in the same place: list the releases, keep the ones tagged for
/// this binary, and report each one's bare version. The download URLs travel
/// with the assets, so the tag never has to round-trip through a version parser.
struct Releases {
    binary: &'static str,
}

impl Releases {
    /// The bare semver a release tag carries, or `None` if the tag is not this
    /// binary's.
    ///
    /// `<binary>@<version>` is the current spelling for both. `esrun` also
    /// answers to the pre-0.24 bare `v<version>` (release.toml's
    /// `legacy_tag_formats`), which still has to resolve so an old binary can
    /// upgrade out of it; no other binary has ever been tagged that way.
    fn version_of<'a>(&self, tag: &'a str) -> Option<&'a str> {
        tag.strip_prefix(self.binary)
            .and_then(|rest| rest.strip_prefix('@'))
            .or_else(|| {
                (self.binary == "esrun")
                    .then(|| {
                        tag.strip_prefix('v')
                            .filter(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
                    })
                    .flatten()
            })
    }
}

impl self_update::ReleaseSource for Releases {
    fn get_releases(&self) -> self_update::Result<Vec<self_update::Release>> {
        // Newest-first, which is the order this trait asks for. 100 is the
        // API's maximum page size and far more history than an upgrade needs.
        let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=100");
        let body = reqwest::blocking::Client::builder()
            // GitHub rejects a request with no user agent.
            .user_agent(concat!("es-runtime/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(self_update::errors::Error::transport)?
            .get(&url)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .and_then(reqwest::blocking::Response::text)
            .map_err(self_update::errors::Error::transport)?;

        let listing: serde_json::Value =
            serde_json::from_str(&body).map_err(self_update::errors::Error::invalid_response)?;

        let mut releases = Vec::new();
        for entry in listing.as_array().into_iter().flatten() {
            let Some(version) = entry["tag_name"]
                .as_str()
                .and_then(|tag| self.version_of(tag))
            else {
                continue;
            };
            let mut release = self_update::Release::builder();
            release.version(version);
            for asset in entry["assets"].as_array().into_iter().flatten() {
                if let (Some(name), Some(url)) = (
                    asset["name"].as_str(),
                    asset["browser_download_url"].as_str(),
                ) {
                    release.asset(self_update::ReleaseAsset::new(name, url));
                }
            }
            // A tag that is not semver after the prefix is skipped rather than
            // failing the whole listing — one malformed release should not make
            // `upgrade` unusable.
            if let Ok(release) = release.build() {
                releases.push(release);
            }
        }
        Ok(releases)
    }
}

/// The `<os>-<arch>` token this platform's release assets are named with.
///
/// Release assets are named `<binary>-<os>-<arch>.{tar.gz,zip}` by the
/// otf-release tool (see release.toml), e.g. `esrun-linux-x86-64.tar.gz`, and
/// self_update selects the asset whose name contains its configured `target` —
/// so this is that token rather than the Rust target triple. It is the same
/// token `install.sh` builds from `uname`.
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

/// Upgrades `binary` from `current_version` to the newest release, in place.
///
/// Returns the line to print. `Err` is a `String` (not a boxed `dyn Error`) so
/// the result is `Send` and can cross the OS-thread boundary the callers run
/// this on: `self_update` drives its own blocking HTTP runtime, and dropping
/// that from inside a `#[tokio::main]` context panics.
pub fn run(binary: &'static str, current_version: &str) -> Result<String, String> {
    let status = self_update::backends::custom::Update::configure()
        .source(Releases { binary })
        .bin_name(binary)
        .target(target())
        // The archive holds the binary at its root, so `{{ bin }}` alone (which
        // self_update fills with the bin name plus the platform `.exe` suffix on
        // Windows) is the in-archive path.
        .bin_path_in_archive("{{ bin }}")
        // Disambiguate the archive from any same-target sidecar by extension.
        .asset_identifier(if cfg!(windows) { ".zip" } else { ".tar.gz" })
        .current_version(current_version)
        .show_download_progress(true)
        .build()
        .map_err(|e| e.to_string())?
        .update()
        .map_err(|e| e.to_string())?;
    Ok(if status.is_updated() {
        format!("Upgraded {binary} to {}.", status.version())
    } else {
        format!("{binary} is already up to date ({}).", status.version())
    })
}

/// Runs [`run`] on a thread of its own and prints the outcome, exiting the
/// process either way.
///
/// Both binaries dispatch `upgrade` from inside `#[tokio::main]`, and both want
/// the same three lines around it, so the thread hop lives here with the reason
/// it exists rather than being remembered twice.
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

#[cfg(test)]
mod tests {
    use super::Releases;

    #[test]
    fn a_tag_belongs_to_one_binary() {
        let esrun = Releases { binary: "esrun" };
        let esdev = Releases { binary: "esdev" };

        assert_eq!(esrun.version_of("esrun@0.24.0"), Some("0.24.0"));
        assert_eq!(esdev.version_of("esdev@0.3.0"), Some("0.3.0"));
        // The failure this source exists to prevent: esdev's release is not an
        // esrun release, whichever was published last.
        assert_eq!(esrun.version_of("esdev@0.3.0"), None);
        assert_eq!(esdev.version_of("esrun@0.24.0"), None);
    }

    #[test]
    fn only_esrun_answers_to_the_legacy_tags() {
        // `v0.23.0` predates the per-binary tags, so an esrun installed before
        // 0.24 can still upgrade out of it. esdev never had such a release, and
        // reading one as its own would offer a downgrade to another binary's
        // version.
        assert_eq!(
            Releases { binary: "esrun" }.version_of("v0.23.0"),
            Some("0.23.0")
        );
        assert_eq!(Releases { binary: "esdev" }.version_of("v0.23.0"), None);
        // A rolling tag is not a version: `v` followed by a word is a name,
        // and `nightly` is not tagged for anybody.
        assert_eq!(Releases { binary: "esrun" }.version_of("vnext"), None);
        assert_eq!(Releases { binary: "esrun" }.version_of("nightly"), None);
    }
}
