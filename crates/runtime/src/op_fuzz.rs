//! Fuzzing the op boundary: every op, called with whatever arguments an input
//! describes (SPEC §5, D253).
//!
//! The other targets reach a parser directly. This one reaches the code that
//! turns a JavaScript call into a Rust one — the marshaler, the typed-argument
//! readers, and each op's handling of what it was handed — which only a live
//! isolate can exercise. Standing one up per input would cost milliseconds and
//! mostly fuzz V8, so one runtime serves many inputs, and is rebuilt after
//! [`REBUILD_EVERY`] of them so that handles and pending work cannot pile up.
//!
//! **What counts as a finding is a Rust panic.** An op handler may refuse
//! anything, with any error; it may not panic. The engine contains a panic as
//! the JS error "internal error in host op" so that it cannot unwind through
//! V8, which hides it from a fuzzer — so the harness looks for exactly that
//! error, from a synchronous call or a rejected promise, and reports it.
//!
//! **What is not called.** Every capability is granted and no provider that
//! reaches the host is installed, so a gated op gets as far as its argument
//! checks and then finds nothing to act on — this fuzzes the boundary, not the
//! machine. The few ops that act on the process itself are skipped by name
//! ([`SKIP`]), as are the ones that do deliberately expensive work (an RSA key,
//! a password hash), whose cost is bounded by D241 and is not a crash.
//!
//! Driven by the cargo-fuzz target `fuzz/fuzz_targets/ops.rs`, and by a
//! deterministic run in the runtime's own tests, so ordinary CI calls every op
//! with a few thousand argument lists too.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use es_runtime_common::{CapabilitySet, Limits};
use es_runtime_engine::{OpDecl, V8Engine, Value};
use es_runtime_providers::{
    BoxFuture, Clock, Console, ConsoleLevel, Entropy, HttpRequest, HttpResponse, NetTransport,
    ProviderError,
};

use crate::{HostProviders, Runtime};

/// Inputs one runtime serves before it is rebuilt.
pub const REBUILD_EVERY: u32 = 256;

/// Ops never called, and why.
pub const SKIP: &[&str] = &[
    // They act on the fuzzing process itself: end it, or block it reading or
    // reconfiguring its terminal.
    "process_exit",
    "process_stdin_read",
    "process_stdin_read_line_sync",
    "process_stdin_set_raw",
    "process_write",
    // Deliberately expensive in proportion to an argument (D241): slow, not
    // broken.
    "rsa_generate_pkcs8",
    "hash_password",
    "hash_password_verify",
    "subtle_pbkdf2",
    // The harness's own.
    "__fuzz_input",
    "__fuzz_found",
];

/// The harness. Runs as internal code, so `__ops` is the op table; it installs
/// one global, `__fuzzStep`, which reads the next input through
/// `__fuzz_input()` and reports a contained panic through `__fuzz_found()`.
const HARNESS: &str = r#"
const SKIP = new Set(__SKIP__);
const names = Object.keys(__ops).filter((n) => !SKIP.has(n)).sort();
const PANIC = "internal error in host op";
let input = new Uint8Array(0);
let pos = 0;
const byte = () => (pos < input.length ? input[pos++] : 0);
const u32 = () => (byte() | (byte() << 8) | (byte() << 16) | (byte() << 24)) >>> 0;
const f64 = () => {
  const b = new Uint8Array(8);
  for (let i = 0; i < 8; i++) b[i] = byte();
  return new Float64Array(b.buffer)[0];
};
const text = () => {
  const n = byte() % 48;
  let s = "";
  for (let i = 0; i < n; i++) s += String.fromCharCode(byte() & 1 ? (byte() << 8) | byte() : byte());
  return s;
};
const bytes = () => {
  const n = u32() % 4097;
  const b = new Uint8Array(n);
  for (let i = 0; i < n && pos < input.length; i++) b[i] = byte();
  return b;
};
const detached = () => {
  const buffer = new ArrayBuffer(16);
  const view = new Uint8Array(buffer);
  structuredClone(buffer, { transfer: [buffer] });
  return view;
};
const hostile = () => {
  const trap = () => { throw new Error("trap"); };
  return new Proxy({}, { get: trap, has: trap, ownKeys: trap, getOwnPropertyDescriptor: trap, getPrototypeOf: trap });
};
function value(depth) {
  switch (byte() % 34) {
    case 0: return undefined;
    case 1: return null;
    case 2: return true;
    case 3: return false;
    case 4: return 0;
    case 5: return -0;
    case 6: return byte() - 128;
    case 7: return u32();
    case 8: return u32() | 0;
    case 9: return f64();
    case 10: return NaN;
    case 11: return Infinity;
    case 12: return -Infinity;
    case 13: return 2 ** 53 + byte();
    case 14: return 2 ** 64;
    case 15: return BigInt(u32()) * BigInt(u32());
    case 16: return -(2n ** 127n) - BigInt(byte());
    case 17: return text();
    case 18: return "\uD800" + text() + "\uDFFF";
    case 19: return "x".repeat(u32() % 70000);
    case 20: return bytes();
    case 21: return new Uint8Array(new SharedArrayBuffer(byte()));
    case 22: return detached();
    case 23: return new DataView(new ArrayBuffer(byte()), byte() % 4);
    case 24: return new Float64Array(byte() % 9);
    case 25: return new BigInt64Array(byte() % 5);
    case 26: {
      if (depth > 3) return [];
      const a = [];
      for (let n = byte() % 6; n > 0; n--) a.push(value(depth + 1));
      return a;
    }
    case 27: {
      if (depth > 3) return {};
      const o = {};
      for (let n = byte() % 6; n > 0; n--) o[text()] = value(depth + 1);
      return o;
    }
    case 28: { const o = {}; o.self = o; return o; }
    case 29: { let a = []; for (let i = 0; i < 1 + (u32() % 3000); i++) a = [a]; return a; }
    case 30: return { get x() { throw new Error("getter"); }, [Symbol.toPrimitive]() { throw new Error("prim"); } };
    case 31: return Symbol(text());
    case 32: return hostile();
    default: return Object.create(null);
  }
}
const check = (name, e) => {
  if (e && e.message === PANIC) __ops.__fuzz_found(name);
};
globalThis.__fuzzStep = () => {
  input = __ops.__fuzz_input();
  pos = 0;
  if (names.length === 0) return;
  const name = names[u32() % names.length];
  globalThis.__fuzzCalls = (globalThis.__fuzzCalls ?? 0) + 1;
  const args = [];
  for (let n = byte() % 7; n > 0; n--) args.push(value(0));
  try {
    const result = __ops[name](...args);
    if (result && typeof result.then === "function") result.then(undefined, (e) => check(name, e));
  } catch (e) {
    check(name, e);
  }
};
"#;

