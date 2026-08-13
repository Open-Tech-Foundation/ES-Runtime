//! The `--deny-*` / `--allow-*` flag grammar (DECISIONS D38, D65), shared by
//! every binary on this runtime so that one command line means one thing
//! everywhere.

use std::collections::HashMap;

use es_runtime_common::{Capability, CapabilitySet};
use es_runtime_default_providers::{HostAllowlist, PathAllowlist};
use es_runtime_providers::Signal;

/// Scope lists by capability, in the order the user wrote them.
pub type Scopes = HashMap<Capability, Vec<String>>;

/// What a scoped `--allow-<name>=<list>` means for `cap`, or `None` if that
/// capability cannot enforce a list.
///
/// Seven of the eight take a list. `imports` deliberately does **not**: what
/// may be loaded is an [import policy](es_runtime_default_providers::ImportPolicy)
/// (`--import-policy=<file>`, D39), not a capability scope — the capability
/// decides whether the loader runs, the policy decides what it may resolve. The
/// `None` arm is the rule, not a placeholder: a capability rejects a value until
/// something enforces it (D38 — a run must never be narrower on the command line
/// than it is in reality).
pub fn scope_hint(cap: Capability) -> Option<&'static str> {
    match cap {
        Capability::Run => Some("program names, e.g. --allow-run=git,ls"),
        Capability::Env => Some("variable names, e.g. --allow-env=HOME,PATH"),
        Capability::Net => Some("hosts, e.g. --allow-net=api.example.com,db.internal:5432"),
        Capability::NetListen => Some("bind addresses, e.g. --allow-listen=127.0.0.1:8080,8443"),
        // One inside the project and one outside it: a path inside narrows the
        // root jail, and a path outside adds that subtree (D54), which is the
        // only way a run reaches a TLS certificate or a CA bundle.
        Capability::FileRead | Capability::FileWrite => {
            Some("paths, e.g. --allow-read=./data,/etc/ssl/certs")
        }
        Capability::Signals => Some("signal names, e.g. --allow-signals=SIGTERM,SIGINT"),
        _ => None,
    }
}

/// The signal list for `--allow-signals`, or `None` if `signals` was granted
/// whole. Names were validated when the flag was parsed.
pub fn signal_scope(scopes: &Scopes) -> Option<Vec<Signal>> {
    scopes
        .get(&Capability::Signals)
        .map(|names| names.iter().filter_map(|n| Signal::from_name(n)).collect())
}

/// The path list for `cap`, resolved against `base` — the working directory the
/// user typed the flags in, which is what a relative `./data` means to them.
/// `None` if that capability was granted whole.
pub fn path_scope(
    scopes: &Scopes,
    cap: Capability,
    base: &std::path::Path,
) -> Result<Option<PathAllowlist>, String> {
    scopes
        .get(&cap)
        .map(|entries| PathAllowlist::parse(entries, base))
        .transpose()
}

/// The address list for `cap`, parsed and validated, or `None` if that
/// capability was granted whole.
///
/// Parsed twice by design — once here at wiring time, once in
/// [`Permissions::record`] so a malformed entry is an *argument* error reported
/// before anything runs rather than a provider error three steps later. The
/// list is a handful of strings; the duplication buys the better message.
pub fn address_scope(scopes: &Scopes, cap: Capability) -> Result<Option<HostAllowlist>, String> {
    scopes.get(&cap).map(HostAllowlist::parse).transpose()
}

/// What a binary grants before a single permission flag is read (D65).
///
/// The only thing that differs between `esrun` and `esdev`. Everything below
/// this line — the vocabulary, the rules, the errors — is identical for both,
/// because the baseline is not a fourth rule: it only decides which of the two
/// modes a command line is in when neither `--allow-all` nor `--deny-all` says
/// so outright.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Baseline {
    /// Nothing host-facing — `esrun`. A deployment states what it may reach, or
    /// it reaches nothing.
    Nothing,
    /// Everything — `esdev`. The inner loop should not need flags to run the
    /// program a developer is in the middle of writing.
    Everything,
}

