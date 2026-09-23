//! Which browser `esdev test --browser` runs the test files in.
//!
//! **Standard WebDriver BiDi, and nothing the environment did not provide.**
//! A browser is *available* only when it can be driven over BiDi with what is
//! already installed: Firefox serves BiDi itself, while Chrome and Edge serve
//! only CDP and need their vendor's driver (`chromedriver`, `msedgedriver`) to
//! speak the standard protocol. esdev downloads nothing — a missing browser or
//! driver is an error that says what is missing, never a fetch.
//!
//! **Automatic choice is an order, and it says what it passed over.** `auto`
//! takes the first available of Chrome, Chromium, Firefox, Edge, Safari, and
//! reports why
//! each one ahead of it was skipped, so "the suite ran in Firefox" is never a
//! surprise found in a CI log a week later. A browser named outright is that
//! browser or a failure: no fallback, because a run that asked for Chrome and
//! quietly got Firefox has answered a different question.
//!
//! **A driver is the browser's, or it is not a driver.** chromedriver serves
//! only the major version it was built for, and a distribution's driver package
//! moves with *its* browser — Debian's `chromium-driver` upgrades Chromium, not
//! the Google Chrome installed beside it. So Chrome and Chromium are two
//! browsers, each paired with the driver only when their major versions agree;
//! a mismatch is a browser that cannot be driven, said as one, rather than a
//! session that fails to start with the driver's own message.
//!
//! Safari is in the order so that its place is decided, and is never available
//! yet: its BiDi support is not ready, and a partial protocol would fail a test
//! file for reasons that are the driver's rather than the file's.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

/// A browser a test file can run in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Browser {
    Chrome,
    Chromium,
    Firefox,
    Edge,
    Safari,
}

/// The order `auto` tries them in.
pub const ORDER: [Browser; 5] = [
    Browser::Chrome,
    Browser::Chromium,
    Browser::Firefox,
    Browser::Edge,
    Browser::Safari,
];

impl Browser {
    /// The name a flag or `esdev.json` spells it with.
    pub fn name(self) -> &'static str {
        match self {
            Browser::Chrome => "chrome",
            Browser::Chromium => "chromium",
            Browser::Firefox => "firefox",
            Browser::Edge => "edge",
            Browser::Safari => "safari",
        }
    }

    fn parse(name: &str) -> Option<Browser> {
        ORDER.into_iter().find(|browser| browser.name() == name)
    }

    /// The executable names looked up on `PATH`, most specific first.
    fn commands(self) -> &'static [&'static str] {
        match self {
            Browser::Chrome => &["google-chrome", "google-chrome-stable", "chrome"],
            Browser::Chromium => &["chromium", "chromium-browser"],
            Browser::Firefox => &["firefox"],
            Browser::Edge => &["microsoft-edge", "microsoft-edge-stable", "msedge"],
            Browser::Safari => &[],
        }
    }

    /// Where the vendor's installer puts it when that is not on `PATH` — the
    /// ordinary state of a macOS or Windows machine. Each is `(base, rest)`:
    /// an environment variable naming a directory, or `None` for an absolute
    /// path, and the path under it.
    fn installed(self) -> &'static [(Option<&'static str>, &'static str)] {
        match self {
            Browser::Chrome => &[
                (
                    None,
                    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                ),
                (Some("PROGRAMFILES"), "Google/Chrome/Application/chrome.exe"),
                (
                    Some("PROGRAMFILES(X86)"),
                    "Google/Chrome/Application/chrome.exe",
                ),
                (Some("LOCALAPPDATA"), "Google/Chrome/Application/chrome.exe"),
            ],
            Browser::Chromium => &[
                (None, "/Applications/Chromium.app/Contents/MacOS/Chromium"),
                (Some("LOCALAPPDATA"), "Chromium/Application/chrome.exe"),
            ],
            Browser::Firefox => &[
                (None, "/Applications/Firefox.app/Contents/MacOS/firefox"),
                (Some("PROGRAMFILES"), "Mozilla Firefox/firefox.exe"),
                (Some("PROGRAMFILES(X86)"), "Mozilla Firefox/firefox.exe"),
            ],
            Browser::Edge => &[
                (
                    None,
                    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
                ),
                (
                    Some("PROGRAMFILES"),
                    "Microsoft/Edge/Application/msedge.exe",
                ),
                (
                    Some("PROGRAMFILES(X86)"),
                    "Microsoft/Edge/Application/msedge.exe",
                ),
            ],
            Browser::Safari => &[],
        }
    }

    /// The driver that serves BiDi for it, when the browser does not serve it
    /// itself.
    fn driver(self) -> Option<&'static str> {
        match self {
            Browser::Chrome | Browser::Chromium => Some("chromedriver"),
            Browser::Edge => Some("msedgedriver"),
            Browser::Firefox | Browser::Safari => None,
        }
    }
}

