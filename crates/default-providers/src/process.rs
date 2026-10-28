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

use es_runtime_providers::{BoxFuture, Process, ProviderError, StdStream};

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
    /// Values for variables that neither the OS environment nor the overlay
    /// sets, such as the `RUNTIME_ENV` esdev gives the programs it runs (D192).
    /// Empty unless [`with_env_defaults`] is used.
    ///
    /// [`with_env_defaults`]: SystemProcess::with_env_defaults
    env_defaults: Vec<(String, String)>,
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
            env_defaults: Vec::new(),
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

    /// Fills in each of `defaults` that neither the OS environment nor the
    /// overlay sets (or sets empty). Anything already there wins, so a shell or
    /// an `--env-file` always decides; the allowlist still applies afterwards.
    #[must_use]
    pub fn with_env_defaults(mut self, defaults: Vec<(String, String)>) -> Self {
        self.env_defaults = defaults;
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
        merge_env(
            std::env::vars(),
            &self.env_overlay,
            self.env_override,
            &self.env_defaults,
            self.env_allow.as_ref(),
            cfg!(windows),
        )
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

    fn hardware_concurrency(&self) -> u32 {
        // `available_parallelism` respects cgroup/affinity limits, so a
        // container sees its share rather than the whole machine — which is the
        // number a pool should be sized from.
        std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(1)
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

    /// The real file descriptor, flushed on every call.
    ///
    /// Flushed because the caller is drawing: a progress line that sits in a
    /// buffer until the next newline is a progress line that appears once, at
    /// the end. `stdout()` is line-buffered when it is a terminal and block-
    /// buffered otherwise, and neither is what a `\r` wants.
    fn write_stdout(&self, bytes: &[u8]) -> Result<(), ProviderError> {
        write_to(&mut std::io::stdout().lock(), bytes, "standard output")
    }

    fn write_stderr(&self, bytes: &[u8]) -> Result<(), ProviderError> {
        write_to(&mut std::io::stderr().lock(), bytes, "standard error")
    }

    fn is_terminal(&self, stream: StdStream) -> bool {
        use std::io::IsTerminal;
        match stream {
            StdStream::In => std::io::stdin().is_terminal(),
            StdStream::Out => std::io::stdout().is_terminal(),
            StdStream::Err => std::io::stderr().is_terminal(),
        }
    }

    /// The window size, asked of the terminal itself rather than of `$COLUMNS`
    /// — which a shell exports to itself and not to a child, and which is stale
    /// the moment the window is dragged.
    ///
    /// Unix only. On a platform without `TIOCGWINSZ` here the answer is "no
    /// answer", and a caller wraps at whatever it likes; a made-up 80 would be
    /// a width that is wrong rather than absent.
    #[cfg(unix)]
    fn terminal_size(&self) -> Option<(u16, u16)> {
        // stderr as well as stdout: a program whose output is piped into
        // something else still draws its progress on the terminal it kept.
        for stream in [
            &std::io::stdout() as &dyn std::os::fd::AsFd,
            &std::io::stderr(),
        ] {
            if let Ok(size) = rustix::termios::tcgetwinsize(stream)
                && size.ws_col > 0
            {
                return Some((size.ws_col, size.ws_row));
            }
        }
        None
    }

    /// The real standard input, through the one reader every caller shares
    /// (D143, [`crate::stdin`]).
    fn read_stdin(&self, line: bool) -> BoxFuture<Result<Option<Vec<u8>>, ProviderError>> {
        Box::pin(async move { crate::stdin::read(line).await.map_err(ProviderError::Other) })
    }

    fn read_stdin_line_blocking(&self) -> Result<Option<Vec<u8>>, ProviderError> {
        crate::stdin::read_line_blocking().map_err(ProviderError::Other)
    }

    fn set_stdin_raw(&self, raw: bool) -> Result<(), ProviderError> {
        crate::stdin::set_raw(raw).map_err(ProviderError::Other)
    }
}

/// The environment a program sees: the OS's, the `--env-file` overlay, the
/// defaults, then the allowlist — with names compared as `windows` says the
/// platform compares them (D195). A pure function of its inputs, so the
/// Windows rule is tested on every platform.
fn merge_env(
    os: impl IntoIterator<Item = (String, String)>,
    overlay: &[(String, String)],
    overlay_wins: bool,
    defaults: &[(String, String)],
    allow: Option<&BTreeSet<String>>,
    windows: bool,
) -> Vec<(String, String)> {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in os {
        put(&mut map, &k, v, windows);
    }
    // Folded first so a later entry wins within it (a later --env-file over an
    // earlier one), including one that differs only in case on Windows.
    let mut folded: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in overlay {
        put(&mut folded, k, v.clone(), windows);
    }
    for (k, v) in folded {
        match spelling(&map, &k, windows) {
            Some(name) if overlay_wins => {
                map.insert(name, v);
            }
            Some(_) => {}
            None => {
                map.insert(k, v);
            }
        }
    }
    // An empty value counts as unset (D192): container templates write
    // `RUNTIME_ENV=` as often as they leave it out.
    for (k, v) in defaults {
        let name = spelling(&map, k, windows).unwrap_or_else(|| k.clone());
        if map.get(&name).is_none_or(String::is_empty) {
            map.insert(name, v.clone());
        }
    }
    // Last, so neither the OS environment nor an --env-file overlay can
    // reintroduce a name the allowlist leaves out.
    if let Some(allow) = allow {
        map.retain(|name, _| allow.iter().any(|a| same_name(a, name, windows)));
    }
    map.into_iter().collect()
}

/// Whether two environment variable names are the same variable (D195):
/// exactly on Unix, ignoring case on Windows, where the OS spells `PATH` as
/// `Path` and treats both as one. Case is compared by Unicode uppercase, as
/// `runtime:process` and `runtime:system` compare it.
fn same_name(a: &str, b: &str, windows: bool) -> bool {
    a == b || (windows && a.to_uppercase() == b.to_uppercase())
}

/// The spelling `map` already holds for `name`, if it holds the variable.
fn spelling(map: &BTreeMap<String, String>, name: &str, windows: bool) -> Option<String> {
    if map.contains_key(name) {
        return Some(name.to_string());
    }
    map.keys().find(|k| same_name(k, name, windows)).cloned()
}

/// Sets `name` in `map`, replacing the variable it names under the spelling
/// that variable already has.
fn put(map: &mut BTreeMap<String, String>, name: &str, value: String, windows: bool) {
    let name = spelling(map, name, windows).unwrap_or_else(|| name.to_string());
    map.insert(name, value);
}

/// One write, all of it, then flushed. A short write is retried by
/// `write_all`; anything left is a real failure and is reported as one rather
/// than silently truncating what the program asked to print.
fn write_to(out: &mut impl std::io::Write, bytes: &[u8], what: &str) -> Result<(), ProviderError> {
    out.write_all(bytes)
        .and_then(|()| out.flush())
        .map_err(|e| ProviderError::Other(format!("cannot write to {what}: {e}")))
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

    #[test]
    fn a_default_fills_only_what_nothing_else_sets() {
        // Unset anywhere: the default appears. Set by the overlay: the overlay
        // wins. Set empty: empty counts as unset (D192), so the default wins.
        // Set by the OS: `PATH` is in every test environment, and stays.
        let unset = "ESRUN_TEST_DEFAULT_UNSET_KEY";
        let overlaid = "ESRUN_TEST_DEFAULT_OVERLAID_KEY";
        let empty = "ESRUN_TEST_DEFAULT_EMPTY_KEY";
        let p = SystemProcess::new(vec![])
            .with_env(
                vec![
                    (overlaid.into(), "file".into()),
                    (empty.into(), String::new()),
                ],
                false,
            )
            .with_env_defaults(vec![
                (unset.into(), "default".into()),
                (overlaid.into(), "default".into()),
                (empty.into(), "default".into()),
                ("PATH".into(), "default".into()),
            ]);
        let env = p.env();
        assert_eq!(get(&env, unset), Some("default"));
        assert_eq!(get(&env, overlaid), Some("file"));
        assert_eq!(get(&env, empty), Some("default"));
        // Windows spells it `Path`; the default must not add a second `PATH`.
        let path: Vec<_> = env
            .iter()
            .filter(|(k, _)| same_name(k, "PATH", cfg!(windows)))
            .collect();
        assert_eq!(path.len(), 1, "one PATH variable, not two: {path:?}");
        assert_ne!(path[0].1, "default");
    }

    #[test]
    fn env_names_compare_as_the_platform_does() {
        assert!(same_name("PATH", "PATH", false));
        assert!(!same_name("Path", "PATH", false));
        assert!(same_name("Path", "PATH", true));
        assert!(same_name("straße", "STRASSE", true), "Unicode uppercase");
        assert!(!same_name("PATHS", "PATH", true));
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// The Windows rule (D195), run on every platform: an overlay, a default
    /// and the allowlist each find `Path` when they name `PATH`, and keep the
    /// OS's spelling.
    #[test]
    fn on_windows_a_name_finds_its_variable_whatever_its_case() {
        let os = pairs(&[("Path", "C:\\bin"), ("TEMP", "C:\\t")]);
        let allow: BTreeSet<String> = ["PATH".to_string(), "temp".to_string()].into();

        let kept = merge_env(os.clone(), &pairs(&[("PATH", "x")]), false, &[], None, true);
        assert_eq!(kept, pairs(&[("Path", "C:\\bin"), ("TEMP", "C:\\t")]));

        let replaced = merge_env(os.clone(), &pairs(&[("PATH", "x")]), true, &[], None, true);
        assert_eq!(replaced, pairs(&[("Path", "x"), ("TEMP", "C:\\t")]));

        let defaulted = merge_env(os.clone(), &[], false, &pairs(&[("PATH", "d")]), None, true);
        assert_eq!(defaulted, pairs(&[("Path", "C:\\bin"), ("TEMP", "C:\\t")]));

        let granted = merge_env(os.clone(), &[], false, &[], Some(&allow), true);
        assert_eq!(granted, pairs(&[("Path", "C:\\bin"), ("TEMP", "C:\\t")]));

        // Within the overlay, a later entry wins over one differing in case.
        let later = merge_env(
            vec![],
            &pairs(&[("Mode", "a"), ("MODE", "b")]),
            false,
            &[],
            None,
            true,
        );
        assert_eq!(later, pairs(&[("Mode", "b")]));
    }

    /// The same inputs on Unix: names are exact, so each is its own variable.
    #[test]
    fn elsewhere_names_differing_in_case_are_different_variables() {
        let os = pairs(&[("Path", "/bin")]);
        let allow: BTreeSet<String> = ["PATH".to_string()].into();
        let merged = merge_env(
            os.clone(),
            &pairs(&[("PATH", "x")]),
            false,
            &[],
            None,
            false,
        );
        assert_eq!(merged, pairs(&[("PATH", "x"), ("Path", "/bin")]));
        assert!(merge_env(os, &[], false, &[], Some(&allow), false).is_empty());
        let later = merge_env(
            vec![],
            &pairs(&[("Mode", "a"), ("MODE", "b")]),
            false,
            &[],
            None,
            false,
        );
        assert_eq!(later, pairs(&[("MODE", "b"), ("Mode", "a")]));
    }

    #[test]
    fn the_allowlist_applies_to_defaults_too() {
        let key = "ESRUN_TEST_DEFAULT_HIDDEN_KEY";
        let p = SystemProcess::new(vec![])
            .with_env_defaults(vec![(key.into(), "default".into())])
            .with_env_allowlist(["PATH"]);
        assert_eq!(get(&p.env(), key), None);
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

    /// A grant names a whole variable: a prefix is not it. Case follows the
    /// platform (D195), which the `merge_env` tests above cover for both.
    #[test]
    fn allowlist_matches_whole_names_only() {
        let key = "ESRUN_TEST_EXACT_KEY";
        let p = SystemProcess::new(vec![])
            .with_env(vec![(key.into(), "v".into())], false)
            .with_env_allowlist(["ESRUN_TEST_EXACT", "ESRUN_TEST_EXACT_KEY_2"]);
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
