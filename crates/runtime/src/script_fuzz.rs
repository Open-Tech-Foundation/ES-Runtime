//! Fuzz harnesses that run JavaScript in a live runtime: the cargo-fuzz
//! `db_wire` and `request_parsers` targets, and a seeded run of each in this
//! crate's tests. Not public API.
//!
//! The other targets reach one Rust parser each, and `ops` (D253) the op
//! boundary. These reach what neither does: the code that reads untrusted bytes
//! in JavaScript, and Rust state machines that only run behind a socket.
//!
//! - `db_wire` answers the PostgreSQL, MySQL and MongoDB drivers with the
//!   input, through a [`NetProvider`] whose every read hands out the next
//!   chunk of it and whose writes go nowhere. The handshake, authentication,
//!   framing and every value decoder downstream of them meet bytes a hostile
//!   or broken server — or anyone on a path without TLS — could send.
//! - `request_parsers` feeds the input to what a server parses out of a
//!   request: `formData()` in both encodings, `Headers`, `URLSearchParams`,
//!   and cookies.
//!
//! A Rust panic is the finding, as in `ops`: libFuzzer's hook aborts on it. In
//! JavaScript, an error of a kind the API never throws on bad input — a
//! `ReferenceError`, a `RangeError` from a blown stack, a `TypeError` a driver
//! raised by reading a field off `undefined` — is reported as one too: it is
//! the runtime's own code failing on input it should have refused.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use es_runtime_common::{CapabilitySet, Limits};
use es_runtime_engine::{OpDecl, V8Engine, Value};
use es_runtime_providers::{
    BoxFuture, Clock, ConnectOptions, Console, ConsoleLevel, Entropy, HttpRequest, HttpResponse,
    ListenOptions, NetProvider, NetTransport, ProviderError, SocketInfo,
};

use crate::{HostProviders, Runtime};

/// Inputs one runtime serves before it is rebuilt: work a step leaves pending
/// (a driver waiting on a timer that never fires) is dropped with it.
pub const REBUILD_EVERY: u32 = 64;

/// Event-loop turns one input is given to settle.
const TURNS: u32 = 64;

/// Which harness a [`ScriptFuzzer`] runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Harness {
    /// Database drivers answered by the input.
    DbWire,
    /// What a server parses out of a request.
    RequestParsers,
}

/// What every harness shares: the input as bytes, a way to report, and a
/// promise the step settles when it is done.
/// Installed as internal code, the only code that holds the op table (D253):
/// it hands the guest-side harness its input and a way to report, and nothing
/// else.
const BRIDGE: &str = r#"
globalThis.__fuzzInput = () => __ops.__fuzz_input();
globalThis.__fuzzFound = (what) => __ops.__fuzz_found(String(what));
"#;

const PRELUDE: &str = r#"
globalThis.__fuzzDone = true;
const report = (where, e) => {
  const name = e?.constructor?.name ?? typeof e;
  __fuzzFound(`${where}: ${name}: ${e?.message ?? e}`);
};
// Errors code throws only when it has itself gone wrong.
const isBug = (e) =>
  e instanceof ReferenceError ||
  e instanceof EvalError ||
  (e instanceof RangeError && /call stack/i.test(e.message)) ||
  // V8's own wording for code that reached for something absent; an API's
  // TypeError about bad input says what was wrong, and may quote the input.
  (e instanceof TypeError &&
    /^(Cannot (read|set) properties of (undefined|null)|.* is not (a function|iterable|a constructor)$)/.test(e.message));
"#;