impl fmt::Display for Browser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// What `--browser` or `test.browser` asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// The first available, in [`ORDER`].
    Auto,
    /// This one, or a failure.
    Named(Browser),
}

impl Choice {
    /// Parses a flag value or `esdev.json` string. The error lists what is
    /// accepted; the caller says where the bad value came from.
    pub fn parse(text: &str) -> Result<Choice, String> {
        if text == "auto" {
            return Ok(Choice::Auto);
        }
        Browser::parse(text).map(Choice::Named).ok_or_else(|| {
            format!(
                "`{text}` is not a browser.\n\n  \
                 auto      — the first available of chrome, chromium, firefox, edge, safari\n  \
                 chrome    — Google Chrome; needs the matching chromedriver on PATH\n  \
                 chromium  — needs the matching chromedriver on PATH\n  \
                 firefox   — serves WebDriver BiDi itself\n  \
                 edge      — needs the matching msedgedriver on PATH\n  \
                 safari    — not supported yet"
            )
        })
    }
}

/// A browser that can be driven, and what drives it.
#[derive(Debug, PartialEq, Eq)]
pub struct Launch {
    pub browser: Browser,
    pub binary: PathBuf,
    /// Its major version, when it could be read.
    pub version: Option<u32>,
    /// The vendor's driver, for a browser that does not serve BiDi itself.
    pub driver: Option<PathBuf>,
}

/// The browser chosen, and why every one ahead of it was not.
#[derive(Debug)]
pub struct Selected {
    pub launch: Launch,
    pub skipped: Vec<(Browser, String)>,
}

impl Selected {
    /// The lines a run prints before it starts: what it runs in, and what it
    /// passed over on the way.
    pub fn describe(&self) -> String {
        let mut text = format!("browser: {}", self.launch.browser);
        if let Some(version) = self.launch.version {
            text.push_str(&format!(" {version}"));
        }
        text.push_str(&format!(" ({}", self.launch.binary.display()));
        if let Some(driver) = &self.launch.driver {
            text.push_str(&format!(", driven by {}", driver.display()));
        }
        text.push(')');
        for (browser, reason) in &self.skipped {
            text.push_str(&format!("\n  skipped {browser}: {reason}"));
        }
        text
    }
}

/// What the machine has. A trait so selection can be tested against a machine
/// that is described rather than the one the tests run on.
pub trait Probe {
    /// An executable of this name on `PATH`.
    fn which(&self, command: &str) -> Option<PathBuf>;
    /// Whether this path is a file.
    fn is_file(&self, path: &Path) -> bool;
    /// An environment variable.
    fn var(&self, name: &str) -> Option<OsString>;
    /// What an executable says its version is: `--version`'s output, in
    /// whatever words the vendor chose. `None` when it will not say.
    fn version(&self, executable: &Path) -> Option<String>;
}

/// The machine esdev is running on.
pub struct System;

