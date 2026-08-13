//! The argument grammar every ES-Runtime binary shares, and the run options
//! those shared flags accumulate into.
//!
//! **The single grammar rule: a value attaches with `=`, never as the next
//! argument.** Each binary owns its own subcommands and its own usage text, but
//! routes the flags below through here so that `--allow-net=…` or `--max-heap=…`
//! means exactly one thing whichever binary reads it.

use std::time::Duration;

use crate::permissions::Permissions;

/// How long a graceful shutdown waits for in-flight HTTP requests before giving
/// up and exiting anyway. Long enough for an ordinary request to finish, short
/// enough that an orchestrator's own kill deadline (commonly 30s) is not the
/// thing that ends the process.
pub const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// The flags that shape a run rather than choose what to run — shared by every
/// binary.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Stop execution after this long (watchdog, SPEC §4), via `--timeout`.
    pub timeout: Option<Duration>,
    /// `.env` file to load, via `--env-file` (last one wins if repeated).
    pub env_file: Option<String>,
    /// Import policy file, via `--import-policy` (D39). Never auto-discovered:
    /// like `--env-file`, nothing on disk is read unless it is named.
    pub import_policy: Option<String>,
    /// Whether `--env-file` values override the OS environment (`--env-override`).
    pub env_override: bool,
    /// How long in-flight HTTP requests get to finish after an interrupt, via
    /// `--shutdown-grace` (see [`DEFAULT_SHUTDOWN_GRACE`]).
    pub shutdown_grace: Duration,
    /// The heap ceiling in bytes, via `--max-heap=<mb>`; `None` sizes it from
    /// the host.
    pub max_heap_bytes: Option<usize>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            timeout: None,
            env_file: None,
            import_policy: None,
            env_override: false,
            shutdown_grace: DEFAULT_SHUTDOWN_GRACE,
            max_heap_bytes: None,
        }
    }
}

impl RunOptions {
    /// Handles one shared flag, reporting whether it was consumed.
    ///
    /// A caller's parse loop offers each argument here first and falls through
    /// to its own flags when this returns `false` — so a binary can add options
    /// without re-implementing (or drifting from) the common ones.
    pub fn try_flag(&mut self, flag: &str, value: Option<&str>) -> Result<bool, String> {
        match flag {
            "-t" | "--timeout" => {
                let ms = require_value(flag, value)?;
                let ms: u64 = ms
                    .parse()
                    .map_err(|_| format!("invalid {flag} value: {ms} (expected milliseconds)"))?;
                self.timeout = Some(Duration::from_millis(ms));
            }
            "--env-file" => {
                self.env_file = Some(require_value(flag, value)?.to_string());
            }
            "--import-policy" => {
                self.import_policy = Some(require_value(flag, value)?.to_string());
            }
            "--env-override" => {
                reject_value(flag, value)?;
                self.env_override = true;
            }
            "--max-heap" => {
                let mb = require_value(flag, value)?;
                let mb: usize = mb.parse().map_err(|_| {
                    format!("invalid {flag} value: {mb} (expected whole megabytes)")
                })?;
                if mb == 0 {
                    return Err(format!("{flag}=0 would leave no heap at all"));
                }
                self.max_heap_bytes = Some(mb * 1024 * 1024);
            }
            "--shutdown-grace" => {
                let ms = require_value(flag, value)?;
                let ms: u64 = ms
                    .parse()
                    .map_err(|_| format!("invalid {flag} value: {ms} (expected milliseconds)"))?;
                self.shutdown_grace = Duration::from_millis(ms);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Whether `flag` is one [`RunOptions::try_flag`] would consume. Used to
    /// reject a shared flag written *after* the script, where it would silently
    /// be the script's own argument.
    pub fn is_shared_flag(flag: &str) -> bool {
        matches!(
            flag,
            "-t" | "--timeout"
                | "--env-file"
                | "--import-policy"
                | "--env-override"
                | "--max-heap"
                | "--shutdown-grace"
        )
    }
}

/// Handles the permission flags — `--allow-all`, `--deny-all`,
/// `--allow-<name>`, `--deny-<name>` — reporting whether `flag` was one of them.
///
/// Both `--all` flags are accepted by every binary, whichever way that binary's
/// baseline points (D65). One of the two always restates the default and does
/// nothing, and that is the point: a deploy line that says `--deny-all` outright,
/// or a dev line that says `--allow-all`, is stating the grant it expects rather
/// than trusting the reader to know which binary defaults which way.
pub fn try_permission_flag(
    permissions: &mut Permissions,
    flag: &str,
    value: Option<&str>,
) -> Result<bool, String> {
    match flag {
        "--deny-all" => {
            reject_value(flag, value)?;
            permissions.deny_all();
            Ok(true)
        }
        "--allow-all" | "-A" => {
            reject_value(flag, value)?;
            permissions.allow_all();
            Ok(true)
        }
        flag if flag.starts_with("--deny-") || flag.starts_with("--allow-") => {
            let allow = flag.starts_with("--allow-");
            let prefix = if allow { "--allow-" } else { "--deny-" };
            let name = flag[prefix.len()..].to_string();
            permissions.record(flag, &name, allow, value)?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// Splits `--flag=value` into its parts. A flag with no `=` yields `None`, which
/// is distinct from `--flag=` (an empty value) — the latter is a mistake worth
/// naming rather than treating as absent.
pub fn split_flag_value(arg: &str) -> (&str, Option<&str>) {
    match arg.split_once('=') {
        Some((flag, value)) => (flag, Some(value)),
        None => (arg, None),
    }
}

/// The value of a flag that requires one.
///
/// **The single grammar rule of this parser: a value attaches with `=`, never as
/// the next argument.** `--timeout 500` is rejected, not read.
///
/// One rule for every flag is the whole point. Two — a space form here, an `=`
/// form there — is how `--allow-net example.com app.js` silently runs
/// `example.com` as the script and hands `app.js` to it as an argument. With one
/// rule the parser never has to guess whether the next word belongs to the flag
/// or is the script, so there is nothing to guess wrong.
pub fn require_value<'a>(flag: &str, value: Option<&'a str>) -> Result<&'a str, String> {
    match value {
        Some(value) if !value.is_empty() => Ok(value),
        Some(_) => Err(format!("{flag}= has an empty value — use `{flag}=<value>`")),
        None => Err(format!(
            "{flag} requires a value, attached with '=': use `{flag}=<value>`.\n\n\
             A value is never a separate word: `{flag} <value>` would leave <value> \
             to be mistaken for the script to run."
        )),
    }
}

/// Rejects a value on a flag that takes none.
pub fn reject_value(flag: &str, value: Option<&str>) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    // Permission flags never reach here: `Permissions::record` owns their
    // values, since for them a value is sometimes a scope list and otherwise an
    // error that has to explain itself. `--deny-all` does — it is a mode switch
    // rather than a capability, so scoping could never apply to it.
    Err(format!("{flag} takes no value (got {flag}={value})"))
}