const DB_WIRE: &str = r#"
const db = await import("runtime:db");
const URLS = [
  "postgres://fuzz:secret@127.0.0.1/app?sslmode=disable",
  "postgres://fuzz:secret@127.0.0.1/app?sslmode=prefer",
  "mysql://fuzz:secret@127.0.0.1/app?ssl-mode=disabled&allowPublicKeyRetrieval=true",
  "mongodb://127.0.0.1/app?directConnection=true",
];
const DRIVERS = [db.postgres, db.postgres, db.mysql, db.mongodb];
globalThis.__fuzzStep = () => {
  const input = __fuzzInput();
  const which = (input[0] ?? 0) % URLS.length;
  globalThis.__fuzzDone = false;
  (async () => {
    let conn;
    try {
      conn = await db.connect(URLS[which], { driver: DRIVERS[which] });
      if (which === 3) {
        await conn.db().collection("c").find({ a: 1 }).toArray();
      } else {
        const rows = await conn.query(db.sql`select ${1} as x, 'y' as z`);
        for await (const row of rows) for (const key in row) void row[key];
      }
    } catch (e) {
      if (globalThis.__fuzzTrace) __fuzzFound(`trace ${which}: ${e?.constructor?.name}: ${e?.code} ${e?.message}`);
      if (isBug(e)) report(`db ${URLS[which]}`, e);
    } finally {
      try { await conn?.close?.(); } catch (e) { if (isBug(e)) report("db close", e); }
      globalThis.__fuzzDone = true;
    }
  })();
};
"#;

const REQUEST_PARSERS: &str = r#"
const http = await import("runtime:http");
const textOf = (bytes) => new TextDecoder().decode(bytes);
const attempt = async (where, run) => {
  try { await run(); } catch (e) { if (isBug(e)) report(where, e); }
};
globalThis.__fuzzStep = () => {
  const input = __fuzzInput();
  const mode = (input[0] ?? 0) % 6;
  const body = input.subarray(1);
  globalThis.__fuzzDone = false;
  (async () => {
    switch (mode) {
      case 0: {
        // The boundary is the input's first line, as an attacker would pick it.
        const text = textOf(body);
        const cut = text.indexOf("\n");
        const boundary = cut > 0 ? text.slice(0, cut).replace(/\r$/, "") : "b";
        const payload = cut > 0 ? body.subarray(new TextEncoder().encode(text.slice(0, cut + 1)).length) : body;
        await attempt("multipart", async () => {
          const form = await new Request("http://x/", {
            method: "POST",
            headers: { "content-type": `multipart/form-data; boundary=${boundary}` },
            body: payload,
          }).formData();
          for (const [, value] of form) if (value instanceof Blob) await value.arrayBuffer();
        });
        break;
      }
      case 1:
        await attempt("urlencoded", async () => {
          const form = await new Request("http://x/", {
            method: "POST",
            headers: { "content-type": "application/x-www-form-urlencoded" },
            body,
          }).formData();
          for (const entry of form) void entry;
        });
        break;
      case 2:
        await attempt("headers", () => {
          const lines = textOf(body).split("\n");
          const headers = new Headers();
          for (const line of lines) {
            const at = line.indexOf(":");
            if (at > 0) headers.append(line.slice(0, at), line.slice(at + 1));
          }
          for (const entry of headers) void entry;
          void headers.getSetCookie();
        });
        break;
      case 3:
        await attempt("cookies", () => {
          const text = textOf(body);
          const jar = new http.CookieMap(text);
          for (const entry of jar) void entry;
          try { void http.Cookie.parse(text).serialize(); } catch (e) { if (isBug(e)) throw e; }
        });
        break;
      case 4:
        await attempt("search params", () => {
          const params = new URLSearchParams(textOf(body));
          params.sort();
          void params.toString();
        });
        break;
      default:
        await attempt("content-type", async () => {
          const type = textOf(body.subarray(0, 256)).replace(/[\r\n\0]/g, "");
          const response = new Response(body.subarray(256), { headers: { "content-type": type } });
          void (await response.blob()).type;
        });
    }
    globalThis.__fuzzDone = true;
  })();
};
"#;

/// One runtime, rebuilt every [`REBUILD_EVERY`] inputs, running a harness.
pub struct ScriptFuzzer {
    harness: Harness,
    /// `None` only between dropping one runtime and building the next.
    runtime: Option<Runtime>,
    input: Rc<RefCell<Vec<u8>>>,
    found: Rc<RefCell<Vec<String>>>,
    replies: Arc<Mutex<VecDeque<Vec<u8>>>>,
    served: u32,
}

