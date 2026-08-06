//! The reference [`PortHub`]: `MessagePort` queues owned by the process.
//!
//! Each port is one inbox and a pointer to its peer. `post` writes to the
//! peer's inbox; the agent currently holding the port reads its own. Because
//! the queues live here rather than in an isolate, transferring a port is just
//! moving its id — the messages already in flight stay exactly where they were.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
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
    next_id: AtomicU64,
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

impl PortHub for ProcessPortHub {
    fn create(&self) -> Result<(u64, u64), ProviderError> {
        match self.ports.lock() {
            Ok(mut ports) => {
                let a = self.next_id.fetch_add(2, Ordering::SeqCst) + 1;
                let b = a + 1;
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
}
