//! The runtime crate's own tests, by subsystem; the fakes, providers and
//! helpers they share are here.

use super::*;
use es_runtime_common::Limits;
use std::sync::Mutex;

/// Serializes V8-touching tests in this binary (see the engine crate's note:
/// V8's snapshot/isolate global state is not safe under the parallel harness).
pub(crate) fn v8_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Ticks `rt` until `ready` holds, bounded by **wall-clock time** rather
/// than a tick count, and panics naming `what` if the budget runs out.
///
/// Some work the loop waits on happens off-thread: a WebAssembly compile
/// runs on V8's background threads and only lands once the platform's
/// foreground queue is pumped. How many ticks that takes is a property of
/// how busy the machine is, not of the runtime — so a fixed spin count
/// passes on an idle box and fails on a loaded CI runner (or a full
/// `cargo test --workspace`, where the other test binaries run alongside).
/// Worse, a count-bounded loop *falls through silently*: the test then
/// asserts against a result that simply had not arrived yet.
///
/// The first `HOT_SPINS` iterations spin, which is what everything settling
/// inside a single tick needs and keeps `eval_async` cheap enough for the
/// 20k-iteration soak. Past that the loop sleeps a millisecond per
/// turn, which also hands the core to the background threads it is waiting
/// on instead of starving them.
fn pump_until(rt: &mut Runtime, what: &str, ready: impl FnMut(&mut Runtime) -> bool) {
    /// Generous enough that only a genuine hang trips it.
    const BUDGET: std::time::Duration = std::time::Duration::from_secs(10);
    pump_until_within(rt, BUDGET, what, ready);
}