/// The permission flags accumulated while parsing, resolved into a
/// [`CapabilitySet`] once the whole command line has been seen (D38, D65).
///
/// A command line is in exactly one of two **modes**, and the mode decides which
/// direction flags may move in:
///
/// - **Restrictive** — start from nothing; `--allow-<name>` adds. Entered by
///   `--deny-all`, or by [`Baseline::Nothing`] with neither `--all` flag given.
/// - **Permissive** — start from everything; `--deny-<name>` subtracts. Entered
///   by `--allow-all`, or by [`Baseline::Everything`] likewise.
///
/// Three rules follow, and they exist so that **no flag ever overrides
/// another** — a reader goes top to bottom and the list is the answer:
///
/// 1. `--allow-all` and `--deny-all` are mutually exclusive: one grants
///    everything, the other nothing, and there is no reading that combines them.
/// 2. A flag may only move in its mode's direction. `--deny-<name>` in
///    restrictive mode subtracts from nothing, and `--allow-<name>` in permissive
///    mode adds to everything; each is a no-op or a contradiction of its own
///    sibling, so each is an **error** naming the `--all` flag that would make it
///    mean something.
/// 3. Each mode therefore has exactly one direction, whichever baseline the
///    binary started from.
pub struct Permissions {
    /// What this binary grants with no flags at all.
    baseline: Baseline,
    /// The `--deny-all` flag, if given.
    deny_all: bool,
    /// The `--allow-all` / `-A` flag, if given.
    allow_all: bool,
    /// `--deny-<name>` flags, in the order given — so an error can quote the one
    /// the user actually typed.
    denied: Vec<(String, Capability)>,
    /// `--allow-<name>` flags, likewise.
    allowed: Vec<Allow>,
}

/// One `--allow-<name>` flag as written.
struct Allow {
    /// The whole argument as the user typed it, scope list included, so an
    /// error can quote it back — `--allow-env` and `--allow-env=HOME` are
    /// different flags to a reader, and the message that tells them apart is
    /// the one complaining that they conflict.
    flag: String,
    cap: Capability,
    /// The entries of `--allow-<name>=a,b`, or `None` for the bare flag —
    /// "granted, unnarrowed".
    values: Option<Vec<String>>,
}

impl Permissions {
    /// An empty flag set for a binary that grants `baseline` on its own.
    pub fn new(baseline: Baseline) -> Self {
        Self {
            baseline,
            deny_all: false,
            allow_all: false,
            denied: Vec::new(),
            allowed: Vec::new(),
        }
    }

    /// Records `--deny-all`.
    pub fn deny_all(&mut self) {
        self.deny_all = true;
    }

    /// Records `--allow-all` / `-A`.
    pub fn allow_all(&mut self) {
        self.allow_all = true;
    }

    /// Whether this command line starts from everything rather than nothing —
    /// set outright by an `--all` flag, and otherwise by the binary's baseline.
    fn permissive(&self) -> bool {
        match (self.allow_all, self.deny_all) {
            (true, _) => true,
            (_, true) => false,
            _ => self.baseline == Baseline::Everything,
        }
    }

    /// Whether `flag` is a `--deny-<name>` / `--allow-<name>` this grammar owns,
    /// so a caller's parse loop can route it here without knowing the eight
    /// names itself.
    pub fn is_permission_flag(flag: &str) -> bool {
        ["--deny-", "--allow-"].iter().any(|prefix| {
            flag.strip_prefix(prefix)
                .is_some_and(|name| Capability::from_flag_name(name).is_some())
        })
    }

