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
//!
//! An optional **allowlist** narrows the snapshot to named variables — the
//! provider-side half of `esrun --allow-env=<names>` (D38). It is applied last,
//! after the overlay is merged, so a `--env-file` value is no way around it.

use std::collections::{BTreeMap, BTreeSet};
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
    /// Variables [`Process::env`] may report. `None` ⇒ all of them. Set by
    /// [`with_env_allowlist`], which backs `esrun --allow-env=<names>`.
    ///
    /// [`with_env_allowlist`]: SystemProcess::with_env_allowlist
    env_allow: Option<BTreeSet<String>>,
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
            env_allow: None,
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

    /// Restricts [`Process::env`] to `names` — the policy seam behind
    /// `esrun --allow-env=<names>` (D38), and the shape an embedder wants when
    /// a guest needs two variables rather than the host's whole environment.
    ///
    /// Unlisted variables are **absent** from the snapshot rather than present
    /// and unreadable: `env` hands out a map, so the honest way to say "you may
    /// not have this" is not to include it. Enumeration of names the guest may
    /// not read is denied along with the values, which is the point — the
    /// variable names alone (`AWS_SECRET_ACCESS_KEY`, `DATABASE_URL`) tell an
    /// attacker where the host keeps its secrets.
    ///
    /// Matching is exact and case-sensitive, as environment lookups are on unix.
    /// This narrows `env` only: `args`, `cwd`, and the rest of the provider are
    /// not environment variables and stay whole under
    /// [`Capability::Env`](es_runtime_common::Capability::Env).
    #[must_use]
    pub fn with_env_allowlist<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.env_allow = Some(names.into_iter().map(Into::into).collect());
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
        // Last, so neither the OS environment nor an --env-file overlay can
        // reintroduce a name the allowlist leaves out.
        if let Some(allow) = &self.env_allow {
            map.retain(|name, _| allow.contains(name));
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

    #[test]
    fn allowlist_keeps_only_the_named_variables() {
        let kept = "ESRUN_TEST_ALLOWED_KEY";
        let dropped = "ESRUN_TEST_DENIED_KEY";
        let p = SystemProcess::new(vec![])
            .with_env(
                vec![(kept.into(), "yes".into()), (dropped.into(), "no".into())],
                false,
            )
            .with_env_allowlist([kept]);
        let env = p.env();
        assert_eq!(get(&env, kept), Some("yes"));
        assert_eq!(get(&env, dropped), None);
        // The host's own environment is narrowed to the same list, so the guest
        // cannot enumerate what it may not read.
        assert_eq!(env.len(), 1);
    }

    #[test]
    fn an_empty_allowlist_hides_the_whole_environment() {
        // `--allow-env=` never reaches here (the CLI rejects an empty list), but
        // an embedder may legitimately grant the capability and name nothing.
        let p = SystemProcess::new(vec![])
            .with_env(vec![("ESRUN_TEST_EMPTY_LIST".into(), "v".into())], false)
            .with_env_allowlist(Vec::<String>::new());
        assert!(p.env().is_empty());
    }

    #[test]
    fn allowlist_matching_is_exact() {
        let key = "ESRUN_TEST_EXACT_KEY";
        let p = SystemProcess::new(vec![])
            .with_env(vec![(key.into(), "v".into())], false)
            .with_env_allowlist(["esrun_test_exact_key", "ESRUN_TEST_EXACT"]);
        assert_eq!(get(&p.env(), key), None);
    }

    #[test]
    fn allowlist_outranks_an_env_file_overlay() {
        // The overlay is merged first; the allowlist is applied after, so
        // --env-file is not a way around --allow-env.
        let key = "ESRUN_TEST_OVERLAY_VS_ALLOWLIST";
        let p = SystemProcess::new(vec![])
            .with_env(vec![(key.into(), "v".into())], true)
            .with_env_allowlist(["SOMETHING_ELSE"]);
        assert_eq!(get(&p.env(), key), None);
    }
}