/// One runtime, rebuilt every [`REBUILD_EVERY`] inputs, calling ops.
pub struct OpFuzzer {
    /// `None` only between dropping one runtime and building the next: V8
    /// requires an isolate gone before the one after it is made.
    runtime: Option<Runtime>,
    input: Rc<RefCell<Vec<u8>>>,
    found: Rc<RefCell<Vec<String>>>,
    served: u32,
    /// An op added to every runtime this builds — a test's planted bug.
    extra: Option<fn() -> OpDecl>,
}

impl OpFuzzer {
    /// A fuzzer with a fresh runtime.
    ///
    /// # Panics
    ///
    /// If the runtime cannot be built — a broken build, not a finding.
    pub fn new() -> Self {
        Self::with(None)
    }

    fn with(extra: Option<fn() -> OpDecl>) -> Self {
        let input = Rc::new(RefCell::new(Vec::new()));
        let found = Rc::new(RefCell::new(Vec::new()));
        let runtime = Some(build(&input, &found, extra));
        OpFuzzer {
            runtime,
            input,
            found,
            served: 0,
            extra,
        }
    }

    /// The ops the harness chooses among, sorted — the first four bytes of an
    /// input pick one.
    pub fn op_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .runtime()
            .engine
            .op_requirements()
            .into_iter()
            .map(|(name, _)| name)
            .filter(|name| !SKIP.contains(&name.as_str()))
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Calls one op as `input` describes, runs whatever it started for a few
    /// turns, and returns the names of ops that panicked.
    pub fn call(&mut self, input: &[u8]) -> Vec<String> {
        if self.served == REBUILD_EVERY {
            self.rebuild();
        }
        self.served += 1;
        input.clone_into(&mut self.input.borrow_mut());
        if self.runtime_mut().eval("__fuzzStep()").is_err() {
            // Terminated (the heap limit) or wedged: start again rather than
            // carry a broken isolate into the next input.
            self.rebuild();
        } else {
            for _ in 0..4 {
                self.runtime_mut().tick(0);
            }
        }
        std::mem::take(&mut self.found.borrow_mut())
    }

    fn rebuild(&mut self) {
        self.runtime = None;
        self.runtime = Some(build(&self.input, &self.found, self.extra));
        self.served = 0;
    }

    fn runtime(&self) -> &Runtime {
        self.runtime.as_ref().expect("a runtime between inputs")
    }

    fn runtime_mut(&mut self) -> &mut Runtime {
        self.runtime.as_mut().expect("a runtime between inputs")
    }

