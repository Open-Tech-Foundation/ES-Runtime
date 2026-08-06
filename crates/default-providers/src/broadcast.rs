//! The reference [`BroadcastHub`]: one process, one agent cluster.
//!
//! Every agent this runtime starts lives in this process, so the cluster the
//! spec scopes `BroadcastChannel` to is exactly the set of subscriptions held
//! here. A host that spread agents wider would replace this with something that
//! carried the message further; nothing above this file would change.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use es_runtime_providers::{BoxFuture, BroadcastHub, ProviderError};
use tokio::sync::mpsc;

/// One open `BroadcastChannel`.
struct Subscription {
    name: String,
    inbox: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<Vec<u8>>>>,
    deliver: mpsc::UnboundedSender<Vec<u8>>,
}

/// A [`BroadcastHub`] that delivers within the current process.
#[derive(Default)]
pub struct ProcessBroadcastHub {
    subscriptions: Mutex<HashMap<u64, Subscription>>,
    next_id: AtomicU64,
}

impl ProcessBroadcastHub {
    /// Creates an empty hub.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl BroadcastHub for ProcessBroadcastHub {
    fn subscribe(&self, name: String) -> BoxFuture<Result<u64, ProviderError>> {
        let (deliver, inbox) = mpsc::unbounded_channel();
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let result = match self.subscriptions.lock() {
            Ok(mut subs) => {
                subs.insert(
                    id,
                    Subscription {
                        name,
                        inbox: Arc::new(tokio::sync::Mutex::new(inbox)),
                        deliver,
                    },
                );
                Ok(id)
            }
            Err(_) => Err(ProviderError::Other("broadcast registry poisoned".into())),
        };
        Box::pin(async move { result })
    }

    fn publish(&self, id: u64, message: Vec<u8>) -> BoxFuture<Result<(), ProviderError>> {
        let result = match self.subscriptions.lock() {
            Ok(subs) => {
                // The sender's own name decides who hears it; posting from a
                // closed or unknown id simply reaches nobody.
                if let Some(from) = subs.get(&id) {
                    for (other, sub) in subs.iter() {
                        // "every channel of the same name except this one" —
                        // a channel never receives its own posts, in this agent
                        // or any other.
                        if *other != id && sub.name == from.name {
                            let _ = sub.deliver.send(message.clone());
                        }
                    }
                }
                Ok(())
            }
            Err(_) => Err(ProviderError::Other("broadcast registry poisoned".into())),
        };
        Box::pin(async move { result })
    }

    fn recv(&self, id: u64) -> BoxFuture<Result<Option<Vec<u8>>, ProviderError>> {
        let inbox = self
            .subscriptions
            .lock()
            .ok()
            .and_then(|subs| subs.get(&id).map(|sub| sub.inbox.clone()));
        Box::pin(async move {
            let Some(inbox) = inbox else { return Ok(None) };
            Ok(inbox.lock().await.recv().await)
        })
    }

    fn close(&self, id: u64) -> BoxFuture<Result<(), ProviderError>> {
        // Dropping the subscription drops its sender, which ends the pump
        // parked in `recv` above.
        if let Ok(mut subs) = self.subscriptions.lock() {
            subs.remove(&id);
        }
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_post_reaches_peers_of_the_same_name_but_not_the_sender() {
        let hub = ProcessBroadcastHub::new();
        let a = hub.subscribe("news".into()).await.unwrap();
        let b = hub.subscribe("news".into()).await.unwrap();
        let elsewhere = hub.subscribe("other".into()).await.unwrap();

        hub.publish(a, b"hello".to_vec()).await.unwrap();

        assert_eq!(hub.recv(b).await.unwrap(), Some(b"hello".to_vec()));
        // The sender hears nothing, and neither does a different name. Proven
        // by closing both and seeing the pumps end with an empty queue.
        hub.close(a).await.unwrap();
        hub.close(elsewhere).await.unwrap();
        assert_eq!(hub.recv(a).await.unwrap(), None);
        assert_eq!(hub.recv(elsewhere).await.unwrap(), None);
    }

    #[tokio::test]
    async fn closing_ends_the_pump() {
        let hub = ProcessBroadcastHub::new();
        let a = hub.subscribe("x".into()).await.unwrap();
        hub.close(a).await.unwrap();
        assert_eq!(hub.recv(a).await.unwrap(), None);
        // Idempotent.
        hub.close(a).await.unwrap();
    }
}
