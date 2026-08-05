//! Deny-by-default capability tokens (ARCHITECTURE.md §7, DECISIONS.md D7).
//!
//! Every side-effecting operation is gated on a [`Capability`]. The embedder
//! constructs a [`CapabilitySet`] and threads it through the runtime; there is
//! no ambient authority and no global escape hatch. A [`CapabilitySet::default`]
//! grants nothing — capabilities are added explicitly.
//!
//! The set is a small hand-rolled bitset (no external bitflags dependency): the
//! capability space is fixed and tiny, mirroring the I/O providers in
//! ARCHITECTURE.md §6.

use crate::error::{Error, Result};

/// A side effect that must be explicitly granted before it can occur.
///
/// Each variant corresponds to an I/O provider (ARCHITECTURE.md §6). Holding the
/// capability is necessary — and, with the provider present, sufficient — for
/// the runtime to permit the effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Capability {
    /// Read wall/monotonic time (`Clock` provider).
    Clock,
    /// Draw cryptographic randomness (`Entropy` provider).
    Entropy,
    /// Schedule and cancel timers (`Timers` provider).
    Timers,
    /// Perform outbound network requests (`NetTransport` provider).
    Net,
    /// Access the filesystem (`FileSystem` provider).
    FileSystem,
    /// Offload blocking work (`TaskSpawner` provider).
    TaskSpawn,
    /// Read host process info — environment, args, cwd, platform (`Process`
    /// provider; backs `runtime:process`). Deny-by-default like the rest.
    Env,
    /// Read files within the configured root jail (`FileSystem` provider; backs
    /// `runtime:fs` reads — `file().text()`, `readDir`, `stat`, …).
    FileRead,
    /// Create, write, or remove files within the configured root jail
    /// (`FileSystem` provider; backs `runtime:fs` mutations — `write`, `mkdir`,
    /// `remove`, `rename`).
    FileWrite,
    /// Bind a listening socket and accept inbound connections (`Net` provider;
    /// backs `runtime:net` `listen`). Distinct from [`Net`](Self::Net) outbound
    /// connections — a server-side privilege, deny-by-default.
    NetListen,
    /// Observe OS signals — `SIGINT`, `SIGTERM`, … (`Signals` provider; backs
    /// `runtime:process` `onSignal`). Distinct from [`Env`](Self::Env): reading
    /// the environment is a read of process *state*, while watching a signal
    /// intercepts process *control* — a handler suppresses the default
    /// termination, so it is the privilege to refuse to die on request.
    Signals,
    /// Spawn a child process (`CommandProvider`; backs `runtime:system`).
    ///
    /// The most consequential grant in the set, and never implied by another: a
    /// child process runs **outside** every confinement this runtime applies —
    /// no capability check, no filesystem root jail, no execution deadline. In
    /// practice, granting `Run` to guest code grants everything the host user
    /// can do. Withhold it from anything untrusted, and prefer a provider-side
    /// allowlist when it must be granted.
    Run,
    /// Start a worker agent — its own thread and its own isolate
    /// (`WorkerHost` provider; backs the `Worker` global).
    ///
    /// A resource grant rather than a reach into the host: a worker runs under
    /// this runtime's confinement, not outside it like [`Run`](Self::Run). What
    /// it costs is a thread and an isolate's heap, which is why it is gated at
    /// all — an ungated `Worker` is an unbounded way to consume both.
    ///
    /// A worker's own capabilities are **not** inherited. It starts from
    /// [`CapabilitySet::none`] and is granted explicitly, bounded above by the
    /// parent's set, so a spawn can never widen what the spawner holds.
    Worker,
}

impl Capability {
    /// All capabilities, in a fixed order. Used to build [`CapabilitySet::all`]
    /// and to keep the bit assignment in [`bit`](Self::bit) exhaustive.
    const ALL: [Capability; 13] = [
        Capability::Clock,
        Capability::Entropy,
        Capability::Timers,
        Capability::Net,
        Capability::FileSystem,
        Capability::TaskSpawn,
        Capability::Env,
        Capability::FileRead,
        Capability::FileWrite,
        Capability::NetListen,
        Capability::Signals,
        Capability::Run,
        Capability::Worker,
    ];

    /// The **host-facing** capabilities: everything that reaches past the
    /// isolate. These are exactly the grants `esrun`'s `--deny-*` flags revoke
    /// and the names `runtime:process` `permissions` reports (D38).
    ///
    /// The four omitted ([`Clock`](Self::Clock), [`Entropy`](Self::Entropy),
    /// [`Timers`](Self::Timers), [`TaskSpawn`](Self::TaskSpawn)) back
    /// `Date.now()`, `crypto`, and `setTimeout` — no op gates them, and a flag
    /// that revoked them would deny nothing while implying otherwise.
    ///
    /// [`Worker`](Self::Worker) is here despite not reaching past the isolate:
    /// a real op gates it, so `--deny-workers` denies something, and a host
    /// bounding what a script may consume wants to name it.
    pub const HOST_FACING: [Capability; 9] = [
        Capability::FileRead,
        Capability::FileWrite,
        Capability::FileSystem,
        Capability::Net,
        Capability::NetListen,
        Capability::Env,
        Capability::Run,
        Capability::Signals,
        Capability::Worker,
    ];