impl ScriptFuzzer {
    /// A fuzzer running `harness` in a fresh runtime.
    ///
    /// # Panics
    ///
    /// If the runtime cannot be built — a broken build, not a finding.
    pub fn new(harness: Harness) -> Self {
        let input = Rc::new(RefCell::new(Vec::new()));
        let found = Rc::new(RefCell::new(Vec::new()));
        let replies = Arc::new(Mutex::new(VecDeque::new()));
        let runtime = Some(build(harness, &input, &found, &replies));
        ScriptFuzzer {
            harness,
            runtime,
            input,
            found,
            replies,
            served: 0,
        }
    }

    /// Runs one input and returns what the harness reported.
    pub fn run(&mut self, input: &[u8]) -> Vec<String> {
        if self.served == REBUILD_EVERY {
            self.rebuild();
        }
        self.served += 1;
        input.clone_into(&mut self.input.borrow_mut());
        *self.replies.lock().unwrap() = split_replies(input.get(1..).unwrap_or_default());
        if self.runtime_mut().eval("__fuzzStep()").is_err() {
            self.rebuild();
            return std::mem::take(&mut self.found.borrow_mut());
        }
        for _ in 0..TURNS {
            // A driver's own modules load on first use, by dynamic import.
            if poll_ready(self.runtime_mut().process_dynamic_imports()).is_err() {
                self.rebuild();
                break;
            }
            self.runtime_mut().tick(0);
            if !matches!(
                self.runtime_mut().eval("__fuzzDone"),
                Ok(Value::Bool(false))
            ) {
                break;
            }
        }
        std::mem::take(&mut self.found.borrow_mut())
    }

    fn rebuild(&mut self) {
        self.runtime = None;
        self.runtime = Some(build(self.harness, &self.input, &self.found, &self.replies));
        self.served = 0;
    }

    fn runtime_mut(&mut self) -> &mut Runtime {
        self.runtime.as_mut().expect("a runtime between inputs")
    }
}

/// The server's replies: each a length byte and that many bytes, so the
/// fuzzer decides where one read ends and the next begins; a length of 0 takes
/// the rest of the input. `fuzz/record-db/record.py` writes recorded
/// sessions this way.
fn split_replies(mut bytes: &[u8]) -> VecDeque<Vec<u8>> {
    let mut out = VecDeque::new();
    while let Some((&len, rest)) = bytes.split_first() {
        let take = if len == 0 {
            rest.len()
        } else {
            usize::from(len).min(rest.len())
        };
        out.push_back(rest[..take].to_vec());
        bytes = &rest[take..];
    }
    out
}

fn build(
    harness: Harness,
    input: &Rc<RefCell<Vec<u8>>>,
    found: &Rc<RefCell<Vec<String>>>,
    replies: &Arc<Mutex<VecDeque<Vec<u8>>>>,
) -> Runtime {
    let limits = Limits::default().with_heap_limit_bytes(256 << 20);
    let engine = V8Engine::new(limits).expect("engine");
    let providers = HostProviders::new(
        Arc::new(StillClock),
        Arc::new(Silent),
        Arc::new(NoFetch),
        Arc::new(Counter(std::sync::atomic::AtomicU64::new(0x9e37_79b9))),
    )
    .with_net_provider(Arc::new(ScriptedNet {
        replies: replies.clone(),
    }));
    let mut runtime = Runtime::new(Box::new(engine), providers).expect("runtime");
    runtime.set_capabilities(CapabilitySet::all());

    let source = input.clone();
    runtime
        .register_op(OpDecl::sync("__fuzz_input", move |_| {
            Ok(Value::Bytes(source.borrow().clone()))
        }))
        .expect("register __fuzz_input");
    let sink = found.clone();
    runtime
        .register_op(OpDecl::sync("__fuzz_found", move |args| {
            let what = match args.first() {
                Some(Value::String(what)) => what.clone(),
                _ => String::from("?"),
            };
            sink.borrow_mut().push(what);
            Ok(Value::Undefined)
        }))
        .expect("register __fuzz_found");

    let body = match harness {
        Harness::DbWire => DB_WIRE,
        Harness::RequestParsers => REQUEST_PARSERS,
    };
    runtime
        .engine
        .eval_internal(BRIDGE)
        .expect("install the bridge");
    // The harness imports `runtime:` modules, which guest code does, so it is
    // installed as guest code by an async function the loop runs to completion.
    runtime
        .eval(&format!(
            "globalThis.__fuzzReady = false; (async () => {{ {PRELUDE} {body} \
             globalThis.__fuzzReady = true; }})().catch((e) => {{ \
             globalThis.__fuzzReady = `failed: ${{e?.stack ?? e}}`; }});"
        ))
        .expect("install the harness");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        match runtime.eval("globalThis.__fuzzReady") {
            Ok(Value::Bool(true)) => return runtime,
            Ok(Value::String(failure)) => panic!("the harness did not install: {failure}"),
            _ => {}
        }
        // A dynamic `import()` is settled by this step, not by a tick.
        poll_ready(runtime.process_dynamic_imports()).expect("load the harness's modules");
        if runtime.tick(0).async_ops_settled == 0 {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    panic!(
        "the harness did not install: {:?}",
        runtime.eval("String(globalThis.__fuzzReady)")
    );
}

/// Drives a future whose work is all in memory — the built-in modules — to its
/// result.
fn poll_ready<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll};
    let mut cx = Context::from_waker(std::task::Waker::noop());
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
    }
}

