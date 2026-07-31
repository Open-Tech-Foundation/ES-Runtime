//! OS-backed [`Signals`] — tokio signal streams behind the pull-based seam.
//!
//! The runtime owns no loop and no thread, so it cannot be *called* when a
//! signal arrives; it asks for the next one and awaits. That inverts tokio's
//! push-shaped `Signal` streams, so each watched signal gets a small forwarding
//! task that pushes deliveries into one shared channel, and
//! [`next`](Signals::next) drains that channel.
//!
//! Coalescing is deliberate: the channel is bounded at one slot per signal kind,
//! so a burst of `SIGHUP`s while the guest is busy handling the first is
//! delivered once, not queued. Signals are edge notifications — "a reload was
//! asked for" — and replaying a backlog of them helps nobody.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use es_runtime_common::ErrorCode;
use es_runtime_providers::{BoxFuture, ProviderError, Signal, Signals};
use tokio::sync::{Notify, mpsc};
use tokio::task::AbortHandle;

/// A [`Signals`] over `tokio::signal`. Cloneable; clones share the registry.
pub struct SystemSignals {
    /// Forwarding task per watched signal, so `unwatch` can stop one.
    watched: Arc<Mutex<HashMap<Signal, AbortHandle>>>,
    tx: mpsc::Sender<Signal>,
    /// Taken by whichever call to `next` is in flight, then put back — no lock
    /// is held across the await (the same shape as `SystemHttpServer`).
    rx: Arc<Mutex<Option<mpsc::Receiver<Signal>>>>,
    /// Wakes a parked `next` when the last watch is dropped. Without it, a guest
    /// that unwatched everything would leave its pump awaiting a delivery that
    /// can no longer come — holding the driven loop open for good.
    released: Arc<Notify>,
}

impl Default for SystemSignals {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemSignals {
    /// Builds a registry watching nothing.
    pub fn new() -> Self {
        // One slot per signal kind: enough that a delivery is never lost,
        // small enough that a storm coalesces (see the module docs).
        let (tx, rx) = mpsc::channel(8);
        SystemSignals {
            watched: Arc::new(Mutex::new(HashMap::new())),
            tx,
            rx: Arc::new(Mutex::new(Some(rx))),
            released: Arc::new(Notify::new()),
        }
    }

    /// Whether the guest is watching `signal`.
    ///
    /// An embedder that also watches a signal for its own shutdown handling asks
    /// this to decide whether to act: a guest that installed a handler has taken
    /// responsibility, and a host default that fired anyway would race it.
    pub fn is_watched(&self, signal: Signal) -> bool {
        self.watched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&signal)
    }

    fn unsupported(signal: Signal) -> ProviderError {
        ProviderError::Coded {
            code: ErrorCode::ProviderUnavailable,
            message: format!(
                "{} is not available on this platform (available: {})",
                signal.name(),
                AVAILABLE
                    .iter()
                    .map(|s| s.name())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        }
    }
}

/// The signals this platform can deliver. Unix gets the full set; Windows has
/// no `SIGTERM`/`SIGHUP`/`SIGUSR*` to give, so it offers what its console API
/// actually has rather than emulating names that would never fire.
#[cfg(unix)]
const AVAILABLE: &[Signal] = &[
    Signal::Int,
    Signal::Term,
    Signal::Hup,
    Signal::Usr1,
    Signal::Usr2,
];
#[cfg(windows)]
const AVAILABLE: &[Signal] = &[Signal::Int, Signal::Break];
#[cfg(not(any(unix, windows)))]
const AVAILABLE: &[Signal] = &[];

/// Spawns the forwarding task for `signal`, translating tokio's push-shaped
/// stream into sends on the shared channel. `try_send` rather than `send`: a
/// full channel means a delivery of this kind is already queued and unread, and
/// coalescing onto it is the intended behaviour, not a dropped event.
#[cfg(unix)]
fn spawn_forwarder(signal: Signal, tx: mpsc::Sender<Signal>) -> Result<AbortHandle, ProviderError> {
    use tokio::signal::unix::{SignalKind, signal as unix_signal};
    let kind = match signal {
        Signal::Int => SignalKind::interrupt(),
        Signal::Term => SignalKind::terminate(),
        Signal::Hup => SignalKind::hangup(),
        Signal::Usr1 => SignalKind::user_defined1(),
        Signal::Usr2 => SignalKind::user_defined2(),
        // Windows-only, plus the two send-only signals: `SIGKILL` cannot be
        // caught at all and `SIGQUIT` is not offered for interception — they
        // exist so a child process can be killed, never watched.
        Signal::Break | Signal::Kill | Signal::Quit => {
            return Err(SystemSignals::unsupported(signal));
        }
    };
    let mut stream = unix_signal(kind).map_err(|e| ProviderError::Coded {
        code: ErrorCode::from_io_kind(e.kind()),
        message: format!("cannot watch {}: {e}", signal.name()),
    })?;
    Ok(tokio::spawn(async move {
        while stream.recv().await.is_some() {
            let _ = tx.try_send(signal);
        }
    })
    .abort_handle())
}

#[cfg(windows)]
fn spawn_forwarder(signal: Signal, tx: mpsc::Sender<Signal>) -> Result<AbortHandle, ProviderError> {
    use tokio::signal::windows::{ctrl_break, ctrl_c};
    let map_err = |e: std::io::Error| ProviderError::Coded {
        code: ErrorCode::from_io_kind(e.kind()),
        message: format!("cannot watch {}: {e}", signal.name()),
    };
    // The two console events have distinct types, so each arm owns its loop.
    let handle = match signal {
        Signal::Int => {
            let mut stream = ctrl_c().map_err(map_err)?;
            tokio::spawn(async move {
                while stream.recv().await.is_some() {
                    let _ = tx.try_send(signal);
                }
            })
            .abort_handle()
        }
        Signal::Break => {
            let mut stream = ctrl_break().map_err(map_err)?;
            tokio::spawn(async move {
                while stream.recv().await.is_some() {
                    let _ = tx.try_send(signal);
                }
            })
            .abort_handle()
        }
        other => return Err(SystemSignals::unsupported(other)),
    };
    Ok(handle)
}

#[cfg(not(any(unix, windows)))]
fn spawn_forwarder(
    signal: Signal,
    _tx: mpsc::Sender<Signal>,
) -> Result<AbortHandle, ProviderError> {
    Err(SystemSignals::unsupported(signal))
}

impl Signals for SystemSignals {
    fn available(&self) -> Vec<Signal> {
        AVAILABLE.to_vec()
    }

