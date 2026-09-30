//! Connections held by the built-in database drivers' ops (DECISIONS.md D147),
//! shared by every wire protocol the runtime speaks.
//!
//! **Checked out, not locked.** An op takes the connection out of its slot for
//! as long as it runs and puts it back after. A connection is one
//! conversation and the driver never runs two exchanges on it at once, so
//! finding one checked out is a bug worth naming rather than something to
//! queue behind.
//!
//! **Connects carry a ticket.** A connect that times out in the driver has to
//! be able to close the socket it opened — a server that accepted and then went
//! silent would otherwise hold the handshake, and the event loop, open for good
//! — so a connect in flight records its socket under a ticket the driver chose.
//! Per agent, like the registry itself: one agent cannot abort another's.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use es_runtime_common::{ErrorCode, ExceptionClass};
use es_runtime_engine::OpError;

/// The socket a connect in flight has opened so far, if any.
pub(crate) type Opened = Arc<Mutex<Option<u64>>>;

pub(crate) struct Registry<C> {
    slots: Arc<Mutex<HashMap<u64, Option<Box<C>>>>>,
    next_id: Arc<AtomicU64>,
    connecting: Arc<Mutex<HashMap<u64, Opened>>>,
}

impl<C> Clone for Registry<C> {
    fn clone(&self) -> Self {
        Registry {
            slots: self.slots.clone(),
            next_id: self.next_id.clone(),
            connecting: self.connecting.clone(),
        }
    }
}

impl<C> Registry<C> {
    pub(crate) fn new() -> Registry<C> {
        Registry {
            slots: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
            connecting: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Stores an opened connection under a fresh id.
    pub(crate) fn insert(&self, connection: C) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.slots
            .lock()
            .unwrap()
            .insert(id, Some(Box::new(connection)));
        id
    }

    /// Takes the connection out for one op.
    pub(crate) fn checkout(&self, id: u64) -> Result<Box<C>, OpError> {
        let mut slots = self.slots.lock().unwrap();
        match slots.get_mut(&id) {
            Some(slot) => slot.take().ok_or_else(|| {
                OpError::new(
                    ExceptionClass::Error,
                    "this connection is already running an operation",
                )
                .with_code(ErrorCode::Io)
            }),
            None => Err(
                OpError::new(ExceptionClass::Error, "the connection is closed")
                    .with_code(ErrorCode::Io),
            ),
        }
    }

    /// Puts it back — unless the connection was closed meanwhile, in which
    /// case it is dropped.
    pub(crate) fn checkin(&self, id: u64, connection: Box<C>) {
        if let Some(slot) = self.slots.lock().unwrap().get_mut(&id) {
            *slot = Some(connection);
        }
    }

    /// Removes a connection: `Some(Some(_))` when it was in its slot,
    /// `Some(None)` when an op has it checked out, `None` when unknown.
    pub(crate) fn remove(&self, id: u64) -> Option<Option<Box<C>>> {
        self.slots.lock().unwrap().remove(&id)
    }

    /// Records a connect in flight under `ticket`.
    pub(crate) fn connecting(&self, ticket: u64) -> Opened {
        let opened = Arc::new(Mutex::new(None));
        self.connecting
            .lock()
            .unwrap()
            .insert(ticket, opened.clone());
        opened
    }

    /// The connect under `ticket` has finished, one way or the other.
    pub(crate) fn connected(&self, ticket: u64) {
        self.connecting.lock().unwrap().remove(&ticket);
    }

    /// The socket the connect under `ticket` has opened, to close it.
    pub(crate) fn connecting_socket(&self, ticket: u64) -> Option<u64> {
        self.connecting
            .lock()
            .unwrap()
            .get(&ticket)
            .and_then(|opened| *opened.lock().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_checked_out_connection_is_named_as_busy_and_comes_back() {
        let registry: Registry<u32> = Registry::new();
        let id = registry.insert(7);
        let taken = registry.checkout(id).unwrap();
        assert!(registry.checkout(id).is_err(), "a second op is refused");
        registry.checkin(id, taken);
        assert_eq!(*registry.checkout(id).unwrap(), 7);
    }

    #[test]
    fn a_connection_closed_while_out_is_dropped_on_return() {
        let registry: Registry<u32> = Registry::new();
        let id = registry.insert(1);
        let taken = registry.checkout(id).unwrap();
        assert!(matches!(registry.remove(id), Some(None)));
        registry.checkin(id, taken);
        assert!(registry.checkout(id).is_err());
    }

    #[test]
    fn a_ticket_names_the_socket_its_connect_opened() {
        let registry: Registry<u32> = Registry::new();
        let opened = registry.connecting(5);
        assert_eq!(registry.connecting_socket(5), None);
        *opened.lock().unwrap() = Some(42);
        assert_eq!(registry.connecting_socket(5), Some(42));
        registry.connected(5);
        assert_eq!(registry.connecting_socket(5), None);
    }
}
