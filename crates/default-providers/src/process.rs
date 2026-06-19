//! OS-backed [`Process`] — the real environment, working directory, and
//! platform, plus an exit-code cell. The standalone embedding's host process
//! view (DECISIONS D24).
//!
//! `args` are **supplied by the embedder** (the CLI knows which argv entries are
//! the user's, after the binary and the script/`-e` code); everything else is
//! read from the OS. `env` is snapshotted on each [`Process::env`] call.
//!
//! An optional **env overlay** (e.g. parsed from `--env-file`) can be layered
//! over the host environment. By default the host environment wins (the overlay
//! only fills keys the OS doesn't set); with the override flag the overlay wins
//! instead. The overlay never mutates the real process environment.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use es_runtime_providers::{Process, ProviderError};

/// A [`Process`] reading the host environment/cwd/platform, with caller-provided
/// program arguments, an optional env overlay, and a recorded exit code.
pub struct SystemProcess {
    args: Vec<String>,
    /// Extra environment entries layered over the OS environment (in order;
    /// later entries win within the overlay). Empty unless [`with_env`] is used.
    ///
    /// [`with_env`]: SystemProcess::with_env
    env_overlay: Vec<(String, String)>,
    /// When `true`, overlay entries override OS environment variables of the
    /// same name; when `false` (default), the OS value wins.
    env_override: bool,
    exit: Arc<ExitCell>,
}

#[derive(Default)]
struct ExitCell {
    requested: AtomicBool,
    code: AtomicI32,
}

impl SystemProcess {
    /// Builds a process view exposing `args` as the program arguments and the
    /// real OS environment/cwd/platform.
    pub fn new(args: Vec<String>) -> Self {
        SystemProcess {
            args,
            env_overlay: Vec::new(),
            env_override: false,
            exit: Arc::new(ExitCell::default()),
        }
    }

    /// Layers `overlay` (e.g. parsed from `--env-file`) over the OS environment.
    /// With `override_os = false` the OS value wins on a conflict; with
    /// `override_os = true` the overlay wins. Within `overlay`, later entries
    /// win (so a later duplicate key in the file overrides an earlier one). The
    /// real process environment is never modified.
    pub fn with_env(mut self, overlay: Vec<(String, String)>, override_os: bool) -> Self {
        self.env_overlay = overlay;
        self.env_override = override_os;
        self
    }
}

impl Process for SystemProcess {
    fn env(&self) -> Vec<(String, String)> {
        // Fold the overlay first so later entries win within it (later
        // --env-file overrides earlier), then merge with the OS environment per
        // the override flag. BTreeMap keeps the output deterministic.
        let mut overlay: BTreeMap<&str, &str> = BTreeMap::new();
        for (k, v) in &self.env_overlay {
            overlay.insert(k, v);
        }
        let mut map: BTreeMap<String, String> = std::env::vars().collect();
        for (k, v) in overlay {
            if self.env_override {
                map.insert(k.to_string(), v.to_string());
            } else {
                map.entry(k.to_string()).or_insert_with(|| v.to_string());
            }
        }
        map.into_iter().collect()
    }

    fn args(&self) -> Vec<String> {
        self.args.clone()
    }

    fn cwd(&self) -> Result<String, ProviderError> {
        let dir = std::env::current_dir()
            .map_err(|e| ProviderError::Other(format!("cannot read working directory: {e}")))?;
        Ok(dir.to_string_lossy().into_owned())
    }

    fn platform(&self) -> String {
        std::env::consts::OS.to_string()
    }

    fn arch(&self) -> String {
        std::env::consts::ARCH.to_string()
    }

    fn exit(&self, code: i32) {
        self.exit.code.store(code, Ordering::SeqCst);
        self.exit.requested.store(true, Ordering::SeqCst);
    }

    fn requested_exit_code(&self) -> Option<i32> {
        self.exit
            .requested
            .load(Ordering::SeqCst)
            .then(|| self.exit.code.load(Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_args_and_records_exit() {
        let p = SystemProcess::new(vec!["a".into(), "b".into()]);
        assert_eq!(p.args(), ["a", "b"]);
        assert!(!p.platform().is_empty());
        assert!(!p.arch().is_empty());
        assert!(p.cwd().is_ok());
        assert_eq!(p.requested_exit_code(), None);
        p.exit(3);
        assert_eq!(p.requested_exit_code(), Some(3));
    }

    fn get<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
        env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    #[test]
    fn overlay_adds_new_keys() {
        // A key the OS does not set is contributed by the overlay regardless of
        // the override flag. Use a name unlikely to exist in the test env.
        let key = "ESRUN_TEST_OVERLAY_ONLY_KEY";
        let p = SystemProcess::new(vec![]).with_env(vec![(key.into(), "v1".into())], false);
        assert_eq!(get(&p.env(), key), Some("v1"));
    }

    // OS-vs-overlay precedence (default OS-wins; --env-override flips it) is
    // verified end-to-end in `tests/env.rs`, where the OS environment can be set
    // per-process via `Command::env` without the (forbidden) `unsafe` set_var.

    #[test]
    fn later_overlay_entry_wins_within_the_overlay() {
        let key = "ESRUN_TEST_LATER_WINS_KEY";
        let p = SystemProcess::new(vec![]).with_env(
            vec![(key.into(), "first".into()), (key.into(), "second".into())],
            false,
        );
        assert_eq!(get(&p.env(), key), Some("second"));
    }
}
