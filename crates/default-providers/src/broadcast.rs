//! The reference [`BroadcastHub`]: one process, one agent cluster.
//!
//! Every agent this runtime starts lives in this process, so the cluster the
//! spec scopes `BroadcastChannel` to is exactly the set of subscriptions held
//! here. A host that spread agents wider would replace this with something that
//! carried the message further; nothing above this file would change.
//!
//! # Why delivery is per *agent* rather than per channel
//!
//! The spec's channels share one event loop, so a post reaches the other
//! channels of that name as tasks on a single queue: every destination of the
//! first post is delivered before any destination of the second, and within a
//! post, in the order the channels were created.
//!
//! A queue per channel cannot reproduce that. Three channels in one agent would
//! each be waiting on their own receive, and the order they came back in would
//! be the order their futures happened to be polled — not the order the
//! messages were published. So the queue is per agent: one ordered stream, and
//! [`recv_next`](BroadcastHub::recv_next) hands back whichever message was
//! published first, along with the subscription it is for.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;

use es_runtime_providers::{BoxFuture, Broadcast, BroadcastHub, ProviderError};
use tokio::sync::Notify;

/// One open `BroadcastChannel`.
struct Subscription {
    name: String,
    /// The agent holding it. One agent per thread is this host's model — the
    /// same identification [`ThreadWorkerHost`](crate::ThreadWorkerHost) makes —
    /// and it is what groups subscriptions into one ordered stream.
    agent: ThreadId,
    /// Published messages waiting for this subscription, each stamped with the
    /// hub-wide sequence it was published at. The stamp is what lets the agent's
    /// stream interleave its channels in publication order.
    queue: VecDeque<(u64, Vec<u8>)>,
}

/// What one round of [`ProcessBroadcastHub::take_next`] found.
enum Next {
    /// A message, and the subscription it is for.
    Message(u64, Vec<u8>),
    /// Subscriptions are open but none has anything yet: park.
    Idle,
    /// The agent holds no open subscription; its receive is over.
    Done,
}

/// The subscription table, shared with the futures `recv_next` hands out —
/// [`BoxFuture`] is `'static`, so a future cannot borrow the hub.
type Registry = Arc<Mutex<BTreeMap<u64, Subscription>>>;

/// A [`BroadcastHub`] that delivers within the current process.
#[derive(Default)]
pub struct ProcessBroadcastHub {
    /// Ordered by id, which is creation order: a broadcast reaches the other
    /// channels of that name in the order they were opened, and a `HashMap`
    /// iteration would have been whatever the hasher felt like.
    subscriptions: Registry,
    next_id: AtomicU64,
    /// Stamps every *delivery*, so an agent can order the messages waiting
    /// across all of its channels.
    next_sequence: AtomicU64,
    /// Wakes the parked receives. One for the whole hub: a wake is cheap and a
    /// receive re-checks its own subscriptions anyway.
    arrived: Arc<Notify>,
}

impl ProcessBroadcastHub {
    /// Creates an empty hub.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The earliest message waiting for `agent`, removed from its queue.
    ///
    /// Earliest by publication, not by channel: that is the whole point of the
    /// per-delivery stamp, and what makes three channels in one agent deliver
    /// in the order a single task queue would.
    fn take_next(registry: &Registry, agent: ThreadId) -> Result<Next, ProviderError> {
        let mut subs = registry
            .lock()
            .map_err(|_| ProviderError::Other("broadcast registry poisoned".into()))?;
        let mut earliest: Option<(u64, u64)> = None; // (sequence, subscription)
        let mut open = false;
        for (id, sub) in subs.iter() {
            if sub.agent != agent {
                continue;
            }
            open = true;
            if let Some(&(sequence, _)) = sub.queue.front()
                && earliest.is_none_or(|(best, _)| sequence < best)
            {
                earliest = Some((sequence, *id));
            }
        }
        if !open {
            return Ok(Next::Done);
        }
        let Some((_, id)) = earliest else {
            return Ok(Next::Idle);
        };
        match subs.get_mut(&id).and_then(|sub| sub.queue.pop_front()) {
            Some((_, message)) => Ok(Next::Message(id, message)),
            None => Ok(Next::Idle),
        }
    }
}