    /// Records a `--deny-<name>` / `--allow-<name>` flag, with the value it
    /// carried (if any).
    ///
    /// `name` has already been split off the `--deny-`/`--allow-` prefix and is
    /// rejected here if it is not one of the eight — never ignored, since an
    /// unrecognised permission flag would otherwise read as a sandbox that is
    /// not actually on.
    pub fn record(
        &mut self,
        flag: &str,
        name: &str,
        allow: bool,
        value: Option<&str>,
    ) -> Result<(), String> {
        let prefix = if allow { "--allow-" } else { "--deny-" };
        let cap = Capability::from_flag_name(name).ok_or_else(|| {
            let known = Capability::HOST_FACING
                .into_iter()
                .filter_map(Capability::flag_name)
                .map(|n| format!("{prefix}{n}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("unknown option: {flag}\n\nexpected one of: {known}")
        })?;
        if !allow {
            if let Some(value) = value {
                // Scoping has one direction, like everything else in D38: it
                // narrows a grant. A `--deny-net=host` would be the other one —
                // "everything except" — and rule 3 says a mode has exactly one.
                return Err(format!(
                    "{flag} takes no value (got {flag}={value}).\n\n\
                     A denial is all-or-nothing: scoping narrows a grant, so it is written \
                     as --allow-{name}=<list>, never as a denial of specific values."
                ));
            }
            self.denied.push((flag.to_string(), cap));
            return Ok(());
        }
        let values = match value {
            None => None,
            Some(value) => {
                if scope_hint(cap).is_none() {
                    // Rejected, never ignored: a value that parsed but was not
                    // enforced would tell the user the run is scoped while the
                    // capability is wide open.
                    if cap == Capability::FileSystem {
                        return Err(format!(
                            "{flag} takes no value (got {flag}={value}).\n\n\
                             What may be *loaded* is an import policy, not a capability \
                             scope: the capability decides whether the loader runs, the \
                             policy decides what it may resolve. Use \
                             --import-policy=<file> — a JSON file with \"allow\" and/or \
                             \"deny\" lists of packages and paths."
                        ));
                    }
                    return Err(format!(
                        "{flag} takes no value (got {flag}={value}).\n\n\
                         Scoping {name} is not implemented — {flag} is all-or-nothing. \
                         It is rejected rather than ignored so a run is never narrower on the \
                         command line than it is in reality.\n\n\
                         A list works for: {}.",
                        scopable_flags()
                    ));
                }
                let entries = parse_scope_list(flag, value)?;
                // Validate the entry syntax now, while the flag is still the
                // thing being talked about: a bad address should be an argument
                // error naming the flag, not a provider failure at the first
                // connect.
                if matches!(cap, Capability::Net | Capability::NetListen) {
                    HostAllowlist::parse(&entries).map_err(|e| {
                        format!(
                            "{flag}: {e}\n\n\
                             An entry is a host (`example.com`), a host and port \
                             (`db.internal:5432`), or a bare port (`8080`, any interface). \
                             Bracket an IPv6 address that carries a port: `[::1]:8080`."
                        )
                    })?;
                }
                if cap == Capability::Signals {
                    // A name this runtime does not know would watch nothing, and
                    // read as protection that is not there.
                    for entry in &entries {
                        if Signal::from_name(entry).is_none() {
                            return Err(format!(
                                "{flag}: {entry} is not a signal name.\n\n\
                                 Expected one of: SIGINT, SIGTERM, SIGHUP, SIGUSR1, SIGUSR2, \
                                 SIGBREAK (what this platform can deliver is reported by \
                                 `signals()` from runtime:process)."
                            ));
                        }
                    }
                }
                Some(entries)
            }
        };
        self.allowed.push(Allow {
            flag: match value {
                Some(value) => format!("{flag}={value}"),
                None => flag.to_string(),
            },
            cap,
            values,
        });
        Ok(())
    }

    /// The scope lists these flags describe, or an error if a capability was
    /// both granted whole and narrowed.
    ///
    /// Repeating a scoped flag **unions** its entries (`--allow-run=git
    /// --allow-run=ls` ≡ `--allow-run=git,ls`), which keeps D38's rule that no
    /// flag ever overrides another: two flags that both add can be read in any
    /// order and the list is still the answer.
    pub fn scopes(&self) -> Result<Scopes, String> {
        let mut scopes: Scopes = HashMap::new();
        // The bare `--allow-<name>` that granted each capability whole, if any.
        let mut whole: HashMap<Capability, &str> = HashMap::new();
        for allow in &self.allowed {
            match &allow.values {
                None => {
                    if let Some(scoped) = scopes.keys().find(|cap| **cap == allow.cap) {
                        let scoped = self.first_scoped_flag(*scoped);
                        return Err(mixed_scope_error(scoped, &allow.flag));
                    }
                    whole.insert(allow.cap, &allow.flag);
                }
                Some(values) => {
                    if let Some(whole) = whole.get(&allow.cap) {
                        return Err(mixed_scope_error(&allow.flag, whole));
                    }
                    let entries = scopes.entry(allow.cap).or_default();
                    for value in values {
                        if !entries.contains(value) {
                            entries.push(value.clone());
                        }
                    }
                }
            }
        }
        Ok(scopes)
    }

    /// The first scoped flag written for `cap`, for an error message.
    fn first_scoped_flag(&self, cap: Capability) -> &str {
        self.allowed
            .iter()
            .find(|allow| allow.cap == cap && allow.values.is_some())
            .map_or("", |allow| allow.flag.as_str())
    }

    /// The capability set these flags describe, or an error naming the rule that
    /// was broken.
    pub fn resolve(&self) -> Result<CapabilitySet, String> {
        // Rule 1.
        if self.allow_all && self.deny_all {
            return Err(
                "--allow-all and --deny-all disagree: one grants every capability, the other \
                 none.\n\n\
                 No flag overrides another, so there is nothing to resolve this with. Pass \
                 only one, and name the exceptions with the opposite prefix."
                    .to_string(),
            );
        }
        // Rule 2, in whichever direction this command line is going.
        if self.permissive() {
            if let Some(Allow { flag, .. }) = self.allowed.first() {
                return Err(self.wrong_direction(flag, true));
            }
            let mut caps = CapabilitySet::all();
            for (_, cap) in &self.denied {
                caps.revoke(*cap);
            }
            Ok(caps)
        } else {
            if let Some((flag, _)) = self.denied.first() {
                return Err(self.wrong_direction(flag, false));
            }
            let mut caps = CapabilitySet::all().without_host_access();
            for allow in &self.allowed {
                // A scoped allow grants the same bit: the capability is what
                // opens the door, the scope list is what the provider then
                // refuses to hand over. `--allow-env=HOME` therefore reports
                // `has("env") === true`, which is the truth — the guest can
                // read *an* environment variable.
                caps.grant(allow.cap);
            }
            Ok(caps)
        }
    }

    /// The rule-2 error for `flag`, which moves against its mode's direction.
    ///
    /// Two shapes, because two things put a command line in that mode and the
    /// fix differs: an explicit `--allow-all`/`--deny-all` is a flag the user
    /// typed and can drop, while the binary's own baseline is not on the line at
    /// all — so that message has to say what the default *is* before it can
    /// explain why the flag adds nothing.
    fn wrong_direction(&self, flag: &str, allowing: bool) -> String {
        // The `--all` flag that would give `flag` something to move, and the one
        // that (if typed) put the line in this mode.
        let (needed, mode_flag) = if allowing {
            ("--deny-all", "--allow-all")
        } else {
            ("--allow-all", "--deny-all")
        };
        if (allowing && self.allow_all) || (!allowing && self.deny_all) {
            return format!(
                "{mode_flag} cannot be combined with {flag}: {mode_flag} already {} everything \
                 {flag} would. Use one or the other.",
                if allowing { "grants" } else { "denies" }
            );
        }
        let (state, direction, sibling) = if allowing {
            ("granted", "add", "--deny-<name>")
        } else {
            ("denied", "take away", "--allow-<name>")
        };
        format!(
            "{flag} requires {needed}: every capability is {state} by default, so there is \
             nothing for {flag} to {direction}. Use {needed} {flag} to start from the other \
             end, or {sibling} to name single capabilities."
        )
    }
}

/// The `--allow-<name>=<list>` flags that accept a scope list today, for the
/// error that names them.
fn scopable_flags() -> String {
    Capability::HOST_FACING
        .into_iter()
        .filter(|cap| scope_hint(*cap).is_some())
        .filter_map(Capability::flag_name)
        .map(|name| format!("--allow-{name}=<list>"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The error for a capability that was both granted whole and narrowed.
///
/// There is no precedence rule to apply here and deliberately so (D38 rule 3):
/// taking the wider flag widens a run the user asked to narrow, and taking the
/// narrower one silently ignores a flag they typed. Both are the failure this
/// design exists to avoid, so the command line is rejected instead.
fn mixed_scope_error(scoped: &str, whole: &str) -> String {
    format!(
        "{scoped} and {whole} disagree: one narrows the grant to a list, the other grants it \
         whole.\n\n\
         No flag overrides another, so there is nothing to resolve this with. Pass only \
         {scoped} to narrow, or only {whole} to grant it all."
    )
}

/// Splits a scoped permission value into its entries — D38's value grammar,
/// one grammar for every capability that takes a list:
///
/// - entries are separated by commas, so `--allow-run=git,ls` is two programs;
/// - surrounding whitespace on each entry is trimmed, so `--allow-run="git, ls"`
///   is the same thing and quoting is a shell convenience, not a syntax;
/// - an **empty entry** (`a,,b`, a trailing comma) is an error, because a typo
///   must never silently change what the run may reach;
/// - a repeated entry is kept once, in first-written order.
fn parse_scope_list(flag: &str, value: &str) -> Result<Vec<String>, String> {
    if value.is_empty() {
        return Err(format!(
            "{flag}= has an empty value — write `{flag}=<list>` to narrow the grant, or the \
             bare `{flag}` to grant it whole."
        ));
    }
    let mut entries: Vec<String> = Vec::new();
    for entry in value.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            return Err(format!(
                "{flag}={value} has an empty entry.\n\n\
                 A stray or trailing comma is a typo, and a typo must not quietly change what \
                 the run may reach. Write the list as `{flag}=a,b`; spaces around an entry are \
                 trimmed."
            ));
        }
        if !entries.iter().any(|seen| seen == entry) {
            entries.push(entry.to_string());
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flags `line` describes, recorded as a parse loop would.
    fn resolve(baseline: Baseline, line: &[&str]) -> Result<CapabilitySet, String> {
        let mut permissions = Permissions::new(baseline);
        for arg in line {
            let (flag, value) = match arg.split_once('=') {
                Some((flag, value)) => (flag, Some(value)),
                None => (*arg, None),
            };
            match flag {
                "--deny-all" => permissions.deny_all(),
                "--allow-all" => permissions.allow_all(),
                _ => {
                    let allow = flag.starts_with("--allow-");
                    let name = &flag[if allow { 8 } else { 7 }..];
                    permissions.record(flag, name, allow, value)?;
                }
            }
        }
        permissions.resolve()
    }

    /// The whole of D65: the same line, read from either end.
    #[test]
    fn the_baseline_is_the_only_difference_between_the_binaries() {
        let nothing = resolve(Baseline::Nothing, &[]).unwrap();
        let everything = resolve(Baseline::Everything, &[]).unwrap();
        assert_eq!(nothing.denied_names().len(), Capability::HOST_FACING.len());
        assert!(everything.denied_names().is_empty());

        // And an `--all` flag overrides the baseline in either direction, so a
        // line that says which end it starts from means one thing everywhere.
        for baseline in [Baseline::Nothing, Baseline::Everything] {
            assert!(
                resolve(baseline, &["--allow-all"])
                    .unwrap()
                    .denied_names()
                    .is_empty()
            );
            assert_eq!(
                resolve(baseline, &["--deny-all"]).unwrap().denied_names(),
                nothing.denied_names()
            );
        }
    }

    /// Rule 2, in both modes and from both baselines: a flag that cannot move
    /// its mode's direction is an error naming the `--all` flag that would let
    /// it, never a silent no-op.
    #[test]
    fn a_flag_against_its_modes_direction_is_refused() {
        // Restrictive by baseline, and by flag.
        for line in [&["--deny-net"][..], &["--deny-all", "--deny-net"][..]] {
            let err = resolve(Baseline::Nothing, line).unwrap_err();
            assert!(err.contains("--deny-net"), "{err}");
        }
        // Permissive by baseline, and by flag.
        for line in [&["--allow-net"][..], &["--allow-all", "--allow-net"][..]] {
            let err = resolve(Baseline::Everything, line).unwrap_err();
            assert!(err.contains("--allow-net"), "{err}");
        }
        // Which is to say: the same line is fine from the other baseline.
        assert!(resolve(Baseline::Everything, &["--deny-net"]).is_ok());
        assert!(resolve(Baseline::Nothing, &["--allow-net"]).is_ok());
    }

    /// Rule 1, from either baseline: the two `--all` flags cannot be combined.
    #[test]
    fn allow_all_and_deny_all_never_appear_together() {
        for baseline in [Baseline::Nothing, Baseline::Everything] {
            let err = resolve(baseline, &["--allow-all", "--deny-all"]).unwrap_err();
            assert!(err.contains("disagree"), "{err}");
        }
    }

    /// A scoped grant opens the same door as a bare one — the list is what the
    /// provider then refuses to hand over, not a lesser capability.
    #[test]
    fn a_scoped_allow_grants_the_capability() {
        let caps = resolve(Baseline::Nothing, &["--allow-env=HOME,PATH"]).unwrap();
        assert!(caps.contains(Capability::Env));
        assert!(!caps.contains(Capability::Net));
    }

    /// The error a wrong-direction flag gets depends on *why* its mode is what
    /// it is: a flag the user typed can be dropped, a baseline cannot.
    #[test]
    fn the_error_names_what_the_reader_can_act_on() {
        let typed = resolve(Baseline::Nothing, &["--allow-all", "--allow-net"]).unwrap_err();
        assert!(typed.contains("cannot be combined with"), "{typed}");

        let baseline = resolve(Baseline::Nothing, &["--deny-net"]).unwrap_err();
        assert!(baseline.contains("requires --allow-all"), "{baseline}");
        assert!(baseline.contains("denied by default"), "{baseline}");
    }
}