/// [`pump_until`] with an explicit budget, so the timeout path is testable
/// without a ten-second test.
fn pump_until_within(
    rt: &mut Runtime,
    budget: std::time::Duration,
    what: &str,
    mut ready: impl FnMut(&mut Runtime) -> bool,
) {
    /// Iterations run without sleeping before falling back to 1ms naps.
    const HOT_SPINS: u32 = 256;

    let deadline = std::time::Instant::now() + budget;
    let mut spins = 0u32;
    loop {
        rt.tick(0);
        if ready(rt) {
            return;
        }
        spins += 1;
        if spins > HOT_SPINS {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out after {budget:?} waiting for {what}"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

/// A capturing console sink for assertions.
#[derive(Default)]
struct TestConsole {
    lines: Mutex<Vec<(ConsoleLevel, String)>>,
}
impl Console for TestConsole {
    fn write(&self, level: ConsoleLevel, message: &str) {
        self.lines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((level, message.to_string()));
    }
}

/// A clock returning fixed monotonic/wall readings.
struct FixedClock {
    monotonic: u64,
    wall: u64,
}
impl Clock for FixedClock {
    fn monotonic_ms(&self) -> u64 {
        self.monotonic
    }
    fn wall_ms(&self) -> u64 {
        self.wall
    }
}

/// A canned NetTransport for fetch tests (no real network).
struct MockNet {
    status: u16,
    headers: Vec<(String, String)>,
    chunks: Vec<Vec<u8>>,
    fail: bool,
    /// Trailers every response carries, or `None` for a transport that has
    /// no way to deliver any.
    trailers: Option<Vec<(String, String)>>,
}
impl MockNet {
    fn ok(body: &str) -> Self {
        MockNet {
            status: 200,
            headers: vec![("content-type".into(), "text/plain".into())],
            chunks: vec![body.as_bytes().to_vec()],
            fail: false,
            trailers: None,
        }
    }
    fn with_trailers(mut self, trailers: Vec<(String, String)>) -> Self {
        self.trailers = Some(trailers);
        self
    }
    /// A transport that errors — for runtimes whose tests never fetch.
    fn stub() -> Self {
        MockNet {
            status: 0,
            headers: Vec::new(),
            chunks: Vec::new(),
            fail: true,
            trailers: None,
        }
    }
}
impl es_runtime_providers::NetTransport for MockNet {
    fn fetch(
        &self,
        request: es_runtime_providers::HttpRequest,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<
            es_runtime_providers::HttpResponse,
            es_runtime_providers::ProviderError,
        >,
    > {
        if self.fail {
            return Box::pin(async {
                Err(es_runtime_providers::ProviderError::Other(
                    "no network".into(),
                ))
            });
        }
        let (status, headers, chunks, url, trailers) = (
            self.status,
            self.headers.clone(),
            self.chunks.clone(),
            request.url,
            self.trailers.clone(),
        );
        Box::pin(async move {
            let body: es_runtime_providers::ByteStream =
                Box::pin(futures_util::stream::iter(chunks.into_iter().map(Ok)));
            Ok(es_runtime_providers::HttpResponse {
                status,
                status_text: "OK".into(),
                url,
                redirected: false,
                headers,
                body,
                trailers: trailers.map(|t| {
                    Box::pin(async move { t })
                        as es_runtime_providers::BoxFuture<Vec<(String, String)>>
                }),
            })
        })
    }
}

/// A deterministic (non-crypto) entropy source for tests.
struct TestEntropy {
    state: std::sync::atomic::AtomicU64,
}
impl TestEntropy {
    fn new() -> Self {
        TestEntropy {
            state: std::sync::atomic::AtomicU64::new(0x1234_5678_9abc_def0),
        }
    }
}
impl Entropy for TestEntropy {
    fn fill(
        &self,
        dest: &mut [u8],
    ) -> std::result::Result<(), es_runtime_providers::ProviderError> {
        use std::sync::atomic::Ordering;
        let mut x = self.state.load(Ordering::SeqCst) | 1;
        for b in dest.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = (x & 0xff) as u8;
        }
        self.state.store(x, Ordering::SeqCst);
        Ok(())
    }
}

fn runtime() -> Runtime {
    runtime_full(
        Arc::new(TestConsole::default()),
        Arc::new(FixedClock {
            monotonic: 0,
            wall: 0,
        }),
        Arc::new(MockNet::stub()),
        Arc::new(TestEntropy::new()),
    )
}

fn runtime_with(console: Arc<dyn Console>, clock: Arc<dyn Clock>) -> Runtime {
    runtime_full(
        console,
        clock,
        Arc::new(MockNet::stub()),
        Arc::new(TestEntropy::new()),
    )
}

fn runtime_full(
    console: Arc<dyn Console>,
    clock: Arc<dyn Clock>,
    net: Arc<dyn NetTransport>,
    entropy: Arc<dyn Entropy>,
) -> Runtime {
    let engine = V8Engine::new(Limits::default()).expect("engine");
    Runtime::new(
        Box::new(engine),
        HostProviders::new(clock, console, net, entropy)
            .with_ports(Arc::new(TestPortHub::default())),
    )
    .expect("runtime")
}

/// The minimum [`Process`] view `runtime:process` needs to evaluate at all —
/// the module snapshots env/args/platform on import, so the signal tests
/// below cannot load it without one.
struct StubProcess;
impl es_runtime_providers::Process for StubProcess {
    fn env(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    fn args(&self) -> Vec<String> {
        Vec::new()
    }
    fn cwd(&self) -> std::result::Result<String, es_runtime_providers::ProviderError> {
        Ok("/".to_string())
    }
    fn platform(&self) -> String {
        "test".to_string()
    }
    fn arch(&self) -> String {
        "test".to_string()
    }
    fn exit(&self, _code: i32) {}
    fn requested_exit_code(&self) -> Option<i32> {
        None
    }
}

/// An in-memory [`FileSystem`], so a test can exercise `runtime:fs` without
/// a real disk — no temp directory to clean up, no file left in the repo,
/// and every future resolves immediately, which is what lets a synchronous
/// runner drive it. Path strings are keys: jailing and realpath resolution
/// belong to the *default* provider (D25), not to this trait.
#[derive(Default)]
struct MemoryFs {
    files: Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
}

impl MemoryFs {
    /// Boxes an immediately-ready result as the trait's future.
    fn ready<T: Send + 'static>(
        value: std::result::Result<T, es_runtime_providers::ProviderError>,
    ) -> es_runtime_providers::BoxFuture<std::result::Result<T, es_runtime_providers::ProviderError>>
    {
        Box::pin(std::future::ready(value))
    }

    /// The error a real filesystem raises for a path that is not there.
    fn not_found(path: &str) -> es_runtime_providers::ProviderError {
        es_runtime_providers::ProviderError::Coded {
            code: es_runtime_common::ErrorCode::NotFound,
            message: format!("no such file or directory: {path}"),
        }
    }
}

impl FileSystem for MemoryFs {
    fn read(
        &self,
        path: String,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<Vec<u8>, es_runtime_providers::ProviderError>,
    > {
        let found = self.files.lock().unwrap().get(&path).cloned();
        Self::ready(found.ok_or_else(|| Self::not_found(&path)))
    }

    fn write(
        &self,
        path: String,
        data: Vec<u8>,
        append: bool,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<u64, es_runtime_providers::ProviderError>,
    > {
        let written = data.len() as u64;
        let mut files = self.files.lock().unwrap();
        let entry = files.entry(path).or_default();
        if !append {
            entry.clear();
        }
        entry.extend_from_slice(&data);
        Self::ready(Ok(written))
    }

    fn stat(
        &self,
        path: String,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<es_runtime_providers::FileStat, es_runtime_providers::ProviderError>,
    > {
        let size = self.files.lock().unwrap().get(&path).map(Vec::len);
        Self::ready(size.map_or_else(
            || Err(Self::not_found(&path)),
            |size| {
                Ok(es_runtime_providers::FileStat {
                    size: size as u64,
                    is_file: true,
                    is_dir: false,
                    is_symlink: false,
                    mtime_ms: None,
                })
            },
        ))
    }

    fn exists(
        &self,
        path: String,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<bool, es_runtime_providers::ProviderError>,
    > {
        Self::ready(Ok(self.files.lock().unwrap().contains_key(&path)))
    }

    fn read_dir(
        &self,
        _path: String,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<
            Vec<es_runtime_providers::DirEntry>,
            es_runtime_providers::ProviderError,
        >,
    > {
        Self::ready(Ok(Vec::new()))
    }

    fn mkdir(
        &self,
        _path: String,
        _recursive: bool,
    ) -> es_runtime_providers::BoxFuture<std::result::Result<(), es_runtime_providers::ProviderError>>
    {
        // A flat map has no directories to create; a write makes its own path.
        Self::ready(Ok(()))
    }

    fn remove(
        &self,
        path: String,
        _recursive: bool,
    ) -> es_runtime_providers::BoxFuture<std::result::Result<(), es_runtime_providers::ProviderError>>
    {
        let removed = self.files.lock().unwrap().remove(&path);
        Self::ready(if removed.is_some() {
            Ok(())
        } else {
            Err(Self::not_found(&path))
        })
    }

    fn rename(
        &self,
        from: String,
        to: String,
    ) -> es_runtime_providers::BoxFuture<std::result::Result<(), es_runtime_providers::ProviderError>>
    {
        let mut files = self.files.lock().unwrap();
        let moved = files.remove(&from);
        Self::ready(match moved {
            Some(bytes) => {
                files.insert(to, bytes);
                Ok(())
            }
            None => Err(Self::not_found(&from)),
        })
    }

    fn glob_match(
        &self,
        _pattern: &str,
        _path: &str,
    ) -> std::result::Result<bool, es_runtime_providers::ProviderError> {
        Err(es_runtime_providers::ProviderError::Other(
            "the in-memory test filesystem does not implement globbing".into(),
        ))
    }

    fn glob_scan(
        &self,
        _base: String,
        _pattern: String,
        _opts: es_runtime_providers::GlobScanOptions,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<Vec<String>, es_runtime_providers::ProviderError>,
    > {
        Self::ready(Err(es_runtime_providers::ProviderError::Other(
            "the in-memory test filesystem does not implement globbing".into(),
        )))
    }

    fn open_tree(
        &self,
        _path: String,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<
            Arc<dyn es_runtime_providers::Tree>,
            es_runtime_providers::ProviderError,
        >,
    > {
        Self::ready(Err(es_runtime_providers::ProviderError::Other(
            "the in-memory test filesystem does not implement trees".into(),
        )))
    }

    fn copy(
        &self,
        from: String,
        to: String,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<u64, es_runtime_providers::ProviderError>,
    > {
        let mut files = self.files.lock().unwrap();
        // Copy-and-insert would make this a harmless no-op, but the contract
        // the system filesystem enforces is a refusal (there it is a wipe),
        // and the conformance suite runs against this double.
        if from == to && files.contains_key(&from) {
            return Self::ready(Err(es_runtime_providers::ProviderError::Coded {
                code: es_runtime_common::ErrorCode::SameFile,
                message: format!(
                    "Source and destination paths refer to the same file: copy '{from}' -> '{to}'"
                ),
            }));
        }
        match files.get(&from).cloned() {
            Some(bytes) => {
                let n = bytes.len() as u64;
                files.insert(to, bytes);
                Self::ready(Ok(n))
            }
            None => Self::ready(Err(Self::not_found(&from))),
        }
    }

    fn real_path(
        &self,
        path: String,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<String, es_runtime_providers::ProviderError>,
    > {
        // Path strings are keys here — there is nothing to canonicalize.
        if self.files.lock().unwrap().contains_key(&path) {
            Self::ready(Ok(path))
        } else {
            Self::ready(Err(Self::not_found(&path)))
        }
    }

    fn read_link(
        &self,
        path: String,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<String, es_runtime_providers::ProviderError>,
    > {
        // No links in an in-memory map; the real jail behaviour is tested
        // against the default provider.
        Self::ready(Err(Self::not_found(&path)))
    }

    fn symlink(
        &self,
        _target: String,
        path: String,
        _kind: Option<es_runtime_providers::SymlinkKind>,
    ) -> es_runtime_providers::BoxFuture<std::result::Result<(), es_runtime_providers::ProviderError>>
    {
        // No links in an in-memory map, for the same reason `read_link` has
        // none: what a link does is a filesystem's behaviour, and it is
        // tested against the default provider.
        Self::ready(Err(Self::not_found(&path)))
    }

    fn truncate(
        &self,
        path: String,
        len: u64,
    ) -> es_runtime_providers::BoxFuture<std::result::Result<(), es_runtime_providers::ProviderError>>
    {
        let mut files = self.files.lock().unwrap();
        match files.get_mut(&path) {
            Some(bytes) => {
                bytes.resize(len as usize, 0);
                Self::ready(Ok(()))
            }
            None => Self::ready(Err(Self::not_found(&path))),
        }
    }

    fn chmod(
        &self,
        _path: String,
        _mode: u32,
    ) -> es_runtime_providers::BoxFuture<std::result::Result<(), es_runtime_providers::ProviderError>>
    {
        // No permission bits to set; accepted so a caller's flow still runs.
        Self::ready(Ok(()))
    }

    fn make_temp_dir(
        &self,
        dir: String,
        prefix: String,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<String, es_runtime_providers::ProviderError>,
    > {
        Self::ready(Ok(format!("{dir}/{prefix}memtmp")))
    }

    fn make_temp_file(
        &self,
        dir: String,
        prefix: String,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<String, es_runtime_providers::ProviderError>,
    > {
        let path = format!("{dir}/{prefix}memtmp");
        self.files.lock().unwrap().insert(path.clone(), Vec::new());
        Self::ready(Ok(path))
    }
}

/// An in-process [`PortHub`] for the tests, so `MessagePort` exercises the
/// host-backed path the CLI uses rather than the agent-local fallback.
/// Deliberately simple: one inbox per port, delivery to the peer.
#[derive(Default)]
struct TestPortHub {
    ports: Arc<std::sync::Mutex<HashMap<u64, TestPort>>>,
    next: std::sync::atomic::AtomicU64,
}

#[derive(Default)]
struct TestPort {
    peer: Option<u64>,
    queue: std::collections::VecDeque<Vec<u8>>,
}

impl es_runtime_providers::PortHub for TestPortHub {
    fn create(&self) -> std::result::Result<(u64, u64), es_runtime_providers::ProviderError> {
        let mut ports = self.ports.lock().expect("port lock");
        let a = self.next.fetch_add(2, std::sync::atomic::Ordering::SeqCst) + 1;
        let b = a + 1;
        ports.insert(
            a,
            TestPort {
                peer: Some(b),
                ..TestPort::default()
            },
        );
        ports.insert(
            b,
            TestPort {
                peer: Some(a),
                ..TestPort::default()
            },
        );
        Ok((a, b))
    }

    fn post(
        &self,
        id: u64,
        message: Vec<u8>,
    ) -> std::result::Result<(), es_runtime_providers::ProviderError> {
        let mut ports = self.ports.lock().expect("port lock");
        if let Some(peer) = ports.get(&id).and_then(|port| port.peer)
            && let Some(port) = ports.get_mut(&peer)
        {
            port.queue.push_back(message);
        }
        Ok(())
    }

    fn recv(
        &self,
        id: u64,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<Option<Vec<u8>>, es_runtime_providers::ProviderError>,
    > {
        // Stays `Pending` while the queue is empty. No waker is registered
        // because none is needed: the runtime re-polls every pending op on
        // each tick, which is exactly what these tests drive by hand.
        let ports = self.ports.clone();
        Box::pin(std::future::poll_fn(move |_cx| {
            let mut ports = ports.lock().expect("port lock");
            match ports.get_mut(&id) {
                None => std::task::Poll::Ready(Ok(None)),
                Some(port) => match port.queue.pop_front() {
                    Some(message) => std::task::Poll::Ready(Ok(Some(message))),
                    None => std::task::Poll::Pending,
                },
            }
        }))
    }

    fn detach_reader(&self, _id: u64) {}

    fn close(&self, id: u64) {
        let mut ports = self.ports.lock().expect("port lock");
        if let Some(port) = ports.remove(&id)
            && let Some(peer) = port.peer
            && let Some(peer) = ports.get_mut(&peer)
        {
            peer.peer = None;
        }
    }
}

use es_runtime_providers::{WorkerIncoming, WorkerSpec};

fn test_providers() -> HostProviders {
    HostProviders::new(
        Arc::new(FixedClock {
            monotonic: 0,
            wall: 0,
        }),
        Arc::new(TestConsole::default()),
        Arc::new(MockNet::stub()),
        Arc::new(TestEntropy::new()),
    )
    .with_ports(Arc::new(TestPortHub::default()))
}

/// Asserts a JS expression evaluates to `true`.
fn assert_true(rt: &mut Runtime, expr: &str) {
    assert_eq!(rt.eval(expr).unwrap(), Value::Bool(true), "expr: {expr}");
}

/// Runs an async JS `body` (which should `return` a value) to completion by
/// ticking the microtask loop, then returns the resolved value. A rejection
/// is returned as a `Value::String` prefixed with `ERR:`.
///
/// A body that never settles is a **panic**, not an `undefined` result: this
/// used to give up after a fixed tick count and read `__result` regardless,
/// so work that had not finished was indistinguishable from work that
/// finished with no value — and a caller's assertion then blamed the
/// behaviour under test rather than the wait.
fn eval_async(rt: &mut Runtime, body: &str) -> Value {
    rt.eval(&format!(
        "globalThis.__done = false; globalThis.__result = undefined; \
         (async () => {{ {body} }})().then( \
           (r) => {{ globalThis.__result = r; globalThis.__done = true; }}, \
           (e) => {{ globalThis.__result = 'ERR:' + ((e && e.message) || e); \
                     globalThis.__done = true; }});"
    ))
    .unwrap();
    pump_until(rt, "the async body to settle", |rt| {
        rt.eval("globalThis.__done").unwrap() == Value::Bool(true)
    });
    rt.eval("globalThis.__result").unwrap()
}

/// Resident set size in KiB (Linux), for the soak/leak test. Elsewhere 0, so
/// the growth bound is trivially satisfied (the registry-drain assertion is
/// the portable leak guard).
#[cfg(target_os = "linux")]
fn resident_kib() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").unwrap_or_default();
    let pages: u64 = statm
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    pages * 4 // 4 KiB pages
}
#[cfg(not(target_os = "linux"))]
fn resident_kib() -> u64 {
    0
}

// ----- ES module loading -------------------------------------------------

/// Drives a synchronous future (the mock loader never truly pends) to its
/// result, so the async `load_module_source` can be used from sync tests.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll};
    let waker = std::task::Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut future = std::pin::pin!(future);
    for _ in 0..10_000 {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
    }
    panic!("future did not complete — the mock loader should be synchronous");
}

/// An in-memory [`ModuleLoader`]: resolves `file://` URLs the way
/// `FsModuleLoader` does but serves sources from a map, so graph-walking
/// (resolution, dedup, cycles) is exercised without touching disk.
struct MapLoader {
    base: url::Url,
    files: std::collections::HashMap<String, String>,
}
impl MapLoader {
    // Returns a trait object (not Self) deliberately — tests pass it straight
    // to the Arc-taking module APIs.
    #[allow(clippy::new_ret_no_self)]
    fn new(files: &[(&str, &str)]) -> Arc<dyn ModuleLoader> {
        let base = url::Url::parse("file:///app/").unwrap();
        let files = files
            .iter()
            .map(|(spec, src)| (base.join(spec).unwrap().to_string(), src.to_string()))
            .collect();
        Arc::new(MapLoader { base, files })
    }
}
impl ModuleLoader for MapLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<String, es_runtime_providers::ProviderError>,
    > {
        let base = self.base.clone();
        let specifier = specifier.to_string();
        let referrer = referrer.to_string();
        Box::pin(async move {
            let base = if referrer.is_empty() {
                base
            } else {
                url::Url::parse(&referrer)
                    .map_err(|e| es_runtime_providers::ProviderError::Other(e.to_string()))?
            };
            base.join(&specifier)
                .map(|u| u.to_string())
                .map_err(|e| es_runtime_providers::ProviderError::Other(e.to_string()))
        })
    }
    fn load(
        &self,
        specifier: &str,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<ModuleSource, es_runtime_providers::ProviderError>,
    > {
        let result = self
            .files
            .get(specifier)
            .cloned()
            .map(ModuleSource::Text)
            .ok_or_else(|| {
                es_runtime_providers::ProviderError::Other(format!("not found: {specifier}"))
            });
        Box::pin(async move { result })
    }
}

const ENTRY: &str = "file:///app/main.mjs";

/// Loads + evaluates a module graph (granting FileSystem) and ticks to
/// quiescence, returning the evaluation outcome.
fn run_module(rt: &mut Runtime, source: &str, loader: Arc<dyn ModuleLoader>) -> ModuleEvalState {
    rt.set_capabilities(CapabilitySet::all());
    block_on(async {
        rt.load_module_source(ENTRY, source, loader)
            .await
            .expect("load module graph");
        for _ in 0..500 {
            rt.tick(0);
            rt.process_dynamic_imports()
                .await
                .expect("process dynamic imports");
            if !rt.has_pending_work() {
                break;
            }
        }
    });
    rt.module_eval_state()
}

// ---- runtime:system (child processes, DECISIONS D37) --------------------

/// What a spawn asked for, recorded so a test can assert on the *seam*
/// rather than on a real process: the environment a child would have been
/// given is the whole point of the D37 design, and no OS is needed to check
/// it.
#[derive(Clone, Debug, Default)]
struct Spawned {
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

/// A [`CommandProvider`] that starts nothing: it records the spec, hands
/// back canned output, and reports a canned status.
#[derive(Clone, Default)]
struct TestCommands {
    spawned: Arc<Mutex<Vec<Spawned>>>,
    stdout: Arc<Mutex<Vec<Vec<u8>>>>,
    stderr: Arc<Mutex<Vec<Vec<u8>>>>,
    written: Arc<Mutex<Vec<u8>>>,
    killed: Arc<Mutex<Vec<String>>>,
    closed: Arc<Mutex<Vec<u64>>>,
    status: Arc<Mutex<es_runtime_providers::ChildStatus>>,
}

impl TestCommands {
    fn with_stdout(chunks: &[&str]) -> Arc<TestCommands> {
        let commands = TestCommands::default();
        *commands.stdout.lock().unwrap() = chunks.iter().map(|c| c.as_bytes().to_vec()).collect();
        *commands.status.lock().unwrap() = es_runtime_providers::ChildStatus {
            success: true,
            code: Some(0),
            signal: None,
        };
        Arc::new(commands)
    }

    fn last_spawn(&self) -> Spawned {
        self.spawned.lock().unwrap().last().cloned().unwrap()
    }
}

impl es_runtime_providers::CommandProvider for TestCommands {
    fn spawn(
        &self,
        spec: es_runtime_providers::CommandSpec,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<(u64, u32), es_runtime_providers::ProviderError>,
    > {
        self.spawned.lock().unwrap().push(Spawned {
            program: spec.program,
            args: spec.args,
            env: spec.env,
        });
        Box::pin(std::future::ready(Ok((1, 4242))))
    }

    fn read(
        &self,
        _id: u64,
        stream: es_runtime_providers::ChildStream,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<Option<Vec<u8>>, es_runtime_providers::ProviderError>,
    > {
        let queue = match stream {
            es_runtime_providers::ChildStream::Stdout => &self.stdout,
            es_runtime_providers::ChildStream::Stderr => &self.stderr,
        };
        let mut queue = queue.lock().unwrap();
        let next = if queue.is_empty() {
            None
        } else {
            Some(queue.remove(0))
        };
        Box::pin(std::future::ready(Ok(next)))
    }

    fn write(
        &self,
        _id: u64,
        data: Vec<u8>,
    ) -> es_runtime_providers::BoxFuture<std::result::Result<(), es_runtime_providers::ProviderError>>
    {
        self.written.lock().unwrap().extend_from_slice(&data);
        Box::pin(std::future::ready(Ok(())))
    }

    fn close_stdin(
        &self,
        _id: u64,
    ) -> es_runtime_providers::BoxFuture<std::result::Result<(), es_runtime_providers::ProviderError>>
    {
        Box::pin(std::future::ready(Ok(())))
    }

    fn wait(
        &self,
        _id: u64,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<es_runtime_providers::ChildStatus, es_runtime_providers::ProviderError>,
    > {
        let status = self.status.lock().unwrap().clone();
        Box::pin(std::future::ready(Ok(status)))
    }

    fn kill(
        &self,
        _id: u64,
        signal: es_runtime_providers::Signal,
    ) -> es_runtime_providers::BoxFuture<std::result::Result<(), es_runtime_providers::ProviderError>>
    {
        self.killed.lock().unwrap().push(signal.name().to_string());
        Box::pin(std::future::ready(Ok(())))
    }

    fn close(
        &self,
        id: u64,
    ) -> es_runtime_providers::BoxFuture<std::result::Result<(), es_runtime_providers::ProviderError>>
    {
        self.closed.lock().unwrap().push(id);
        Box::pin(std::future::ready(Ok(())))
    }
}

/// A [`Process`] view with a fixed environment, so the Secret-masking and
/// inheritance paths can be exercised without touching the host's.
struct EnvProcess(Vec<(String, String)>);
impl es_runtime_providers::Process for EnvProcess {
    fn env(&self) -> Vec<(String, String)> {
        self.0.clone()
    }
    fn args(&self) -> Vec<String> {
        Vec::new()
    }
    fn cwd(&self) -> std::result::Result<String, es_runtime_providers::ProviderError> {
        Ok("/".to_string())
    }
    fn platform(&self) -> String {
        "test".to_string()
    }
    fn arch(&self) -> String {
        "test".to_string()
    }
    fn exit(&self, _code: i32) {}
    fn requested_exit_code(&self) -> Option<i32> {
        None
    }
}

fn command_runtime(commands: Arc<TestCommands>, env: &[(&str, &str)]) -> Runtime {
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
    .with_commands(commands)
    .with_process(Arc::new(EnvProcess(
        env.iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
    )));
    Runtime::new(Box::new(engine), providers).expect("runtime")
}

mod console;
mod crypto;
mod event_loop;
mod fetch;
mod fs;
mod http_server;
mod modules;
mod op_boundary;
mod signals;
mod system;
mod web_apis;
mod websocket;
mod workers;
