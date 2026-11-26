//! Signals: watching, dispatch and the capability that gates them.

use super::*;

/// A [`Signals`] that delivers only what a test hands it — no OS involved,
/// so signal dispatch is exercised deterministically.
#[derive(Clone, Default)]
struct TestSignals {
    watched: Arc<Mutex<Vec<es_runtime_providers::Signal>>>,
    queued: Arc<Mutex<Vec<es_runtime_providers::Signal>>>,
}

impl TestSignals {
    /// Queues `signal` for the next `next()`, as the OS would.
    fn deliver(&self, signal: es_runtime_providers::Signal) {
        self.queued.lock().unwrap().push(signal);
    }
    fn is_watched(&self, signal: es_runtime_providers::Signal) -> bool {
        self.watched.lock().unwrap().contains(&signal)
    }
}

impl es_runtime_providers::Signals for TestSignals {
    fn available(&self) -> Vec<es_runtime_providers::Signal> {
        use es_runtime_providers::Signal as S;
        vec![S::Int, S::Term, S::Hup]
    }

    fn watch(
        &self,
        signal: es_runtime_providers::Signal,
    ) -> std::result::Result<(), es_runtime_providers::ProviderError> {
        if !self.available().contains(&signal) {
            return Err(es_runtime_providers::ProviderError::Other(format!(
                "{} unavailable",
                signal.name()
            )));
        }
        let mut watched = self.watched.lock().unwrap();
        if !watched.contains(&signal) {
            watched.push(signal);
        }
        Ok(())
    }

    fn unwatch(&self, signal: es_runtime_providers::Signal) {
        self.watched.lock().unwrap().retain(|s| *s != signal);
    }

    fn next(&self) -> es_runtime_providers::BoxFuture<Option<es_runtime_providers::Signal>> {
        // Synchronous by construction: a queued delivery comes straight
        // back, and an empty queue ends the pump rather than parking it, so
        // a test never depends on wall-clock timing.
        let watched = self.watched.lock().unwrap().clone();
        let mut queued = self.queued.lock().unwrap();
        let next = queued
            .iter()
            .position(|s| watched.contains(s))
            .map(|i| queued.remove(i));
        Box::pin(std::future::ready(next))
    }
}

fn signal_runtime(signals: Arc<TestSignals>) -> Runtime {
    let engine = V8Engine::new(Limits::default()).expect("engine");
    let providers = HostProviders::new(
        Arc::new(FixedClock {
            monotonic: 0,
            wall: 0,
        }),
        Arc::new(TestConsole::default()),
        Arc::new(MockNet::stub()),
        Arc::new(TestEntropy::new()),
    )
    .with_signals(signals)
    .with_process(Arc::new(StubProcess));
    Runtime::new(Box::new(engine), providers).expect("runtime")
}

#[test]
fn signal_ops_require_the_signals_capability() {
    let _g = v8_guard();
    let mut rt = signal_runtime(Arc::new(TestSignals::default()));
    rt.expose_ops_for_testing();
    // Everything except the capability under test, so a denial can only be
    // Signals — not some other gate the op happens to trip first.
    let mut caps = CapabilitySet::all();
    caps.revoke(Capability::Signals);
    rt.set_capabilities(caps);
    let out = eval_async(
        &mut rt,
        "try { __ops.signal_watch('SIGTERM'); return 'no throw'; } catch (e) { return e.code; }",
    );
    assert_eq!(out, Value::String("ERR_CAPABILITY_DENIED".into()));
}

#[test]
fn env_capability_alone_does_not_grant_signals() {
    // The whole reason Signals is its own capability: reading process state
    // must not carry the privilege to suppress termination.
    let _g = v8_guard();
    let mut rt = signal_runtime(Arc::new(TestSignals::default()));
    rt.expose_ops_for_testing();
    rt.set_capabilities(CapabilitySet::none().with(Capability::Env));
    let out = eval_async(
        &mut rt,
        "try { __ops.signal_watch('SIGTERM'); return 'no throw'; } catch (e) { return e.code; }",
    );
    assert_eq!(out, Value::String("ERR_CAPABILITY_DENIED".into()));
}

