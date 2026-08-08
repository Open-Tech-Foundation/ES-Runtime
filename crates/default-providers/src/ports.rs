//! The reference [`PortHub`]: `MessagePort` queues owned by the process.
//!
//! Each port is one inbox and a pointer to its peer. `post` writes to the
//! peer's inbox; the agent currently holding the port reads its own. Because
//! the queues live here rather than in an isolate, transferring a port is just
//! moving its id — the messages already in flight stay exactly where they were.
//!
//! That last property is also why port ids are **random** rather than counted.
//! Every other host handle is confined to the agent that made it (D50,
//! `es_runtime::handles`), but a port id is meant to travel: a transfer hands it
//! over inside structured-clone bytes that the host neither writes nor reads, so
//! there is no moment at which the receiving agent's claim could be recorded.
//! Holding the id *is* the authority, so the id has to be unguessable — with a
//! counter, an agent that was never given a port could read and write another
//! agent's channel by trying 1, 2, 3.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use es_runtime_providers::{BoxFuture, PortHub, ProviderError};
use tokio::sync::{Notify, mpsc};

struct Port {
    /// The other end, until one side closes.
    peer: Option<u64>,
    inbox: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Vec<u8>>>>,
    deliver: mpsc::UnboundedSender<Vec<u8>>,
    /// Wakes a parked `recv` so a transfer can take the port away from this
    /// agent without the pump swallowing a message on the way out.
    detached: Arc<Notify>,
}

/// A [`PortHub`] holding its queues in this process.
#[derive(Default)]
pub struct ProcessPortHub {
    ports: Mutex<HashMap<u64, Port>>,
}

impl ProcessPortHub {
    /// Creates an empty hub.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn make_port(&self, id: u64, peer: u64, ports: &mut HashMap<u64, Port>) {
        let (deliver, inbox) = mpsc::unbounded_channel();
        ports.insert(
            id,
            Port {
                peer: Some(peer),
                inbox: Arc::new(tokio::sync::Mutex::new(inbox)),
                deliver,
                detached: Arc::new(Notify::new()),
            },
        );
    }
}

/// A fresh unguessable port id, not already in `ports`.
///
/// 53 bits, not 64: the id crosses into the guest as a JS number, and anything
/// wider would come back rounded and name a different port. 53 bits of CSPRNG
/// output is far past what an in-process guess can search — and unlike a
/// counter, knowing one port's id says nothing about any other's.
fn mint(ports: &HashMap<u64, Port>) -> Result<u64, ProviderError> {
    loop {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).map_err(|e| ProviderError::Entropy(e.to_string()))?;
        let id = u64::from_le_bytes(bytes) & ((1 << 53) - 1);
        // 0 is skipped so it can keep meaning "no port" on the JS side.
        if id != 0 && !ports.contains_key(&id) {
            return Ok(id);
        }
    }
}

impl PortHub for ProcessPortHub {
    fn create(&self) -> Result<(u64, u64), ProviderError> {
        match self.ports.lock() {
            Ok(mut ports) => {
                let a = mint(&ports)?;
                // `b` is minted against a map that does not yet hold `a`, so it
                // is compared with it directly; the odds are astronomical and
                // the cost of being wrong is a port that is its own peer.
                let b = loop {
                    let b = mint(&ports)?;
                    if b != a {
                        break b;
                    }
                };
                self.make_port(a, b, &mut ports);
                self.make_port(b, a, &mut ports);
                Ok((a, b))
            }
            Err(_) => Err(ProviderError::Other("port registry poisoned".into())),
        }
    }

    fn post(&self, id: u64, message: Vec<u8>) -> Result<(), ProviderError> {
        if let Ok(ports) = self.ports.lock()
            && let Some(peer) = ports.get(&id).and_then(|port| port.peer)
            && let Some(target) = ports.get(&peer)
        {
            // A send failure means the peer's reader is gone; the message is
            // dropped, as it is for a port whose other end was closed.
            let _ = target.deliver.send(message);
        }
        Ok(())
    }