    fn watch(&self, signal: Signal) -> Result<(), ProviderError> {
        if !AVAILABLE.contains(&signal) {
            return Err(SystemSignals::unsupported(signal));
        }
        let mut watched = self.watched.lock().unwrap_or_else(|e| e.into_inner());
        if watched.contains_key(&signal) {
            return Ok(()); // idempotent
        }
        watched.insert(signal, spawn_forwarder(signal, self.tx.clone())?);
        Ok(())
    }

    fn unwatch(&self, signal: Signal) {
        let mut watched = self.watched.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(handle) = watched.remove(&signal) {
            handle.abort();
        }
        if watched.is_empty() {
            // `notify_one` rather than `notify_waiters`: it stores a permit, so
            // a `next` that has checked the map but not yet awaited is still
            // released instead of losing the wakeup.
            self.released.notify_one();
        }
    }

    fn next(&self) -> BoxFuture<Option<Signal>> {
        // Nothing watched: release the caller rather than parking it forever.
        if self
            .watched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
        {
            return Box::pin(std::future::ready(None));
        }
        let slot = self.rx.clone();
        let taken = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        let released = self.released.clone();
        Box::pin(async move {
            let Some(mut rx) = taken else {
                // A `next` is already in flight; only one caller is expected, so
                // this is a misuse rather than a state to queue behind.
                return None;
            };
            let got = tokio::select! {
                biased;
                signal = rx.recv() => signal,
                () = released.notified() => None,
            };
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(rx);
            got
        })
    }
}

/// A [`Signals`] that delivers only what a test hands it — no OS involvement,
/// so signal-driven behaviour is testable and deterministic.
#[derive(Clone)]
pub struct ManualSignals {
    watched: Arc<Mutex<Vec<Signal>>>,
    tx: mpsc::Sender<Signal>,
    rx: Arc<Mutex<Option<mpsc::Receiver<Signal>>>>,
    released: Arc<Notify>,
}

impl Default for ManualSignals {
    fn default() -> Self {
        Self::new()
    }
}

impl ManualSignals {
    /// Builds a registry watching nothing.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(8);
        ManualSignals {
            watched: Arc::new(Mutex::new(Vec::new())),
            tx,
            rx: Arc::new(Mutex::new(Some(rx))),
            released: Arc::new(Notify::new()),
        }
    }

    /// Delivers `signal` as though the OS had. Ignored if it is not watched,
    /// mirroring a real signal whose default action already ran.
    pub fn deliver(&self, signal: Signal) {
        if self
            .watched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&signal)
        {
            let _ = self.tx.try_send(signal);
        }
    }

    /// Whether `signal` is currently watched.
    pub fn is_watched(&self, signal: Signal) -> bool {
        self.watched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&signal)
    }
}