/// `runtime:process` is a module, so these go through the module path (a
/// bare `eval` cannot import) and read their answer back off `globalThis`.
fn run_signal_module(signals: Arc<TestSignals>, source: &str) -> (Runtime, ModuleEvalState) {
    let mut rt = signal_runtime(signals);
    let state = run_module(&mut rt, source, MapLoader::new(&[]));
    (rt, state)
}

#[test]
fn a_watched_signal_reaches_its_handler() {
    let _g = v8_guard();
    let signals = Arc::new(TestSignals::default());
    signals.deliver(es_runtime_providers::Signal::Term);
    let (mut rt, state) = run_signal_module(
        signals,
        "import { onSignal } from 'runtime:process'; \
         globalThis.result = 'none'; \
         onSignal('SIGTERM', (name) => { globalThis.result = name; });",
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(
        rt.eval("globalThis.result").unwrap(),
        Value::String("SIGTERM".into())
    );
}

#[test]
fn a_signal_that_is_not_watched_is_never_dispatched() {
    let _g = v8_guard();
    let signals = Arc::new(TestSignals::default());
    signals.deliver(es_runtime_providers::Signal::Int); // nobody asked for it
    let (mut rt, state) = run_signal_module(
        signals,
        "import { onSignal } from 'runtime:process'; \
         globalThis.result = 'none'; \
         onSignal('SIGTERM', (name) => { globalThis.result = name; });",
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(
        rt.eval("globalThis.result").unwrap(),
        Value::String("none".into())
    );
}

#[test]
fn the_watch_lasts_until_the_last_handler_is_removed() {
    let _g = v8_guard();
    let signals = Arc::new(TestSignals::default());
    let (_rt, state) = run_signal_module(
        signals.clone(),
        "import { onSignal, offSignal } from 'runtime:process'; \
         const a = () => {}; const b = () => {}; \
         onSignal('SIGHUP', a); onSignal('SIGHUP', b); \
         offSignal('SIGHUP', a); \
         globalThis.stillWatched = true; \
         offSignal('SIGHUP', b);",
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert!(
        !signals.is_watched(es_runtime_providers::Signal::Hup),
        "the last handler was removed, so the watch must be dropped"
    );
}

#[test]
fn one_handler_of_several_keeps_the_watch() {
    let _g = v8_guard();
    let signals = Arc::new(TestSignals::default());
    let (_rt, state) = run_signal_module(
        signals.clone(),
        "import { onSignal, offSignal } from 'runtime:process'; \
         const a = () => {}; const b = () => {}; \
         onSignal('SIGHUP', a); onSignal('SIGHUP', b); \
         offSignal('SIGHUP', a);",
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert!(
        signals.is_watched(es_runtime_providers::Signal::Hup),
        "one handler remains, so the watch must stay"
    );
}

#[test]
fn a_signal_the_platform_lacks_throws_rather_than_registering() {
    // A handler that could never fire is worse than a clear failure.
    let _g = v8_guard();
    let (mut rt, state) = run_signal_module(
        Arc::new(TestSignals::default()),
        "import { onSignal } from 'runtime:process'; \
         try { onSignal('SIGUSR1', () => {}); globalThis.result = 'no throw'; } \
         catch (e) { globalThis.result = 'threw'; }",
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(
        rt.eval("globalThis.result").unwrap(),
        Value::String("threw".into())
    );
}

#[test]
fn an_unknown_signal_name_is_a_type_error() {
    let _g = v8_guard();
    let (mut rt, state) = run_signal_module(
        Arc::new(TestSignals::default()),
        "import { onSignal } from 'runtime:process'; \
         try { onSignal('SIGNOPE', () => {}); globalThis.result = 'no throw'; } \
         catch (e) { globalThis.result = e.constructor.name; }",
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(
        rt.eval("globalThis.result").unwrap(),
        Value::String("TypeError".into())
    );
}

#[test]
fn signals_reports_what_the_platform_can_deliver() {
    let _g = v8_guard();
    let (mut rt, state) = run_signal_module(
        Arc::new(TestSignals::default()),
        "import { signals } from 'runtime:process'; \
         globalThis.result = signals().join(',');",
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(
        rt.eval("globalThis.result").unwrap(),
        Value::String("SIGINT,SIGTERM,SIGHUP".into())
    );
}