    fn recv(&self, id: u64) -> BoxFuture<Result<Option<Vec<u8>>, ProviderError>> {
        let handles = self.ports.lock().ok().and_then(|ports| {
            ports
                .get(&id)
                .map(|p| (p.inbox.clone(), p.detached.clone()))
        });
        Box::pin(async move {
            let Some((inbox, detached)) = handles else {
                return Ok(None);
            };
            let mut inbox = inbox.lock().await;
            // `mpsc::Receiver::recv` is cancel-safe, so losing this race leaves
            // any queued message queued — which is the whole point of
            // `detach_reader`.
            tokio::select! {
                message = inbox.recv() => Ok(message),
                () = detached.notified() => Ok(None),
            }
        })
    }

    fn detach_reader(&self, id: u64) {
        if let Ok(ports) = self.ports.lock()
            && let Some(port) = ports.get(&id)
        {
            port.detached.notify_waiters();
        }
    }

    fn close(&self, id: u64) {
        if let Ok(mut ports) = self.ports.lock()
            && let Some(port) = ports.remove(&id)
        {
            port.detached.notify_waiters();
            // Closing one end disentangles the other, so its sends now go
            // nowhere rather than piling up unread.
            if let Some(peer) = port.peer
                && let Some(peer) = ports.get_mut(&peer)
            {
                peer.peer = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_post_reaches_the_peer_and_not_the_sender() {
        let hub = ProcessPortHub::new();
        let (a, b) = hub.create().unwrap();
        hub.post(a, b"ping".to_vec()).unwrap();
        assert_eq!(hub.recv(b).await.unwrap(), Some(b"ping".to_vec()));
        // Nothing came back the other way: closing `a` ends its own pump with
        // an empty queue.
        hub.close(a);
        assert_eq!(hub.recv(a).await.unwrap(), None);
    }

    #[tokio::test]
    async fn detaching_a_reader_leaves_queued_messages_queued() {
        // The transfer case: one agent stops reading, another picks the port up
        // and must still find what was already in flight.
        let hub = ProcessPortHub::new();
        let (a, b) = hub.create().unwrap();
        hub.post(a, b"in flight".to_vec()).unwrap();

        hub.detach_reader(b);
        // A reader that parks *after* the detach still gets the message: the
        // notify only cancels a pump that is already waiting.
        assert_eq!(hub.recv(b).await.unwrap(), Some(b"in flight".to_vec()));
    }

    #[tokio::test]
    async fn a_parked_reader_is_woken_by_a_detach_without_eating_a_message() {
        let hub = Arc::new(ProcessPortHub::new());
        let (a, b) = hub.create().unwrap();

        let reader = {
            let hub = hub.clone();
            tokio::spawn(async move { hub.recv(b).await.unwrap() })
        };
        // Let the reader park before detaching it.
        tokio::task::yield_now().await;
        hub.detach_reader(b);
        assert_eq!(reader.await.unwrap(), None);

        // The port still works for whoever holds it next.
        hub.post(a, b"later".to_vec()).unwrap();
        assert_eq!(hub.recv(b).await.unwrap(), Some(b"later".to_vec()));
    }

    #[tokio::test]
    async fn closing_one_end_disentangles_the_other() {
        let hub = ProcessPortHub::new();
        let (a, b) = hub.create().unwrap();
        hub.close(a);
        // `b` is still open, but its posts now reach nobody.
        hub.post(b, b"lost".to_vec()).unwrap();
        hub.close(b);
        assert_eq!(hub.recv(b).await.unwrap(), None);
    }

    #[test]
    fn port_ids_are_unguessable_and_js_safe() {
        // Holding a port id is the authority to use the port, so an agent that
        // was never handed one must not be able to arrive at a live id by
        // counting. Small numbers name nothing, and no two pairs are adjacent.
        let hub = ProcessPortHub::new();
        let mut seen = Vec::new();
        for _ in 0..64 {
            let (a, b) = hub.create().unwrap();
            for id in [a, b] {
                assert!(id > 1 << 24, "{id} is small enough to be searched");
                // Survives the trip through a JS number.
                assert_eq!(id as f64 as u64, id, "{id} does not round-trip");
                assert!(!seen.contains(&id), "{id} was minted twice");
                seen.push(id);
            }
            assert_ne!(a + 1, b, "the peer follows from the port");
        }
    }
}