impl Signals for ManualSignals {
    fn available(&self) -> Vec<Signal> {
        vec![
            Signal::Int,
            Signal::Term,
            Signal::Hup,
            Signal::Usr1,
            Signal::Usr2,
        ]
    }

    fn watch(&self, signal: Signal) -> Result<(), ProviderError> {
        if !self.available().contains(&signal) {
            return Err(SystemSignals::unsupported(signal));
        }
        let mut watched = self.watched.lock().unwrap_or_else(|e| e.into_inner());
        if !watched.contains(&signal) {
            watched.push(signal);
        }
        Ok(())
    }

    fn unwatch(&self, signal: Signal) {
        let mut watched = self.watched.lock().unwrap_or_else(|e| e.into_inner());
        watched.retain(|s| *s != signal);
        if watched.is_empty() {
            self.released.notify_one();
        }
    }

    fn next(&self) -> BoxFuture<Option<Signal>> {
        if self
            .watched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
        {
            return Box::pin(std::future::ready(None));
        }
        let slot = self.rx.clone();
        let taken = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
        let released = self.released.clone();
        Box::pin(async move {
            let mut rx = taken?; // a `next` is already in flight
            let got = tokio::select! {
                biased;
                signal = rx.recv() => signal,
                () = released.notified() => None,
            };
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(rx);
            got
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn next_returns_none_when_nothing_is_watched() {
        let signals = ManualSignals::new();
        assert_eq!(signals.next().await, None, "must release, not park");
    }

    #[tokio::test]
    async fn a_watched_signal_is_delivered() {
        let signals = ManualSignals::new();
        signals.watch(Signal::Term).unwrap();
        signals.deliver(Signal::Term);
        assert_eq!(signals.next().await, Some(Signal::Term));
    }

    #[tokio::test]
    async fn an_unwatched_signal_is_not_delivered() {
        let signals = ManualSignals::new();
        signals.watch(Signal::Term).unwrap();
        signals.deliver(Signal::Int); // never watched
        signals.deliver(Signal::Term);
        // The SIGINT was dropped, so the first thing out is the SIGTERM.
        assert_eq!(signals.next().await, Some(Signal::Term));
    }

    #[tokio::test]
    async fn watch_is_idempotent_and_unwatch_stops_delivery() {
        let signals = ManualSignals::new();
        signals.watch(Signal::Hup).unwrap();
        signals.watch(Signal::Hup).unwrap();
        assert!(signals.is_watched(Signal::Hup));
        signals.unwatch(Signal::Hup);
        signals.unwatch(Signal::Hup); // idempotent
        assert!(!signals.is_watched(Signal::Hup));
        signals.deliver(Signal::Hup);
        assert_eq!(signals.next().await, None, "nothing watched any more");
    }

    #[tokio::test]
    async fn a_signal_the_platform_lacks_is_a_clear_error() {
        let signals = ManualSignals::new();
        let err = signals.watch(Signal::Break).unwrap_err();
        assert!(
            err.to_string().contains("SIGBREAK"),
            "the error names the signal asked for: {err}"
        );
    }

    /// The pump must be released when the last watch goes, even though it is
    /// already parked on a delivery that can no longer arrive — otherwise a
    /// guest that stopped listening would hold the driven loop open for good.
    #[tokio::test]
    async fn unwatching_the_last_signal_releases_a_parked_next() {
        let signals = ManualSignals::new();
        signals.watch(Signal::Term).unwrap();
        let parked = {
            let signals = signals.clone();
            tokio::spawn(async move { signals.next().await })
        };
        // Let the pump actually park before the watch is dropped: releasing a
        // future that has not awaited yet would not prove anything.
        tokio::task::yield_now().await;
        signals.unwatch(Signal::Term);
        let got = tokio::time::timeout(std::time::Duration::from_secs(5), parked)
            .await
            .expect("a parked next must be released, not left waiting")
            .unwrap();
        assert_eq!(got, None);
    }

    /// The real provider must at least register and release on the host CI runs
    /// on — proving the tokio wiring, without asking the test to raise a signal
    /// at its own process.
    #[tokio::test]
    async fn the_system_provider_watches_and_unwatches() {
        let signals = SystemSignals::new();
        let first = *signals
            .available()
            .first()
            .expect("a supported platform offers at least one signal");
        signals.watch(first).unwrap();
        signals.watch(first).unwrap(); // idempotent
        signals.unwatch(first);
        assert_eq!(signals.next().await, None, "unwatched ⇒ released");
    }
}
