// Minimal WebAssembly binary emitter, shared by the wasm_* / wasi_* workloads.
//
// The modules are assembled here rather than checked in as `.wasm` fixtures so
// every runtime compiles byte-identical input, and so a workload can vary a
// constant per iteration to defeat compilation caches.

const I32 = 0x7f;
const I64 = 0x7e;

const uleb = (n) => {
  const out = [];
  do {
    let b = n & 0x7f;
    n >>>= 7;
    if (n !== 0) b |= 0x80;
    out.push(b);
  } while (n !== 0);
  return out;
};

const sleb = (n) => {
  const out = [];
  for (;;) {
    let b = n & 0x7f;
    n >>= 7;
    if ((n === 0 && !(b & 0x40)) || (n === -1 && b & 0x40)) {
      out.push(b);
      return out;
    }
    out.push(b | 0x80);
  }
};

const vec = (items) => [...uleb(items.length), ...items.flat()];
const section = (id, content) => [id, ...uleb(content.length), ...content];
const str = (s) => [...uleb(s.length), ...Array.from(s, (c) => c.charCodeAt(0))];
const funcType = (params, results) => [0x60, ...vec(params.map((t) => [t])), ...vec(results.map((t) => [t]))];

// A code-section entry: local declarations + body + `end`, prefixed by its size.
const code = (locals, body) => {
  const inner = [...vec(locals.map(([n, t]) => [...uleb(n), t])), ...body, 0x0b];
  return [...uleb(inner.length), ...inner];
};

const i32c = (n) => [0x41, ...sleb(n)];
const i64c = (n) => [0x42, ...sleb(n)];
const get = (i) => [0x20, ...uleb(i)];
const set = (i) => [0x21, ...uleb(i)];
const call = (i) => [0x10, ...uleb(i)];
const ADD = 0x6a;
const MUL = 0x6c;
const GE_S = 0x4e;
const DROP = 0x1a;

// `for (i = 0; i < <bound>; i++) <body>` where `i` is local `iIdx` and `bound`
// is the value pushed by `boundExpr`. Emitted as WebAssembly's block/loop pair:
// `br_if 1` leaves the block when the counter is spent, `br 0` repeats the loop.
const forLoop = (iIdx, boundExpr, body) => [
  ...i32c(0), ...set(iIdx),
  0x02, 0x40,
  0x03, 0x40,
  ...get(iIdx), ...boundExpr, GE_S, 0x0d, 0x01,
  ...body,
  ...get(iIdx), ...i32c(1), ADD, ...set(iIdx),
  0x0c, 0x00,
  0x0b,
  0x0b,
];

const MAGIC = [0, 0x61, 0x73, 0x6d, 1, 0, 0, 0];

/**
 * Compute module: `add(a, b)` for the call-overhead workload and `sum(n)` for
 * a loop that stays inside wasm. One memory page is exported so the module has
 * the same shape as the others.
 */
export function computeModule() {
  const add = [...get(0), ...get(1), ADD];
  // acc += i * i, summed in wasm (locals: n=0, i=1, acc=2).
  const sum = [
    ...i32c(0), ...set(2),
    ...forLoop(1, get(0), [...get(2), ...get(1), ...get(1), MUL, ADD, ...set(2)]),
    ...get(2),
  ];
  return new Uint8Array([
    ...MAGIC,
    ...section(1, vec([funcType([I32, I32], [I32]), funcType([I32], [I32])])),
    ...section(3, vec([[0x00], [0x01]])),
    ...section(5, vec([[0x00, 0x01]])),
    ...section(7, vec([
      [...str("memory"), 0x02, 0x00],
      [...str("add"), 0x00, 0x00],
      [...str("sum"), 0x00, 0x01],
    ])),
    ...section(10, vec([code([], add), code([[2, I32]], sum)])),
  ]);
}

/**
 * Memory module: `sum8(ptr, len)` adds `len` bytes of linear memory, so the
 * workload can hand wasm a buffer JS filled in place.
 */
