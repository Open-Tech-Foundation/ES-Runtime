//! A cap on how many connections one peer address may hold at once.
//!
//! The whole-server cap (`max_connections`) bounds what a deployment spends,
//! and nothing else: one peer opening every slot reaches it exactly as a
//! thousand peers opening one each do, and the server is then full for
//! everybody. That is the gap D45 and D47 both recorded — the peer address D44
//! added is the missing half, and this is the policy that applies it.
//!
//! **Refused, not held** — the one place this deliberately differs from the
//! whole-server cap. That cap makes an excess connection *wait*, because the
//! excess is legitimate traffic that will be served as soon as a slot frees, and
//! waiting in the kernel backlog costs the server nothing. A per-peer excess is
//! by definition one client past its own share, and holding it would be the hold
//! the cap exists to prevent: the connection is already accepted, so it costs a
//! descriptor, and the peer decides when it ends. Closing it returns both
//! immediately, and a client that wanted the connection can open another when
//! one of its own finishes.
//!
//! The count is per **address**, which is as far as a TCP peer identity goes.
//! Everything behind one NAT or one load balancer therefore shares a budget —
//! which is why this is off unless a deployment asks for it, and why the number
//! belongs to whoever knows what sits in front of the server.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

/// How many connections each peer address may hold at once.
pub(crate) struct PeerLimit {
    max: usize,
    /// Live connections per address. An address is **removed** at zero rather
    /// than left at zero: the keys are supplied by whoever connects, so a map
    /// that only ever grew would be its own slow exhaustion under a botnet.
    live: Mutex<HashMap<IpAddr, usize>>,
}

/// One peer's connection, counted for as long as this is alive.
pub(crate) struct PeerSlot {
    limit: Arc<PeerLimit>,
    ip: IpAddr,
}

impl PeerLimit {
    /// A limit of `max` connections per address, or `None` for no limit — so a
    /// caller can write `PeerLimit::new(options.max_connections_per_ip)` and
    /// hold an `Option` either way.
    pub(crate) fn new(max: Option<usize>) -> Option<Arc<PeerLimit>> {
        max.map(|max| {
            Arc::new(PeerLimit {
                max,
                live: Mutex::new(HashMap::new()),
            })
        })
    }

    /// A slot for `ip`, or `None` if that address is already at its cap.
    ///
    /// The lock is held for a compare and an increment and nothing else — no
    /// I/O, no allocation beyond the first connection from an address — so this
    /// stays uncontended on an accept loop that is otherwise doing syscalls.
    pub(crate) fn take(self: &Arc<Self>, ip: IpAddr) -> Option<PeerSlot> {
        let mut live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        let held = live.entry(ip).or_insert(0);
        if *held >= self.max {
            // Never inserted a zero on the refusal path: an address that is
            // refused must not leave a key behind, which is the shape a flood
            // of one-shot connections would otherwise take.
            if *held == 0 {
                live.remove(&ip);
            }
            return None;
        }
        *held += 1;
        Some(PeerSlot {
            limit: self.clone(),
            ip,
        })
    }

    /// How many connections `ip` currently holds — for tests and diagnostics.
    #[cfg(test)]
    fn held(&self, ip: IpAddr) -> usize {
        let live = self.live.lock().unwrap_or_else(|e| e.into_inner());
        live.get(&ip).copied().unwrap_or(0)
    }
}

/// Returns the slot however the connection ended, including a panic.
impl Drop for PeerSlot {
    fn drop(&mut self) {
        let mut live = self.limit.live.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(held) = live.get_mut(&self.ip) {
            *held -= 1;
            if *held == 0 {
                live.remove(&self.ip);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([203, 0, 113, last])
    }

    #[test]
    fn one_peer_is_held_to_its_own_cap() {
        let limit = PeerLimit::new(Some(2)).unwrap();
        let a = limit.take(ip(1)).expect("first");
        let b = limit.take(ip(1)).expect("second");
        assert!(
            limit.take(ip(1)).is_none(),
            "a third got through a cap of 2"
        );
        drop(a);
        // A returned slot is immediately reusable — by that peer or another.
        limit.take(ip(1)).expect("after one ended");
        drop(b);
    }

    #[test]
    fn one_peer_at_its_cap_does_not_shut_out_the_rest() {
        // The whole point: a flood from one address must not be a flood at
        // everybody, which is all the whole-server cap can offer.
        let limit = PeerLimit::new(Some(1)).unwrap();
        let _held = limit.take(ip(1)).expect("the flooder");
        assert!(limit.take(ip(1)).is_none(), "the flooder got a second");
        for other in 2..=50 {
            assert!(limit.take(ip(other)).is_some(), "peer {other} was refused");
        }
    }

    #[test]
    fn an_address_is_forgotten_once_it_holds_nothing() {
        // The map's keys come from whoever connects, so one that only grew
        // would be its own exhaustion under a botnet.
        let limit = PeerLimit::new(Some(4)).unwrap();
        {
            let _a = limit.take(ip(1)).unwrap();
            let _b = limit.take(ip(1)).unwrap();
            assert_eq!(limit.held(ip(1)), 2);
        }
        assert_eq!(limit.held(ip(1)), 0);
        assert!(
            limit.live.lock().unwrap().is_empty(),
            "an address that holds nothing must leave no entry"
        );

        // And a refusal leaves none either.
        let full = PeerLimit::new(Some(0)).unwrap();
        assert!(full.take(ip(9)).is_none());
        assert!(full.live.lock().unwrap().is_empty());
    }

    #[test]
    fn no_limit_is_no_bookkeeping() {
        assert!(PeerLimit::new(None).is_none());
    }

    #[test]
    fn v4_and_v6_are_different_peers() {
        // They are different addresses, and nothing here tries to relate them.
        let limit = PeerLimit::new(Some(1)).unwrap();
        let _v4 = limit.take(ip(1)).expect("v4");
        let _v6 = limit
            .take(IpAddr::from([0x2001, 0xdb8, 0, 0, 0, 0, 0, 1]))
            .expect("v6");
    }
}
