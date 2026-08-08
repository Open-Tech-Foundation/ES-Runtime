//! Per-agent ownership of host handles (DECISIONS D50).
//!
//! Most host resources are reached by an integer id: a socket, a child process,
//! an HTTP server, one in-flight request. The op that *creates* the resource is
//! capability-checked (`net_connect` needs `Net`, `system_spawn` needs `Run`);
//! the ops that *use* the resulting id are not, on the reasoning that the id
//! could only have come from a checked call.
//!
//! That reasoning holds for one agent and fails for a process full of them.
//! Providers are shared — the CLI hands every worker the same
//! `Arc<SystemCommands>`, `Arc<SystemNet>`, `Arc<SystemHttpServer>` — and their
//! ids come from one counter, so they start at 1 and go up. `globalThis.__ops`
//! is reachable from guest code by design. Put together, a worker spawned with
//! `{ permissions: [] }` could call `system_kill(1, "SIGKILL")` or
//! `http_respond(2, …)` and reach a resource belonging to the agent that
//! started it — with no capability of its own and nothing to guess.
//!
//! So an id is not, by itself, authority. This is the second half: the agent
//! that minted a handle records it here, and every op that takes one checks it
//! first. The registry is built inside each `install`, which runs once per
//! `Runtime`, so "this agent" is exactly the enclosing isolate and no sharing
//! is possible — the `Rc` cannot leave the thread it was made on.
//!
//! **Not covered: `MessagePort`.** A port id is *meant* to move between agents:
//! transferring one hands the id over inside the structured-clone bytes, which
//! the host writes and reads from JS, so there is no host-side moment at which
//! the receiving agent's claim could be recorded. Ports are defended instead by
//! making their ids unguessable (`ProcessPortHub` mints them from the CSPRNG),
//! which is what ownership tracking would have bought there and nothing more.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use es_runtime_common::{ErrorCode, ExceptionClass};
use es_runtime_engine::OpError;

/// The handles of one kind that *this agent* obtained legitimately.
///
/// Cloning shares the set — the clones are the copies captured by each op's
/// closure, all of them views onto one agent's ownership.
#[derive(Clone)]
pub(crate) struct Handles {
    /// What these ids name, for the error message: "socket", "child process".
    kind: &'static str,
    ids: Rc<RefCell<HashSet<u64>>>,
}

impl Handles {
    /// An empty registry for handles described by `kind`.
    pub(crate) fn new(kind: &'static str) -> Handles {
        Handles {
            kind,
            ids: Rc::new(RefCell::new(HashSet::new())),
        }
    }

    /// Records `id` as this agent's, and hands it back so a minting op can
    /// write `Ok(socket_value(handles.own(id), &info))`.
    pub(crate) fn own(&self, id: u64) -> u64 {
        self.ids.borrow_mut().insert(id);
        id
    }

    /// `id` if this agent owns it, else a refusal naming what was asked for.
    ///
    /// [`ErrorCode::ForeignHandle`] rather than `ERR_CAPABILITY_DENIED`: the
    /// caller may well hold the capability and still not hold *this* handle,
    /// and the two have different fixes. It is not a `NotFound` either — the
    /// resource usually exists; it is simply not this agent's to name.
    pub(crate) fn check(&self, id: u64) -> Result<u64, OpError> {
        if self.ids.borrow().contains(&id) {
            return Ok(id);
        }
        Err(OpError::new(
            ExceptionClass::NOT_ALLOWED,
            format!(
                "{} {id} does not belong to this agent (a handle is usable only by the agent that created it)",
                self.kind
            ),
        )
        .with_code(ErrorCode::ForeignHandle))
    }

    /// [`check`](Self::check), then forget the handle — for the op that ends
    /// the resource's life. Checked first, so closing someone else's is refused
    /// exactly like using it.
    pub(crate) fn check_and_release(&self, id: u64) -> Result<u64, OpError> {
        let id = self.check(id)?;
        self.ids.borrow_mut().remove(&id);
        Ok(id)
    }

    /// Forgets `id` without checking — for a handle this agent is replacing
    /// (a socket consumed by `startTls` in favour of its upgraded self).
    pub(crate) fn release(&self, id: u64) {
        self.ids.borrow_mut().remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_owned_handle_passes_and_an_unknown_one_does_not() {
        let handles = Handles::new("socket");
        assert_eq!(handles.own(7), 7);
        assert_eq!(handles.check(7).unwrap(), 7);
        assert!(handles.check(8).is_err());
    }

    #[test]
    fn a_foreign_handle_is_refused_with_its_own_code() {
        // The case this module exists for: another agent's id, guessed.
        let handles = Handles::new("child process");
        let err = handles.check(1).unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::ForeignHandle));
        let message = err.to_string();
        assert!(message.contains("child process 1"), "{message}");
    }

    #[test]
    fn releasing_makes_a_handle_foreign_again() {
        let handles = Handles::new("server");
        handles.own(3);
        assert_eq!(handles.check_and_release(3).unwrap(), 3);
        assert!(handles.check(3).is_err());
        // And a second close is refused rather than reaching the provider,
        // where the id may since have been reissued to another agent.
        assert!(handles.check_and_release(3).is_err());
    }

    #[test]
    fn two_agents_do_not_share_a_registry() {
        let a = Handles::new("socket");
        let b = Handles::new("socket");
        a.own(1);
        assert!(b.check(1).is_err());
    }

    #[test]
    fn clones_are_one_registry() {
        // Each op captures its own clone; they must all see one agent's set.
        let a = Handles::new("socket");
        let b = a.clone();
        a.own(1);
        assert!(b.check(1).is_ok());
    }
}
