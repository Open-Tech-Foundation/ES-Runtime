//! Graceful shutdown on `^C` / `SIGTERM`, shared by every binary that can end
//! up holding an HTTP server.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

use es_runtime::InterruptHandle;
use es_runtime_default_providers::{SystemHttpServer, SystemSignals};
use es_runtime_providers::Signal;

/// The exit code a completed graceful shutdown should use, or `0` if no
/// interrupt was handled. Read once the drive loop returns.
pub static SHUTDOWN_CODE: AtomicI32 = AtomicI32::new(0);

/// Watches for an interrupt and, if the guest has not taken responsibility for
/// it, drains the HTTP servers instead of letting the process be killed.
///
/// The three-way split is the whole design:
///
/// * **The guest is watching this signal** — it installed a handler, so it owns
///   shutdown. Do nothing; racing its handler would be worse than useless.
/// * **No server is running** — there is no in-flight request to protect, so
///   exit at once. A script with a `setInterval` should still die instantly on
///   `^C`; waiting out a grace period there would be a regression, not a
///   feature.
/// * **Servers are running** — stop accepting, let in-flight requests answer,
///   and exit with the conventional 128+signal once they drain. `grace` is the
///   backstop for a handler that never finishes.
///
/// A second interrupt during the drain exits immediately: someone pressing `^C`
/// twice means it, and the first press has already been given its chance.
///
/// `bin` names the binary in the drain notice, so the line a user sees says
/// which program is holding the process open.
pub fn spawn_shutdown_watcher(
    bin: &'static str,
    signals: Arc<SystemSignals>,
    http: Arc<SystemHttpServer>,
    interrupt: InterruptHandle,
    grace: Duration,
) {
    let draining = Arc::new(AtomicBool::new(false));
    for signal in [Signal::Int, Signal::Term] {
        // Watching here also suppresses the default action, which is the point:
        // the process must survive long enough to drain. A platform that cannot
        // deliver this signal simply gets no watcher.
        let Some(mut stream) = watch_process_signal(signal) else {
            continue;
        };
        let (signals, http, interrupt, draining) = (
            signals.clone(),
            http.clone(),
            interrupt.clone(),
            draining.clone(),
        );
        tokio::spawn(async move {
            while stream.recv().await.is_some() {
                // The guest asked for this signal: its handler is the shutdown.
                if signals.is_watched(signal) {
                    continue;
                }
                if draining.swap(true, Ordering::SeqCst) {
                    // Second interrupt while draining — stop waiting.
                    std::process::exit(signal.exit_code());
                }
                if http.shutdown_all() == 0 {
                    // Nothing in flight to protect; behave as the default action
                    // would have.
                    std::process::exit(signal.exit_code());
                }
                eprintln!(
                    "{bin}: {} received, draining in-flight requests (up to {}ms)",
                    signal.name(),
                    grace.as_millis()
                );
                // Backstop: a handler that never finishes must not outlive the
                // grace. Terminating the engine unblocks the drive loop, and the
                // exit code is the same either way.
                let handle = interrupt.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(grace).await;
                    handle.terminate();
                    std::process::exit(signal.exit_code());
                });
                // The drive loop reaches quiescence once the servers have
                // drained; record the code it should exit with.
                SHUTDOWN_CODE.store(signal.exit_code(), Ordering::SeqCst);
            }
        });
    }
}

/// A process-level stream for `signal`, or `None` where the platform has no such
/// signal to deliver. Separate from the guest's `Signals` provider on purpose:
/// this one is the *host's* shutdown behaviour, and both can watch the same
/// signal without competing for deliveries.
#[cfg(unix)]
fn watch_process_signal(signal: Signal) -> Option<tokio::signal::unix::Signal> {
    use tokio::signal::unix::{SignalKind, signal as unix_signal};
    let kind = match signal {
        Signal::Int => SignalKind::interrupt(),
        Signal::Term => SignalKind::terminate(),
        _ => return None,
    };
    unix_signal(kind).ok()
}

#[cfg(windows)]
fn watch_process_signal(signal: Signal) -> Option<tokio::signal::windows::CtrlC> {
    // Windows has no SIGTERM; Ctrl+C is the interrupt that exists.
    match signal {
        Signal::Int => tokio::signal::windows::ctrl_c().ok(),
        _ => None,
    }
}