impl Probe for System {
    fn which(&self, command: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        let names: Vec<String> = if cfg!(windows) {
            vec![format!("{command}.exe"), command.to_string()]
        } else {
            vec![command.to_string()]
        };
        std::env::split_paths(&path)
            .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
            .find(|candidate| candidate.is_file())
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn var(&self, name: &str) -> Option<OsString> {
        std::env::var_os(name)
    }

    fn version(&self, executable: &Path) -> Option<String> {
        // `chrome.exe --version` prints nothing on Windows — it opens a
        // window. The installer keeps each version's files in a directory
        // named for it beside the executable, which is what Chrome's own
        // updater reads.
        if cfg!(windows) && executable.extension().is_some_and(|ext| ext == "exe") {
            let dir = executable.parent()?;
            let named = std::fs::read_dir(dir)
                .ok()?
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_dir())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| major(name).is_some())
                .max_by_key(|name| major(name));
            if named.is_some() {
                return named;
            }
        }
        let output = std::process::Command::new(executable)
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// The major version in what an executable printed for `--version`: the first
/// word that is a dotted version. `Google Chrome 120.0.6099.71`,
/// `ChromeDriver 131.0.6778.85 (…)`, `Microsoft Edge WebDriver 140.0.1.2`.
fn major(text: &str) -> Option<u32> {
    text.split_whitespace()
        .find(|word| {
            word.contains('.')
                && word
                    .split('.')
                    .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        })
        .and_then(|word| word.split('.').next()?.parse().ok())
}

/// Finds one browser, or says what is missing.
pub fn find(browser: Browser, probe: &dyn Probe) -> Result<Launch, String> {
    if browser == Browser::Safari {
        return Err("Safari's WebDriver BiDi support is not ready yet".to_string());
    }
    let binary = browser
        .commands()
        .iter()
        .find_map(|command| probe.which(command))
        .or_else(|| {
            browser.installed().iter().find_map(|(base, rest)| {
                let path = match base {
                    None => PathBuf::from(rest),
                    Some(var) => PathBuf::from(probe.var(var)?).join(rest),
                };
                probe.is_file(&path).then_some(path)
            })
        })
        .ok_or_else(|| {
            format!(
                "not installed (looked for {} on PATH)",
                browser.commands().join(", ")
            )
        })?;
    let version = probe.version(&binary).as_deref().and_then(major);
    let driver = match browser.driver() {
        None => None,
        Some(command) => {
            let matching = version.map_or(String::new(), |v| format!(" {v}"));
            let driver = probe.which(command).ok_or_else(|| {
                format!(
                    "{command} not found on PATH — {browser} speaks WebDriver BiDi only \
                     through it; install the {command}{matching} matching {}",
                    binary.display()
                )
            })?;
            // Only a disagreement is refused. A version either side will not
            // state is left to the driver, which checks it again when the
            // session starts.
            let driven = probe.version(&driver).as_deref().and_then(major);
            if let (Some(want), Some(have)) = (version, driven)
                && want != have
            {
                return Err(format!(
                    "{command} {have} ({}) does not match {browser} {want} ({}); \
                     a driver serves only its own major version",
                    driver.display(),
                    binary.display()
                ));
            }
            Some(driver)
        }
    };
    Ok(Launch {
        browser,
        binary,
        version,
        driver,
    })
}

/// Resolves a choice to the browser the run will use.
pub fn select(choice: Choice, probe: &dyn Probe) -> Result<Selected, String> {
    match choice {
        Choice::Named(browser) => find(browser, probe)
            .map(|launch| Selected {
                launch,
                skipped: Vec::new(),
            })
            .map_err(|reason| format!("cannot run the tests in {browser}: {reason}.")),
        Choice::Auto => {
            let mut skipped = Vec::new();
            for browser in ORDER {
                match find(browser, probe) {
                    Ok(launch) => return Ok(Selected { launch, skipped }),
                    Err(reason) => skipped.push((browser, reason)),
                }
            }
            let mut text =
                String::from("no browser that can run the tests over WebDriver BiDi was found.\n");
            for (browser, reason) in &skipped {
                text.push_str(&format!("\n  {browser}: {reason}"));
            }
            text.push_str(
                "\n\nesdev downloads neither browsers nor drivers: install one, \
                 and put its driver on PATH where it needs one.",
            );
            Err(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    /// A machine described by what is on its `PATH`, which files exist, and
    /// its environment.
    #[derive(Default)]
    struct Machine {
        path: HashMap<&'static str, &'static str>,
        files: HashSet<PathBuf>,
        vars: HashMap<&'static str, &'static str>,
        versions: HashMap<&'static str, &'static str>,
    }

    impl Machine {
        fn on_path(mut self, command: &'static str, at: &'static str) -> Self {
            self.path.insert(command, at);
            self
        }
        fn file(mut self, path: impl Into<PathBuf>) -> Self {
            self.files.insert(path.into());
            self
        }
        fn var(mut self, name: &'static str, value: &'static str) -> Self {
            self.vars.insert(name, value);
            self
        }
        /// What the executable at `at` prints for `--version`.
        fn says(mut self, at: &'static str, version: &'static str) -> Self {
            self.versions.insert(at, version);
            self
        }
    }

    impl Probe for Machine {
        fn which(&self, command: &str) -> Option<PathBuf> {
            self.path.get(command).map(PathBuf::from)
        }
        fn is_file(&self, path: &Path) -> bool {
            self.files.contains(path)
        }
        fn var(&self, name: &str) -> Option<OsString> {
            self.vars.get(name).map(OsString::from)
        }
        fn version(&self, executable: &Path) -> Option<String> {
            self.versions
                .get(executable.to_str()?)
                .map(|text| (*text).to_string())
        }
    }

    #[test]
    fn parses_every_name_and_auto() {
        assert_eq!(Choice::parse("auto"), Ok(Choice::Auto));
        for browser in ORDER {
            assert_eq!(Choice::parse(browser.name()), Ok(Choice::Named(browser)));
        }
    }

    #[test]
    fn an_unknown_name_lists_what_is_accepted() {
        let err = Choice::parse("opera").unwrap_err();
        assert!(err.contains("`opera` is not a browser"), "{err}");
        for browser in ORDER {
            assert!(err.contains(browser.name()), "{err}");
        }
        // Case is part of the name, as it is for every other flag value.
        assert!(Choice::parse("Chrome").is_err());
    }

    #[test]
    fn auto_prefers_chrome_when_it_can_be_driven() {
        let machine = Machine::default()
            .on_path("google-chrome", "/usr/bin/google-chrome")
            .on_path("chromedriver", "/usr/bin/chromedriver")
            .on_path("firefox", "/usr/bin/firefox");
        let selected = select(Choice::Auto, &machine).unwrap();
        assert_eq!(
            selected.launch,
            Launch {
                browser: Browser::Chrome,
                binary: "/usr/bin/google-chrome".into(),
                version: None,
                driver: Some("/usr/bin/chromedriver".into()),
            }
        );
        assert!(selected.skipped.is_empty());
    }

    #[test]
    fn auto_passes_over_chrome_without_its_driver_and_says_so() {
        let machine = Machine::default()
            .on_path("google-chrome", "/usr/bin/google-chrome")
            .on_path("firefox", "/usr/bin/firefox");
        let selected = select(Choice::Auto, &machine).unwrap();
        assert_eq!(selected.launch.browser, Browser::Firefox);
        // Firefox serves BiDi itself.
        assert_eq!(selected.launch.driver, None);
        let skipped: Vec<_> = selected.skipped.iter().map(|(b, _)| *b).collect();
        assert_eq!(skipped, [Browser::Chrome, Browser::Chromium]);
        assert!(selected.skipped[0].1.contains("chromedriver not found"));
        let described = selected.describe();
        assert!(
            described.starts_with("browser: firefox (/usr/bin/firefox)"),
            "{described}"
        );
        assert!(
            described.contains("skipped chrome: chromedriver not found"),
            "{described}"
        );
    }

    #[test]
    fn auto_reaches_edge_after_chrome_chromium_and_firefox() {
        let machine = Machine::default()
            .on_path("microsoft-edge", "/usr/bin/microsoft-edge")
            .on_path("msedgedriver", "/usr/bin/msedgedriver");
        let selected = select(Choice::Auto, &machine).unwrap();
        assert_eq!(selected.launch.browser, Browser::Edge);
        assert_eq!(
            selected.launch.driver,
            Some(PathBuf::from("/usr/bin/msedgedriver"))
        );
        let skipped: Vec<_> = selected.skipped.iter().map(|(b, _)| *b).collect();
        assert_eq!(
            skipped,
            [Browser::Chrome, Browser::Chromium, Browser::Firefox]
        );
    }

    #[test]
    fn chromium_is_its_own_browser() {
        let machine = Machine::default()
            .on_path("chromium", "/usr/bin/chromium")
            .on_path("chromedriver", "/usr/bin/chromedriver");
        // Chrome means Google Chrome, and a Chromium beside it is not one.
        assert!(
            find(Browser::Chrome, &machine)
                .unwrap_err()
                .starts_with("not installed")
        );
        let launch = find(Browser::Chromium, &machine).unwrap();
        assert_eq!(launch.binary, PathBuf::from("/usr/bin/chromium"));
        let selected = select(Choice::Auto, &machine).unwrap();
        assert_eq!(selected.launch.browser, Browser::Chromium);
        assert_eq!(selected.skipped[0].0, Browser::Chrome);
    }

    /// Google Chrome left behind at 120, and a Chromium upgraded to 131 with
    /// the distribution's chromedriver — what installing Debian's
    /// `chromium-driver` does to a machine that had both.
    fn chrome_behind_chromium() -> Machine {
        Machine::default()
            .on_path("google-chrome", "/usr/bin/google-chrome")
            .on_path("chromium", "/usr/bin/chromium")
            .on_path("chromedriver", "/usr/bin/chromedriver")
            .on_path("firefox", "/usr/bin/firefox")
            .says("/usr/bin/google-chrome", "Google Chrome 120.0.6099.71 \n")
            .says(
                "/usr/bin/chromium",
                "Chromium 131.0.6778.85 built on Debian GNU/Linux 12 (bookworm)\n",
            )
            .says(
                "/usr/bin/chromedriver",
                "ChromeDriver 131.0.6778.85 (0a1b2c3d-refs/branch-heads/6778@{#1})\n",
            )
    }

    #[test]
    fn auto_passes_over_a_browser_its_driver_does_not_match() {
        let selected = select(Choice::Auto, &chrome_behind_chromium()).unwrap();
        assert_eq!(selected.launch.browser, Browser::Chromium);
        assert_eq!(selected.launch.version, Some(131));
        let described = selected.describe();
        assert!(
            described.starts_with(
                "browser: chromium 131 (/usr/bin/chromium, driven by /usr/bin/chromedriver)"
            ),
            "{described}"
        );
        assert!(
            described.contains(
                "skipped chrome: chromedriver 131 (/usr/bin/chromedriver) does not match chrome 120"
            ),
            "{described}"
        );
    }

    #[test]
    fn a_named_browser_its_driver_does_not_match_is_refused() {
        let err = select(Choice::Named(Browser::Chrome), &chrome_behind_chromium()).unwrap_err();
        assert!(err.contains("cannot run the tests in chrome"), "{err}");
        assert!(err.contains("chromedriver 131"), "{err}");
        assert!(err.contains("does not match chrome 120"), "{err}");
    }

    #[test]
    fn a_missing_driver_names_the_version_to_install() {
        let machine = Machine::default()
            .on_path("google-chrome", "/usr/bin/google-chrome")
            .says("/usr/bin/google-chrome", "Google Chrome 120.0.6099.71");
        let err = find(Browser::Chrome, &machine).unwrap_err();
        assert!(
            err.contains("install the chromedriver 120 matching"),
            "{err}"
        );
    }

    #[test]
    fn a_version_either_side_will_not_state_is_left_to_the_driver() {
        let machine = Machine::default()
            .on_path("google-chrome", "/usr/bin/google-chrome")
            .on_path("chromedriver", "/usr/bin/chromedriver")
            .says("/usr/bin/chromedriver", "ChromeDriver 131.0.6778.85");
        let launch = find(Browser::Chrome, &machine).unwrap();
        assert_eq!(launch.version, None);
        assert!(launch.driver.is_some());
    }

    #[test]
    fn edge_is_matched_against_msedgedriver() {
        let machine = Machine::default()
            .on_path("microsoft-edge", "/usr/bin/microsoft-edge")
            .on_path("msedgedriver", "/usr/bin/msedgedriver")
            .says("/usr/bin/microsoft-edge", "Microsoft Edge 140.0.3485.54 ")
            .says(
                "/usr/bin/msedgedriver",
                "Microsoft Edge WebDriver 139.0.3405.86 (5e2d0a7…)",
            );
        let err = find(Browser::Edge, &machine).unwrap_err();
        assert!(err.contains("msedgedriver 139"), "{err}");
        assert!(err.contains("does not match edge 140"), "{err}");
    }

    #[test]
    fn major_reads_the_first_dotted_version() {
        assert_eq!(major("Google Chrome 120.0.6099.71 "), Some(120));
        assert_eq!(
            major("ChromeDriver 131.0.6778.85 (abc-refs/x@{#1})"),
            Some(131)
        );
        assert_eq!(
            major("Chromium 120.0.6099.71 built on Debian GNU/Linux 12"),
            Some(120)
        );
        assert_eq!(major("Mozilla Firefox 128.0"), Some(128));
        // A Windows version directory is the version and nothing else.
        assert_eq!(major("120.0.6099.71"), Some(120));
        assert_eq!(major("GNU/Linux 13 (trixie)"), None);
        assert_eq!(major(""), None);
    }

    #[test]
    fn an_installed_browser_off_path_is_found_where_its_installer_puts_it() {
        let machine = Machine::default()
            .var("PROGRAMFILES", "C:/Program Files")
            .file("C:/Program Files/Mozilla Firefox/firefox.exe");
        let launch = find(Browser::Firefox, &machine).unwrap();
        assert_eq!(
            launch.binary,
            PathBuf::from("C:/Program Files/Mozilla Firefox/firefox.exe")
        );
        let mac = Machine::default().file("/Applications/Firefox.app/Contents/MacOS/firefox");
        assert!(find(Browser::Firefox, &mac).is_ok());
    }

    #[test]
    fn a_driver_without_its_browser_is_not_enough() {
        let machine = Machine::default().on_path("chromedriver", "/usr/bin/chromedriver");
        let err = find(Browser::Chrome, &machine).unwrap_err();
        assert!(err.starts_with("not installed"), "{err}");
    }

    #[test]
    fn safari_is_never_available_yet() {
        let err = select(Choice::Named(Browser::Safari), &Machine::default()).unwrap_err();
        assert!(err.contains("cannot run the tests in safari"), "{err}");
        assert!(err.contains("not ready"), "{err}");
    }

    #[test]
    fn a_named_browser_never_falls_back() {
        let machine = Machine::default()
            .on_path("google-chrome", "/usr/bin/google-chrome")
            .on_path("firefox", "/usr/bin/firefox");
        let err = select(Choice::Named(Browser::Chrome), &machine).unwrap_err();
        assert!(err.contains("cannot run the tests in chrome"), "{err}");
        assert!(err.contains("chromedriver not found on PATH"), "{err}");
    }

    #[test]
    fn nothing_available_names_every_reason_and_downloads_nothing() {
        let err = select(Choice::Auto, &Machine::default()).unwrap_err();
        for browser in ORDER {
            assert!(err.contains(&format!("\n  {browser}: ")), "{err}");
        }
        assert!(
            err.contains("downloads neither browsers nor drivers"),
            "{err}"
        );
    }
}