    /// This capability's name in the denial vocabulary — the suffix of `esrun`'s
    /// `--deny-<name>` flag and the string `permissions.has(<name>)` takes — or
    /// `None` for a capability with no flag (see [`HOST_FACING`](Self::HOST_FACING)).
    ///
    /// One vocabulary across the CLI, the JS API, and the docs: `--deny-net` ⇔
    /// `has("net")`. Never spell these anywhere else.
    pub const fn flag_name(self) -> Option<&'static str> {
        match self {
            Capability::FileRead => Some("read"),
            Capability::FileWrite => Some("write"),
            // "imports", not "modules": what it gates is the *loader* — a file or
            // `node_modules` import. `runtime:` built-ins never consult it (D26).
            Capability::FileSystem => Some("imports"),
            Capability::Net => Some("net"),
            Capability::NetListen => Some("listen"),
            Capability::Env => Some("env"),
            Capability::Run => Some("run"),
            Capability::Signals => Some("signals"),
            // Plural, unlike the rest: what it gates is starting workers, and
            // `--deny-worker` would read as naming one particular worker.
            Capability::Worker => Some("workers"),
            Capability::Clock
            | Capability::Entropy
            | Capability::Timers
            | Capability::TaskSpawn => None,
        }
    }

    /// The capability a denial name refers to, or `None` if the name is not one
    /// of the eight. The inverse of [`flag_name`](Self::flag_name).
    pub fn from_flag_name(name: &str) -> Option<Capability> {
        Capability::HOST_FACING
            .into_iter()
            .find(|cap| cap.flag_name() == Some(name))
    }

    /// This capability's single-bit mask within a [`CapabilitySet`].
    const fn bit(self) -> u32 {
        // Explicit, stable assignments — never reorder; only append.
        match self {
            Capability::Clock => 1 << 0,
            Capability::Entropy => 1 << 1,
            Capability::Timers => 1 << 2,
            Capability::Net => 1 << 3,
            Capability::FileSystem => 1 << 4,
            Capability::TaskSpawn => 1 << 5,
            Capability::Env => 1 << 6,
            Capability::FileRead => 1 << 7,
            Capability::FileWrite => 1 << 8,
            Capability::NetListen => 1 << 9,
            Capability::Signals => 1 << 10,
            Capability::Run => 1 << 11,
            Capability::Worker => 1 << 12,
        }
    }
}

impl std::fmt::Display for Capability {
    /// The variant name, plus the denial vocabulary word when it has one — so a
    /// denial message tells a CLI user which `--deny-<name>` produced it without
    /// making them consult a translation table (D38).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")?;
        match self.flag_name() {
            Some(name) => write!(f, " (permission \"{name}\")"),
            None => Ok(()),
        }
    }
}

/// An immutable, copyable grant of zero or more [`Capability`]s.
///
/// Built additively from [`none`](Self::none) (the default) so that *not*
/// granting a capability is the path of least resistance — deny by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapabilitySet {
    /// Bitset over [`Capability::bit`]. `0` denies everything.
    bits: u32,
}

impl CapabilitySet {
    /// The empty set — every capability denied. Equivalent to
    /// [`CapabilitySet::default`].
    pub const fn none() -> Self {
        CapabilitySet { bits: 0 }
    }

    /// The full set — every capability granted. Intended for trusted embedders
    /// and tests, never as a default.
    pub fn all() -> Self {
        let mut set = CapabilitySet::none();
        for cap in Capability::ALL {
            set.bits |= cap.bit();
        }
        set
    }

    /// Returns `self` with `cap` added (builder style).
    #[must_use]
    pub const fn with(mut self, cap: Capability) -> Self {
        self.bits |= cap.bit();
        self
    }

    /// Adds `cap` to this set in place.
    pub fn grant(&mut self, cap: Capability) {
        self.bits |= cap.bit();
    }

    /// Removes `cap` from this set in place.
    pub fn revoke(&mut self, cap: Capability) {
        self.bits &= !cap.bit();
    }

    /// Whether `cap` is granted.
    pub const fn contains(self, cap: Capability) -> bool {
        self.bits & cap.bit() != 0
    }

    /// Returns `self` with every [`HOST_FACING`](Capability::HOST_FACING)
    /// capability revoked — `esrun`'s `--deny-all` (D38).
    ///
    /// What survives is the isolate's own hygiene (clock, entropy, timers,
    /// task spawn): the guest still computes, but reaches nothing outside.
    #[must_use]
    pub fn without_host_access(mut self) -> Self {
        for cap in Capability::HOST_FACING {
            self.revoke(cap);
        }
        self
    }