impl BroadcastHub for ProcessBroadcastHub {
    fn subscribe(&self, name: String) -> Result<u64, ProviderError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        match self.subscriptions.lock() {
            Ok(mut subs) => {
                subs.insert(
                    id,
                    Subscription {
                        name,
                        agent: std::thread::current().id(),
                        queue: VecDeque::new(),
                    },
                );
                Ok(id)
            }
            Err(_) => Err(ProviderError::Other("broadcast registry poisoned".into())),
        }
    }

    fn publish(&self, id: u64, message: Vec<u8>) -> BoxFuture<Result<(), ProviderError>> {
        let result = match self.subscriptions.lock() {
            Ok(mut subs) => {
                // The sender's own name decides who hears it; posting from a
                // closed or unknown id simply reaches nobody.
                let Some(name) = subs.get(&id).map(|from| from.name.clone()) else {
                    return Box::pin(async { Ok(()) });
                };
                // In id order, which is creation order — the order the spec
                // delivers in — and each stamped so the receiving agent can
                // keep that order across its own channels.
                let targets: Vec<u64> = subs
                    .iter()
                    .filter(|(other, sub)| **other != id && sub.name == name)
                    .map(|(other, _)| *other)
                    .collect();
                for target in targets {
                    let sequence = self.next_sequence.fetch_add(1, Ordering::SeqCst);
                    if let Some(sub) = subs.get_mut(&target) {
                        sub.queue.push_back((sequence, message.clone()));
                    }
                }
                Ok(())
            }
            Err(_) => Err(ProviderError::Other("broadcast registry poisoned".into())),
        };
        self.arrived.notify_waiters();
        Box::pin(async move { result })
    }

    fn recv_next(&self) -> BoxFuture<Result<Option<Broadcast>, ProviderError>> {
        // Captured here, on the agent's own thread, rather than inside the
        // future: it is the same thread either way (each agent drives its own
        // current-thread runtime), and taking it at the call is the part that
        // does not depend on that staying true.
        let agent = std::thread::current().id();
        let arrived = self.arrived.clone();
        let registry = self.subscriptions.clone();
        Box::pin(async move {
            loop {
                // Registered *before* the check, so a publish landing between
                // the two is not a lost wake-up.
                let notified = arrived.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();

                match Self::take_next(&registry, agent)? {
                    Next::Message(id, message) => return Ok(Some((id, message))),
                    Next::Done => return Ok(None),
                    Next::Idle => notified.await,
                }
            }
        })
    }

    fn close(&self, id: u64) -> BoxFuture<Result<(), ProviderError>> {
        if let Ok(mut subs) = self.subscriptions.lock() {
            subs.remove(&id);
        }
        // A receive parked for an agent whose last channel just closed has to
        // learn that it is over.
        self.arrived.notify_waiters();
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_post_reaches_peers_of_the_same_name_but_not_the_sender() {
        let hub = ProcessBroadcastHub::new();
        let a = hub.subscribe("news".into()).unwrap();
        let b = hub.subscribe("news".into()).unwrap();
        let elsewhere = hub.subscribe("other".into()).unwrap();

        hub.publish(a, b"hello".to_vec()).await.unwrap();

        assert_eq!(
            hub.recv_next().await.unwrap(),
            Some((b, b"hello".to_vec())),
            "only the peer of the same name hears it"
        );
        hub.close(a).await.unwrap();
        hub.close(b).await.unwrap();
        hub.close(elsewhere).await.unwrap();
        // Nothing open for this agent: the receive ends rather than parking.
        assert_eq!(hub.recv_next().await.unwrap(), None);
    }

    #[tokio::test]
    async fn messages_arrive_in_publication_order_across_channels() {
        // The ordering the spec's single task queue gives: every destination of
        // the first post before any destination of the second, and within a
        // post, in channel creation order.
        let hub = ProcessBroadcastHub::new();
        let c1 = hub.subscribe("order".into()).unwrap();
        let c2 = hub.subscribe("order".into()).unwrap();
        let c3 = hub.subscribe("order".into()).unwrap();

        hub.publish(c1, b"from c1".to_vec()).await.unwrap();
        hub.publish(c3, b"from c3".to_vec()).await.unwrap();
        hub.publish(c2, b"done".to_vec()).await.unwrap();

        let mut seen = Vec::new();
        for _ in 0..6 {
            let (id, bytes) = hub.recv_next().await.unwrap().expect("a message");
            seen.push((id, String::from_utf8(bytes).unwrap()));
        }
        assert_eq!(
            seen,
            vec![
                (c2, "from c1".to_string()),
                (c3, "from c1".to_string()),
                (c1, "from c3".to_string()),
                (c2, "from c3".to_string()),
                (c1, "done".to_string()),
                (c3, "done".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn closing_ends_the_receive() {
        let hub = ProcessBroadcastHub::new();
        let a = hub.subscribe("x".into()).unwrap();
        hub.close(a).await.unwrap();
        assert_eq!(hub.recv_next().await.unwrap(), None);
        // Idempotent.
        hub.close(a).await.unwrap();
    }
}