/// A network with one server on every address, answering with the input.
struct ScriptedNet {
    replies: Arc<Mutex<VecDeque<Vec<u8>>>>,
}

impl NetProvider for ScriptedNet {
    fn connect(
        &self,
        host: String,
        port: u16,
        _opts: ConnectOptions,
    ) -> BoxFuture<Result<(u64, SocketInfo), ProviderError>> {
        Box::pin(async move {
            Ok((
                1,
                SocketInfo {
                    remote_address: host,
                    remote_port: port,
                    ..SocketInfo::default()
                },
            ))
        })
    }

    fn read(&self, _id: u64) -> BoxFuture<Result<Option<Vec<u8>>, ProviderError>> {
        let next = self.replies.lock().unwrap().pop_front();
        Box::pin(async move { Ok(next) })
    }

    fn write(&self, _id: u64, _data: Vec<u8>) -> BoxFuture<Result<(), ProviderError>> {
        Box::pin(async { Ok(()) })
    }

    fn shutdown(&self, _id: u64) -> BoxFuture<Result<(), ProviderError>> {
        Box::pin(async { Ok(()) })
    }

    fn close(&self, _id: u64) -> BoxFuture<Result<(), ProviderError>> {
        Box::pin(async { Ok(()) })
    }

    fn listen(
        &self,
        _host: String,
        _port: u16,
        _opts: ListenOptions,
    ) -> BoxFuture<Result<(u64, SocketInfo), ProviderError>> {
        Box::pin(async { Err(ProviderError::Other("no listening while fuzzing".into())) })
    }

    fn accept(&self, _id: u64) -> BoxFuture<Result<Option<(u64, SocketInfo)>, ProviderError>> {
        Box::pin(async { Ok(None) })
    }

    fn close_listener(&self, _id: u64) -> BoxFuture<Result<(), ProviderError>> {
        Box::pin(async { Ok(()) })
    }
}

struct StillClock;

impl Clock for StillClock {
    fn monotonic_ms(&self) -> u64 {
        0
    }
    fn wall_ms(&self) -> u64 {
        0
    }
}

struct Silent;

impl Console for Silent {
    fn write(&self, _level: ConsoleLevel, _message: &str) {}
}

struct NoFetch;

impl NetTransport for NoFetch {
    fn fetch(&self, _request: HttpRequest) -> BoxFuture<Result<HttpResponse, ProviderError>> {
        Box::pin(async { Err(ProviderError::Other("no fetch while fuzzing".into())) })
    }
}

struct Counter(std::sync::atomic::AtomicU64);