export function memoryModule(pages) {
  // locals: ptr=0, len=1, i=2, acc=3.
  const body = [
    ...i32c(0), ...set(3),
    ...forLoop(2, get(1), [
      ...get(3),
      ...get(0), ...get(2), ADD, 0x2d, 0x00, 0x00, // i32.load8_u
      ADD, ...set(3),
    ]),
    ...get(3),
  ];
  return new Uint8Array([
    ...MAGIC,
    ...section(1, vec([funcType([I32, I32], [I32])])),
    ...section(3, vec([[0x00]])),
    ...section(5, vec([[0x00, ...uleb(pages)]])),
    ...section(7, vec([[...str("memory"), 0x02, 0x00], [...str("sum8"), 0x00, 0x00]])),
    ...section(10, vec([code([[2, I32]], body)])),
  ]);
}

/**
 * A large module for the compile workload: `funcs` functions, each a
 * straight-line chain of `chain` arithmetic pairs. `salt` shifts every constant
 * so successive modules differ byte for byte and no compilation cache hits.
 */
export function bigModule({ funcs = 400, chain = 40, salt = 0 } = {}) {
  const bodies = [];
  const exports = [];
  for (let f = 0; f < funcs; f++) {
    const body = [...get(0)];
    for (let c = 0; c < chain; c++) {
      body.push(...i32c(salt + f + c + 1), ADD, ...i32c((c % 7) + 2), MUL);
    }
    bodies.push(code([], body));
    exports.push([...str("f" + f), 0x00, ...uleb(f)]);
  }
  return new Uint8Array([
    ...MAGIC,
    ...section(1, vec([funcType([I32], [I32])])),
    ...section(3, vec(bodies.map(() => [0x00]))),
    ...section(7, vec(exports)),
    ...section(10, vec(bodies)),
  ]);
}

/**
 * A WASI command module. `_start` runs `syscalls` iterations of
 * `random_get` + `clock_time_get`, then a short compute loop — so
 * `syscalls: 0` measures WASI bootstrap alone and a larger count measures the
 * syscall path from inside the guest, where a real program calls it from.
 */
export function wasiModule({ syscalls = 0 } = {}) {
  // Imported functions occupy the low function indices: random_get=0,
  // clock_time_get=1, so the defined `_start` is function 2.
  const RANDOM_GET = 0;
  const CLOCK_TIME_GET = 1;
  const loop = syscalls === 0 ? [] : forLoop(0, i32c(syscalls), [
    ...i32c(0), ...i32c(16), ...call(RANDOM_GET), DROP,
    ...i32c(1), ...i64c(0), ...i32c(32), ...call(CLOCK_TIME_GET), DROP,
  ]);
  return new Uint8Array([
    ...MAGIC,
    ...section(1, vec([
      funcType([I32, I32], [I32]),           // random_get
      funcType([I32, I64, I32], [I32]),      // clock_time_get
      funcType([], []),                      // _start
    ])),
    ...section(2, vec([
      [...str("wasi_snapshot_preview1"), ...str("random_get"), 0x00, 0x00],
      [...str("wasi_snapshot_preview1"), ...str("clock_time_get"), 0x00, 0x01],
    ])),
    ...section(3, vec([[0x02]])),
    ...section(5, vec([[0x00, 0x01]])),
    ...section(7, vec([[...str("memory"), 0x02, 0x00], [...str("_start"), 0x00, 0x02]])),
    ...section(10, vec([code([[1, I32]], loop)])),
  ]);
}

/**
 * Resolves the runtime's `WASI` constructor: `runtime:wasi` on esrun,
 * `node:wasi` on Node/Bun/Deno. Returns null where there is none, so the
 * workload can report n/a instead of failing.
 */
export async function loadWASI() {
  for (const spec of ["runtime:wasi", "node:wasi"]) {
    try {
      const mod = await import(spec);
      if (typeof mod.WASI === "function") return mod.WASI;
    } catch {}
  }
  return null;
}
