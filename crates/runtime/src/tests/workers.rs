//! Workers and isolates at the host seam, MessagePort and BroadcastChannel.

use super::*;

/// A [`WorkerHost`] that starts no threads and runs no isolates.
///
/// The end-to-end worker tests drive the real host through `esrun`, which
/// costs an OS thread and a V8 isolate per case and can only observe what a
/// worker prints. This sees the other side: exactly what the runtime asks
/// the host to do, in order, with nothing in between. It is also the only
/// way to assert what an embedder's own `WorkerHost` will be called with.
#[derive(Default)]
struct TestWorkerHost {
    /// Every call, in order — `spawn`, `post`, `terminate`.
    calls: Arc<std::sync::Mutex<Vec<String>>>,
    /// What each worker's `recv` hands back, oldest first. Drained, so a
    /// worker with nothing left reports `Closed` and is done.
    inbox: Arc<std::sync::Mutex<Vec<WorkerIncoming>>>,
    /// The `WorkerSpec` of the most recent spawn, so a test can assert what
    /// the runtime narrowed before asking for an agent.
    last: Arc<std::sync::Mutex<Option<WorkerSpec>>>,
    /// With nothing queued, `recv` waits instead of reporting the end: a
    /// worker that is alive and idle, for tests about a live one.
    held_open: bool,
}

impl TestWorkerHost {
    fn held_open() -> Self {
        TestWorkerHost {
            held_open: true,
            ..TestWorkerHost::default()
        }
    }

    /// What `recv` has not handed out yet.
    fn unread(&self) -> usize {
        self.inbox.lock().expect("inbox").len()
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("calls").clone()
    }

    fn queue(&self, event: WorkerIncoming) {
        self.inbox.lock().expect("inbox").push(event);
    }

    fn last_spec(&self) -> WorkerSpec {
        self.last
            .lock()
            .expect("last")
            .clone()
            .expect("a worker was spawned")
    }
}