impl Entropy for Counter {
    fn fill(&self, dest: &mut [u8]) -> Result<(), ProviderError> {
        use std::sync::atomic::Ordering;
        let mut x = self.0.load(Ordering::Relaxed) | 1;
        for b in dest.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = x as u8;
        }
        self.0.store(x, Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded(harness: Harness, inputs: u32) -> Vec<String> {
        let mut fuzzer = ScriptFuzzer::new(harness);
        let mut x: u64 = 0x2545_f491_4f6c_dd1d;
        let mut found = Vec::new();
        for round in 0..inputs {
            let mut input = vec![round as u8];
            input.extend((0..512).map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            }));
            for what in fuzzer.run(&input) {
                found.push(format!("{what} (input {round})"));
            }
        }
        found
    }

    /// A seeded run of the database harness in ordinary CI: every driver,
    /// answered by noise, fails as a driver should.
    #[test]
    fn database_drivers_refuse_noise_cleanly() {
        let _g = crate::tests::v8_guard();
        let found = seeded(Harness::DbWire, 256);
        assert!(
            found.is_empty(),
            "drivers failed badly:\n{}",
            found.join("\n")
        );
    }

    /// And of the request parsers.
    #[test]
    fn request_parsers_refuse_noise_cleanly() {
        let _g = crate::tests::v8_guard();
        let found = seeded(Harness::RequestParsers, 512);
        assert!(
            found.is_empty(),
            "parsers failed badly:\n{}",
            found.join("\n")
        );
    }

    /// The harness finds what it is for: a step that fails as a bug does is
    /// reported.
    #[test]
    fn a_bug_shaped_error_is_reported() {
        let _g = crate::tests::v8_guard();
        let mut fuzzer = ScriptFuzzer::new(Harness::RequestParsers);
        fuzzer
            .runtime_mut()
            .engine
            .eval_internal(
                "const step = globalThis.__fuzzStep; \
                 globalThis.__fuzzStep = () => { \
                   try { undefined.x; } catch (e) { __fuzzFound(e.constructor.name); } \
                   step(); };",
            )
            .unwrap();
        assert_eq!(fuzzer.run(&[0, 1, 2]), vec!["TypeError".to_string()]);
    }

    /// Every committed seed — the representative inputs, and each one that has
    /// found a bug — runs through its harness and finds nothing.
    #[test]
    fn the_committed_seeds_find_nothing() {
        let _g = crate::tests::v8_guard();
        let seeds = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fuzz/seeds");
        for (harness, dir) in [
            (Harness::DbWire, "db_wire"),
            (Harness::RequestParsers, "request_parsers"),
        ] {
            let mut fuzzer = ScriptFuzzer::new(harness);
            for entry in std::fs::read_dir(format!("{seeds}/{dir}")).expect("the seeds") {
                let path = entry.expect("a seed").path();
                let found = fuzzer.run(&std::fs::read(&path).expect("read the seed"));
                assert!(found.is_empty(), "{}: {found:?}", path.display());
            }
        }
    }

    /// The committed seeds are working sessions: each driver gets through its
    /// handshake and its query on them, so the fuzzer starts past the point
    /// noise alone never reaches. `fuzz/record-db/record.py` records them from
    /// real servers.
    #[test]
    fn the_seeds_are_sessions_that_succeed() {
        let _g = crate::tests::v8_guard();
        let mut fuzzer = ScriptFuzzer::new(Harness::DbWire);
        fuzzer
            .runtime_mut()
            .eval("globalThis.__fuzzTrace = true")
            .unwrap();
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fuzz/seeds/db_wire");
        for seed in ["postgres-disable", "postgres-prefer", "mysql", "mongodb"] {
            let input = std::fs::read(format!("{dir}/{seed}")).expect("read the seed");
            let found = fuzzer.run(&input);
            assert!(found.is_empty(), "{seed}: {found:?}");
        }
    }

    #[test]
    fn replies_are_split_where_the_input_says() {
        let split = split_replies(&[2, 1, 2, 1, 9, 3, 7]);
        assert_eq!(split, VecDeque::from([vec![1, 2], vec![9], vec![7]]));
        let whole = split_replies(&[0, 7, 8, 9]);
        assert_eq!(whole, VecDeque::from([vec![7, 8, 9]]));
    }
}