    /// What one step threw, when it threw — the harness's own failures.
    #[cfg(test)]
    fn step_error(&mut self, input: &[u8]) -> Option<String> {
        input.clone_into(&mut self.input.borrow_mut());
        self.runtime_mut()
            .eval("__fuzzStep()")
            .err()
            .map(|e| e.to_string())
    }
}

impl Default for OpFuzzer {
    fn default() -> Self {
        Self::new()
    }
}

fn build(
    input: &Rc<RefCell<Vec<u8>>>,
    found: &Rc<RefCell<Vec<String>>>,
    extra: Option<fn() -> OpDecl>,
) -> Runtime {
    // A heap cap well under what a fuzzer allows a process, so an op asked to
    // build something huge ends the isolate rather than the run.
    let limits = Limits::default().with_heap_limit_bytes(256 << 20);
    let engine = V8Engine::new(limits).expect("engine");
    let providers = HostProviders::new(
        Arc::new(StillClock),
        Arc::new(Silent),
        Arc::new(NoNetwork),
        Arc::new(Xorshift(std::sync::atomic::AtomicU64::new(0x9e37_79b9))),
    );
    let mut runtime = Runtime::new(Box::new(engine), providers).expect("runtime");
    runtime.set_capabilities(CapabilitySet::all());

    if let Some(extra) = extra {
        runtime.register_op(extra()).expect("register the extra op");
    }
    let source = input.clone();
    runtime
        .register_op(OpDecl::sync("__fuzz_input", move |_| {
            Ok(Value::Bytes(source.borrow().clone()))
        }))
        .expect("register __fuzz_input");
    let sink = found.clone();
    runtime
        .register_op(OpDecl::sync("__fuzz_found", move |args| {
            let name = match args.first() {
                Some(Value::String(name)) => name.clone(),
                _ => String::from("?"),
            };
            sink.borrow_mut().push(name);
            Ok(Value::Undefined)
        }))
        .expect("register __fuzz_found");

    let skip = format!(
        "[{}]",
        SKIP.iter()
            .map(|name| format!("{name:?}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    runtime
        .engine
        .eval_internal(&HARNESS.replace("__SKIP__", &skip))
        .expect("install the harness");
    runtime
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

struct NoNetwork;

impl NetTransport for NoNetwork {
    fn fetch(&self, _request: HttpRequest) -> BoxFuture<Result<HttpResponse, ProviderError>> {
        Box::pin(async { Err(ProviderError::Other("no network while fuzzing".into())) })
    }
}

struct Xorshift(std::sync::atomic::AtomicU64);

impl Entropy for Xorshift {
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

    /// A seeded run over every op, in ordinary CI: a few thousand calls, each
    /// a fresh argument list, and not one may panic. The cargo-fuzz target runs
    /// the same harness for as long as it is given.
    #[test]
    fn no_op_panics_on_arbitrary_arguments() {
        let _g = crate::tests::v8_guard();
        let mut fuzzer = OpFuzzer::new();
        let ops = fuzzer.op_names().len() as u32;
        assert!(ops > 300, "the harness sees only {ops} ops");
        let mut x: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        assert_eq!(fuzzer.step_error(&[0; 8]), None, "the harness itself fails");
        let mut found = Vec::new();
        // Every op at least eight times: the first four bytes pick it.
        for round in 0..ops * 8 {
            let mut input: Vec<u8> = (round % ops).to_le_bytes().to_vec();
            input.extend((0..96).map(|_| next() as u8));
            for name in fuzzer.call(&input) {
                found.push(format!("{name} (input {input:02x?})"));
            }
        }
        assert!(found.is_empty(), "ops panicked:\n{}", found.join("\n"));
    }

    /// The harness finds what it is for: a planted op that panics on one
    /// argument, synchronously, is reported by name — and an op is in fact
    /// called once per input.
    #[test]
    fn a_panicking_op_is_found() {
        let _g = crate::tests::v8_guard();
        fn planted() -> OpDecl {
            OpDecl::sync("aaa_planted", |args| match args.first() {
                Some(Value::Bool(true)) => panic!("planted"),
                _ => Ok(Value::Undefined),
            })
        }
        let mut fuzzer = OpFuzzer::with(Some(planted));
        let index = fuzzer
            .op_names()
            .iter()
            .position(|name| name == "aaa_planted")
            .expect("the planted op is offered") as u32;
        // Select it, one argument, of kind 2 (`true`).
        let mut input = index.to_le_bytes().to_vec();
        input.extend([1, 2]);
        assert_eq!(fuzzer.call(&input), ["aaa_planted"]);
        // Kind 3 (`false`) does not panic.
        input[5] = 3;
        assert!(fuzzer.call(&input).is_empty());
        let calls = fuzzer.runtime_mut().eval("globalThis.__fuzzCalls").unwrap();
        assert_eq!(calls, Value::Number(2.0));
    }
}