impl WorkerHost for TestWorkerHost {
    fn spawn(
        &self,
        spec: WorkerSpec,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<u64, es_runtime_providers::ProviderError>,
    > {
        self.calls
            .lock()
            .expect("calls")
            .push(format!("spawn {}", spec.specifier));
        *self.last.lock().expect("last") = Some(spec);
        Box::pin(std::future::ready(Ok(1)))
    }

    fn post(
        &self,
        id: u64,
        message: Vec<u8>,
    ) -> std::result::Result<(), es_runtime_providers::ProviderError> {
        self.calls
            .lock()
            .expect("calls")
            .push(format!("post {id} ({} bytes)", message.len()));
        Ok(())
    }

    fn queued(&self, _id: u64) -> usize {
        0
    }

    fn recv(
        &self,
        _id: u64,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<Option<WorkerIncoming>, es_runtime_providers::ProviderError>,
    > {
        let next = {
            let mut inbox = self.inbox.lock().expect("inbox");
            if inbox.is_empty() {
                None
            } else {
                Some(inbox.remove(0))
            }
        };
        if next.is_none() && self.held_open {
            return Box::pin(std::future::pending());
        }
        Box::pin(std::future::ready(Ok(next)))
    }

    fn terminate(
        &self,
        id: u64,
    ) -> es_runtime_providers::BoxFuture<std::result::Result<(), es_runtime_providers::ProviderError>>
    {
        self.calls
            .lock()
            .expect("calls")
            .push(format!("terminate {id}"));
        Box::pin(std::future::ready(Ok(())))
    }
}

/// A runtime that can start workers, holding every capability so the tests
/// below are about the worker seam rather than about a denial.
fn runtime_with_workers(host: Arc<TestWorkerHost>) -> Runtime {
    let mut rt = Runtime::new(
        Box::new(V8Engine::new(es_runtime_common::Limits::default()).expect("engine")),
        HostProviders::new(
            Arc::new(FixedClock {
                monotonic: 0,
                wall: 0,
            }),
            Arc::new(TestConsole::default()),
            Arc::new(MockNet::stub()),
            Arc::new(TestEntropy::new()),
        )
        .with_workers(host),
    )
    .expect("runtime");
    rt.set_capabilities(CapabilitySet::all());
    rt
}

/// Loads `main` as the entry module with a worker script available at
/// `./w.mjs`, then drives the loop far enough for the spawn and the first
/// receives to settle.
///
/// The loader has to be installed the way a real program installs one —
/// through a module load — because reading a worker's entry goes through
/// it, and a runtime with no loader refuses to start a worker at all.
fn drive_worker_main(rt: &mut Runtime, main: &str) {
    block_on(rt.load_module_source(
        "file:///app/main.mjs",
        main,
        MapLoader::new(&[("./w.mjs", "// the worker's own body never runs here")]),
    ))
    .expect("load main");
    for _ in 0..16 {
        rt.tick(0);
    }
}

/// What the runtime actually asks a host to start — the narrowing every
/// other worker test can only observe indirectly, asserted at the seam.
#[test]
fn a_spawn_hands_the_host_a_narrowed_spec() {
    let _g = v8_guard();
    let host = Arc::new(TestWorkerHost::default());
    let mut rt = runtime_with_workers(host.clone());
    drive_worker_main(
        &mut rt,
        "globalThis.__w = new Worker('./w.mjs', \
           { name: 'jobs', permissions: ['net'], memory: 64 });",
    );

    let spec = host.last_spec();
    assert_eq!(spec.name, "jobs");
    assert_eq!(spec.specifier, "file:///app/w.mjs");
    // Exactly what was asked for, and the ungated ones that ride along.
    assert!(spec.capabilities.contains(Capability::Net));
    assert!(!spec.capabilities.contains(Capability::FileWrite));
    assert!(!spec.capabilities.contains(Capability::Worker));
    // Loading the child's graph is the *parent's* authority, and only that.
    assert!(spec.load_capabilities.contains(Capability::FileSystem));
    assert!(!spec.load_capabilities.contains(Capability::Net));
    // `{ memory: 64 }` in megabytes, lowered from this agent's own ceiling.
    assert_eq!(spec.limits.heap_limit_bytes, Some(64 * 1024 * 1024));
    // A worker owns its thread, so `Atomics.wait` is legal there.
    assert!(spec.limits.can_block);
}

/// `permissions: "inherit"` is a ceiling, not an escape: the host is asked
/// for what this agent holds and no more.
#[test]
fn inherited_permissions_are_bounded_by_the_parents_own() {
    let _g = v8_guard();
    let host = Arc::new(TestWorkerHost::default());
    let mut rt = runtime_with_workers(host.clone());
    let mut narrowed = CapabilitySet::all();
    narrowed.revoke(Capability::Net);
    rt.set_capabilities(narrowed);
    drive_worker_main(
        &mut rt,
        "globalThis.__w = new Worker('./w.mjs', { permissions: 'inherit' });",
    );

    let spec = host.last_spec();
    assert!(spec.capabilities.contains(Capability::FileRead));
    assert!(
        !spec.capabilities.contains(Capability::Net),
        "a worker cannot inherit what its parent does not hold"
    );
}

/// Messages queued before the spawn resolves reach the host in the order
/// they were posted — the regression this seam exists to pin down, since
/// end-to-end it can only be observed by what the worker prints.
#[test]
fn queued_messages_reach_the_host_in_order() {
    let _g = v8_guard();
    let host = Arc::new(TestWorkerHost::default());
    let mut rt = runtime_with_workers(host.clone());
    drive_worker_main(
        &mut rt,
        "globalThis.__w = new Worker('./w.mjs'); \
         for (let i = 0; i < 3; i++) __w.postMessage(new Uint8Array(i + 1));",
    );

    let calls = host.calls();
    assert_eq!(calls[0], "spawn file:///app/w.mjs", "{calls:?}");
    // Byte lengths stand in for identity: growing means the order held.
    let posts: Vec<&String> = calls.iter().filter(|c| c.starts_with("post")).collect();
    assert_eq!(posts.len(), 3, "{calls:?}");
    assert!(posts[0] < posts[1] && posts[1] < posts[2], "{posts:?}");
}

/// A failure the host reports reaches the parent's `onerror` with its class
/// rebuilt — the whole point of describing a failure rather than formatting
/// it, checked without a second isolate to throw in.
#[test]
fn a_hosts_error_reaches_onerror_with_its_class() {
    let _g = v8_guard();
    let host = Arc::new(TestWorkerHost::default());
    host.queue(WorkerIncoming::Error {
        error: UncaughtError::new(
            "RangeError",
            "out of range",
            "RangeError: out of range\n    at inner (file:///w.mjs:2:9)",
        ),
    });
    let mut rt = runtime_with_workers(host.clone());
    drive_worker_main(
        &mut rt,
        "globalThis.__seen = null; \
         globalThis.__w = new Worker('./w.mjs'); \
         __w.onerror = (e) => { \
           __seen = [e.error.name, e.message, e.filename, e.lineno, e.colno, \
                     e.error instanceof RangeError].join('|'); \
           e.preventDefault(); \
         };",
    );

    assert_eq!(
        rt.eval("globalThis.__seen").unwrap(),
        Value::String("RangeError|out of range|file:///w.mjs|2|9|true".into())
    );
}

/// An isolate that fails — its CPU or memory limit, a module that throws —
/// still reads on to the host's `Closed`. That read is what retires the
/// agent on the host; an `Isolate` that stopped at the failure left its
/// entry registered for good, and ten thousand such failures (the pooled
/// host's ceiling) refused every later `new Isolate()`. Found by the
/// isolates soak scenario.
#[test]
fn a_failed_isolate_reads_on_to_its_close() {
    let _g = v8_guard();
    let host = Arc::new(TestWorkerHost::default());
    host.queue(WorkerIncoming::Error {
        error: UncaughtError::new(
            "ERR_ISOLATE_CPU_LIMIT",
            "isolate terminated: a turn used more than its 50 ms of CPU",
            "",
        ),
    });
    host.queue(WorkerIncoming::Closed);
    let mut rt = Runtime::new(
        Box::new(V8Engine::new(es_runtime_common::Limits::default()).expect("engine")),
        HostProviders::new(
            Arc::new(FixedClock {
                monotonic: 0,
                wall: 0,
            }),
            Arc::new(TestConsole::default()),
            Arc::new(MockNet::stub()),
            Arc::new(TestEntropy::new()),
        )
        .with_isolates(host.clone()),
    )
    .expect("runtime");
    rt.set_capabilities(CapabilitySet::all());
    drive_worker_main(
        &mut rt,
        "import { Isolate } from 'runtime:workers'; \
         globalThis.__code = null; \
         const iso = new Isolate({ source: 'export const f = () => 1;' }); \
         iso.onerror = (e) => e.preventDefault(); \
         iso.exports.catch((e) => { __code = e.code; });",
    );
    assert_eq!(
        rt.eval("globalThis.__code").unwrap(),
        Value::String("ERR_ISOLATE_CPU_LIMIT".into())
    );
    assert_eq!(
        host.unread(),
        0,
        "the Closed after the failure was never read"
    );
}

/// A worker's id leaves this agent's table when its stream ends (D183), so
/// a program that churns workers does not grow by one id each. What is
/// called on it afterwards is a no-op, as HTML has it: no throw, and
/// nothing reaches the host.
#[test]
fn an_ended_workers_id_is_released_and_later_calls_are_no_ops() {
    let _g = v8_guard();
    let host = Arc::new(TestWorkerHost::default());
    host.queue(WorkerIncoming::Closed);
    let mut rt = runtime_with_workers(host.clone());
    drive_worker_main(
        &mut rt,
        "import { inventory } from 'runtime:diagnostics'; \
         const w = new Worker('./w.mjs'); \
         globalThis.__kinds = () => inventory().handles.map((h) => h.kind).join(','); \
         globalThis.__after = () => { w.postMessage(1); const q = w.queued; w.terminate(); return q; };",
    );
    assert_eq!(host.unread(), 0, "the end was read");
    assert_eq!(rt.eval("__kinds()").unwrap(), Value::String(String::new()));
    assert_eq!(rt.eval("__after()").unwrap(), Value::Number(0.0));
    for _ in 0..8 {
        rt.tick(0);
    }
    assert_eq!(host.calls(), vec!["spawn file:///app/w.mjs".to_string()]);
}

/// The same for an isolate, which ends by failing: the id goes with the
/// close that follows the failure.
#[test]
fn a_failed_isolates_id_is_released() {
    let _g = v8_guard();
    let host = Arc::new(TestWorkerHost::default());
    host.queue(WorkerIncoming::Error {
        error: UncaughtError::new("ERR_ISOLATE_MEMORY_LIMIT", "isolate terminated", ""),
    });
    host.queue(WorkerIncoming::Closed);
    let mut rt = Runtime::new(
        Box::new(V8Engine::new(es_runtime_common::Limits::default()).expect("engine")),
        HostProviders::new(
            Arc::new(FixedClock {
                monotonic: 0,
                wall: 0,
            }),
            Arc::new(TestConsole::default()),
            Arc::new(MockNet::stub()),
            Arc::new(TestEntropy::new()),
        )
        .with_isolates(host.clone()),
    )
    .expect("runtime");
    rt.set_capabilities(CapabilitySet::all());
    drive_worker_main(
        &mut rt,
        "import { Isolate } from 'runtime:workers'; \
         import { inventory } from 'runtime:diagnostics'; \
         const iso = new Isolate({ source: 'export const f = () => 1;' }); \
         iso.onerror = (e) => e.preventDefault(); \
         iso.exports.catch(() => {}); \
         globalThis.__kinds = () => inventory().handles.map((h) => h.kind).join(','); \
         globalThis.__after = () => { iso.postMessage(1); iso.terminate(); return 'ok'; };",
    );
    assert_eq!(host.unread(), 0);
    assert_eq!(rt.eval("__kinds()").unwrap(), Value::String(String::new()));
    assert_eq!(rt.eval("__after()").unwrap(), Value::String("ok".into()));
    for _ in 0..8 {
        rt.tick(0);
    }
    assert!(
        !host
            .calls()
            .iter()
            .any(|c| c.starts_with("post") || c.starts_with("terminate")),
        "{:?}",
        host.calls()
    );
}

/// `terminate()` reaches the host, and stops the runtime asking for more.
#[test]
fn terminate_reaches_the_host_and_ends_the_pump() {
    let _g = v8_guard();
    // Alive and idle: a worker that already ended has nothing for
    // `terminate()` to reach (D183).
    let host = Arc::new(TestWorkerHost::held_open());
    let mut rt = runtime_with_workers(host.clone());
    drive_worker_main(&mut rt, "globalThis.__w = new Worker('./w.mjs');");
    rt.eval("__w.terminate(); __w.postMessage('dropped');")
        .unwrap();
    for _ in 0..8 {
        rt.tick(0);
    }

    let calls = host.calls();
    assert!(calls.contains(&"terminate 1".to_string()), "{calls:?}");
    // Nothing is posted after a terminate, even though the host would
    // happily have taken it.
    let after = calls
        .iter()
        .position(|c| c == "terminate 1")
        .expect("terminate");
    assert!(
        !calls[after..].iter().any(|c| c.starts_with("post")),
        "{calls:?}"
    );
}

#[test]
fn message_port_delivers_asynchronously_and_in_order() {
    let _g = v8_guard();
    let mut rt = runtime();
    // `sync` captures that nothing is delivered synchronously: postMessage
    // queues a task, so the count is still 0 at the point of the send.
    let out = eval_async(
        &mut rt,
        "const ch = new MessageChannel(); \
         const seen = []; \
         ch.port2.onmessage = (e) => seen.push(e.data); \
         ch.port1.postMessage('a'); \
         ch.port1.postMessage('b'); \
         const sync = seen.length; \
         await new Promise((r) => setTimeout(r, 0)); \
         await new Promise((r) => setTimeout(r, 0)); \
         return sync + '|' + seen.join(',');",
    );
    assert_eq!(out, Value::String("0|a,b".into()));
}

#[test]
fn message_port_structured_clones_the_payload() {
    let _g = v8_guard();
    let mut rt = runtime();
    // The receiver must see the value as it was at postMessage time, and
    // must not share identity with the sender's object.
    let out = eval_async(
        &mut rt,
        "const ch = new MessageChannel(); \
         let got = null; \
         ch.port2.onmessage = (e) => { got = e.data; }; \
         const payload = { n: 1, nested: { deep: true } }; \
         ch.port1.postMessage(payload); \
         payload.n = 999; \
         await new Promise((r) => setTimeout(r, 0)); \
         await new Promise((r) => setTimeout(r, 0)); \
         return got.n + '|' + got.nested.deep + '|' + (got === payload);",
    );
    assert_eq!(out, Value::String("1|true|false".into()));
}

#[test]
fn message_port_buffers_until_started() {
    let _g = v8_guard();
    let mut rt = runtime();
    // addEventListener does not start a port; only start() (or assigning
    // onmessage) does. Messages sent before then are buffered, not dropped.
    //
    // Delivery after `start()` is a *task*, not a synchronous flush inside
    // the call — so the turn below is part of the assertion, not padding.
    // (This read `seen` immediately after `start()` while ports were
    // pure-JS objects and the queue was drained inline, which was the
    // shortcut rather than the spec.)
    let out = eval_async(
        &mut rt,
        "const ch = new MessageChannel(); \
         const seen = []; \
         ch.port2.addEventListener('message', (e) => seen.push(e.data)); \
         ch.port1.postMessage('early'); \
         await new Promise((r) => setTimeout(r, 0)); \
         await new Promise((r) => setTimeout(r, 0)); \
         const beforeStart = seen.length; \
         ch.port2.start(); \
         await new Promise((r) => setTimeout(r, 0)); \
         return beforeStart + '|' + seen.join(',');",
    );
    assert_eq!(out, Value::String("0|early".into()));
}

#[test]
fn message_port_close_stops_delivery() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const ch = new MessageChannel(); \
         const seen = []; \
         ch.port2.onmessage = (e) => seen.push(e.data); \
         ch.port1.postMessage('before'); \
         ch.port2.close(); \
         ch.port1.postMessage('after'); \
         await new Promise((r) => setTimeout(r, 0)); \
         await new Promise((r) => setTimeout(r, 0)); \
         return String(seen.length);",
    );
    assert_eq!(out, Value::String("0".into()));
}

#[test]
fn broadcast_channel_reaches_peers_but_not_the_sender() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const a = new BroadcastChannel('room'); \
         const b = new BroadcastChannel('room'); \
         const c = new BroadcastChannel('other'); \
         const seen = []; \
         a.onmessage = () => seen.push('a'); \
         b.onmessage = () => seen.push('b'); \
         c.onmessage = () => seen.push('c'); \
         a.postMessage('hello'); \
         await new Promise((r) => setTimeout(r, 0)); \
         await new Promise((r) => setTimeout(r, 0)); \
         a.close(); b.close(); c.close(); \
         return seen.join(',');",
    );
    // Only the same-named peer, and never the sender itself.
    assert_eq!(out, Value::String("b".into()));
}

#[test]
fn broadcast_channel_close_unsubscribes() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const a = new BroadcastChannel('r2'); \
         const b = new BroadcastChannel('r2'); \
         let n = 0; \
         b.onmessage = () => n++; \
         b.close(); \
         a.postMessage('x'); \
         await new Promise((r) => setTimeout(r, 0)); \
         await new Promise((r) => setTimeout(r, 0)); \
         a.close(); \
         return String(n);",
    );
    assert_eq!(out, Value::String("0".into()));
}
