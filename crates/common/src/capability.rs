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
}

impl Capability {
    /// All capabilities, in a fixed order. Used to build [`CapabilitySet::all`]
    /// and to keep the bit assignment in [`bit`](Self::bit) exhaustive.
    const ALL: [Capability; 6] = [
        Capability::Clock,
        Capability::Entropy,
        Capability::Timers,
        Capability::Net,
        Capability::FileSystem,
        Capability::TaskSpawn,
    ];

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
    fn every_capability_has_a_distinct_bit() {
        // Guards against a copy-paste error in `bit()` collapsing two variants.
        let mut seen = 0u32;
        for cap in Capability::ALL {
            assert_eq!(seen & cap.bit(), 0, "{cap:?} shares a bit");
            seen |= cap.bit();
        }
    }
}
