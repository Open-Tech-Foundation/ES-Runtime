//! The `--deny-*` / `--allow-*` flag grammar (DECISIONS D38), shared by every
//! binary on this runtime so that one command line means one thing everywhere.

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

/// The permission flags accumulated while parsing, resolved into a
/// [`CapabilitySet`] once the whole command line has been seen (D38).
///
/// Three rules, and they exist so that **no flag ever overrides another** — a
/// reader goes top to bottom and the list is the answer:
///
/// 1. `--deny-all` and `--deny-<name>` are mutually exclusive. `--deny-all` is
///    precisely the union of the eight, so a combination could only be
///    redundant.
/// 2. `--allow-<name>` requires `--deny-all`. Against the default baseline
///    (everything granted) an allow is either a no-op or a contradiction of its
///    own `--deny-<name>` sibling.
/// 3. Each mode therefore has exactly one direction: `--deny-<name>` subtracts
///    from everything, `--deny-all --allow-<name>` adds to nothing.
#[derive(Default)]
pub struct Permissions {
    /// The `--deny-all` flag, if given.
    pub all: bool,
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
                     as --deny-all --allow-{name}=<list>, never as a denial of specific \
                     values."
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
        if let (true, Some((flag, _))) = (self.all, self.denied.first()) {
            return Err(format!(
                "--deny-all cannot be combined with {flag}: --deny-all already denies \
                 everything {flag} would. Use one or the other."
            ));
        }
        if let (false, Some(Allow { flag, .. })) = (self.all, self.allowed.first()) {
            return Err(format!(
                "{flag} requires --deny-all: everything is granted by default, so there is \
                 nothing for {flag} to add. Use --deny-all {flag} to start from nothing, or \
                 --deny-<name> to take single capabilities away."
            ));
        }
        if self.all {
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
        } else {
            let mut caps = CapabilitySet::all();
            for (_, cap) in &self.denied {
                caps.revoke(*cap);
            }
            Ok(caps)
        }
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