    /// The [`flag_name`](Capability::flag_name)s of the host-facing capabilities
    /// this set does **not** hold, in [`HOST_FACING`](Capability::HOST_FACING)
    /// order. Backs `runtime:process` `permissions.denied`.
    pub fn denied_names(self) -> Vec<&'static str> {
        Capability::HOST_FACING
            .into_iter()
            .filter(|cap| !self.contains(*cap))
            .filter_map(Capability::flag_name)
            .collect()
    }

    /// Returns `Ok(())` if `cap` is granted, else
    /// [`Error::CapabilityDenied`]. Call this *before* a side effect: a denial
    /// must yield a clean error and never a partial effect (ARCHITECTURE.md §4).
    pub fn require(self, cap: Capability) -> Result<()> {
        if self.contains(cap) {
            Ok(())
        } else {
            Err(Error::CapabilityDenied(cap))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_denies_everything() {
        let set = CapabilitySet::default();
        for cap in Capability::ALL {
            assert!(!set.contains(cap), "{cap:?} should be denied by default");
            assert!(set.require(cap).is_err());
        }
    }

    #[test]
    fn all_grants_everything() {
        let set = CapabilitySet::all();
        for cap in Capability::ALL {
            assert!(set.contains(cap));
            assert!(set.require(cap).is_ok());
        }
    }

    #[test]
    fn with_is_additive_and_isolated() {
        let set = CapabilitySet::none().with(Capability::Net);
        assert!(set.contains(Capability::Net));
        // Granting one must not grant another (distinct bits).
        assert!(!set.contains(Capability::FileSystem));
    }

    #[test]
    fn grant_then_revoke_round_trips() {
        let mut set = CapabilitySet::none();
        set.grant(Capability::Clock);
        assert!(set.contains(Capability::Clock));
        set.revoke(Capability::Clock);
        assert!(!set.contains(Capability::Clock));
    }

    #[test]
    fn require_denied_names_the_capability() {
        let err = CapabilitySet::none()
            .require(Capability::Entropy)
            .unwrap_err();
        match err {
            Error::CapabilityDenied(Capability::Entropy) => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn flag_names_round_trip() {
        for cap in Capability::HOST_FACING {
            let name = cap
                .flag_name()
                .expect("host-facing capability needs a name");
            assert_eq!(Capability::from_flag_name(name), Some(cap));
        }
    }

    #[test]
    fn flag_names_are_unique() {
        // A collision would make two `--deny-*` flags revoke the same bit while
        // silently leaving the other granted.
        let mut names: Vec<&str> = Capability::HOST_FACING
            .into_iter()
            .filter_map(Capability::flag_name)
            .collect();
        names.sort_unstable();
        let count = names.len();
        names.dedup();
        assert_eq!(names.len(), count);
    }

    #[test]
    fn only_host_facing_capabilities_have_flag_names() {
        for cap in Capability::ALL {
            assert_eq!(
                cap.flag_name().is_some(),
                Capability::HOST_FACING.contains(&cap),
                "{cap:?} disagrees with HOST_FACING about having a flag"
            );
        }
    }

    #[test]
    fn unknown_flag_name_is_rejected() {
        // Never a silent `None`-as-granted: the CLI and `has()` both report it.
        assert_eq!(Capability::from_flag_name("ffi"), None);
        assert_eq!(Capability::from_flag_name(""), None);
        assert_eq!(Capability::from_flag_name("Net"), None);
    }

    #[test]
    fn deny_all_keeps_only_the_ungated_capabilities() {
        let set = CapabilitySet::all().without_host_access();
        for cap in Capability::HOST_FACING {
            assert!(!set.contains(cap), "{cap:?} should be denied by --deny-all");
        }
        // The isolate's own hygiene survives: a denied script still computes.
        for cap in [
            Capability::Clock,
            Capability::Entropy,
            Capability::Timers,
            Capability::TaskSpawn,
        ] {
            assert!(set.contains(cap), "{cap:?} should survive --deny-all");
        }
    }

    #[test]
    fn denied_names_reports_exactly_what_is_missing() {
        assert!(CapabilitySet::all().denied_names().is_empty());
        assert_eq!(
            CapabilitySet::all().without_host_access().denied_names(),
            [
                "read", "write", "imports", "net", "listen", "env", "run", "signals", "workers"
            ]
        );
        let mut set = CapabilitySet::all();
        set.revoke(Capability::Net);
        assert_eq!(set.denied_names(), ["net"]);
    }

    #[test]
    fn every_capability_has_a_distinct_bit() {
        // Guards against a copy-paste error in `bit()` collapsing two variants.
        let mut seen = 0u32;
        for cap in Capability::ALL {
            assert_eq!(seen & cap.bit(), 0, "{cap:?} shares a bit");
            seen |= cap.bit();
        }
    }
}
