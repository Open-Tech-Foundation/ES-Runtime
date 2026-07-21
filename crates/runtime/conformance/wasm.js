// WebAssembly JS API (https://webassembly.github.io/spec/js-api/).
//
// V8 owns the implementation; what is asserted here is that the runtime exposes
// it correctly — the namespace is present and complete, sync compilation and
// instantiation work, imports/exports/memory/traps behave, and the streaming
// entry points the host adds enforce their fetch-side contract.
//
// Only the synchronous API is exercised here: this harness runs each file to
// completion without a driver, so it never pumps V8's foreground task queue and
// an async compile could not settle. The async and streaming paths are covered
// as spec assertions below only where they reject *before* reaching V8; their
// resolving paths are verified end-to-end under `esrun`.

// Minimal WASM encoder — writes sections with computed lengths, so a test can
// state the module it means rather than a hand-counted byte blob.
const MAGIC = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
const str = (s) => [s.length, ...Array.from(s, (c) => c.charCodeAt(0))];
const vec = (items) => [items.length, ...items.flat()];
const section = (id, payload) => [id, payload.length, ...payload];
// A function body: size-prefixed, no locals, terminated by the `end` opcode.
const body = (code) => [code.length + 2, 0x00, ...code, 0x0b];

// (module (func (export "add") (param i32 i32) (result i32)
//   local.get 0 local.get 1 i32.add))
const ADD_MODULE = new Uint8Array([
  ...MAGIC,
  ...section(1, vec([[0x60, 0x02, 0x7f, 0x7f, 0x01, 0x7f]])), // type
  ...section(3, vec([[0x00]])), // func -> type 0
  ...section(7, vec([[...str("add"), 0x00, 0x00]])), // export
  ...section(10, vec([body([0x20, 0x00, 0x20, 0x01, 0x6a])])), // code
]);

// (module (import "env" "log" (func (param i32)))
//   (memory (export "mem") 1)
//   (global (export "answer") i32 (i32.const 42))
//   (func (export "callLog") (param i32) local.get 0 call 0)
//   (func (export "div") (param i32 i32) (result i32)
//     local.get 0 local.get 1 i32.div_s))
const HOST_MODULE = new Uint8Array([
  ...MAGIC,
  ...section(
    1,
    vec([
      [0x60, 0x01, 0x7f, 0x00],
      [0x60, 0x02, 0x7f, 0x7f, 0x01, 0x7f],
    ]),
  ),
  ...section(2, vec([[...str("env"), ...str("log"), 0x00, 0x00]])), // import
  ...section(3, vec([[0x00], [0x01]])), // funcs 1,2
  ...section(5, vec([[0x00, 0x01]])), // memory: min 1 page
  ...section(6, vec([[0x7f, 0x00, 0x41, 0x2a, 0x0b]])), // global i32 = 42
  ...section(
    7,
    vec([
      [...str("mem"), 0x02, 0x00],
      [...str("answer"), 0x03, 0x00],
      [...str("callLog"), 0x00, 0x01],
      [...str("div"), 0x00, 0x02],
    ]),
  ),
  ...section(
    10,
    vec([body([0x20, 0x00, 0x10, 0x00]), body([0x20, 0x00, 0x20, 0x01, 0x6d])]),
  ),
]);

test("WebAssembly namespace exposes the JS API", () => {
  assertEquals(typeof WebAssembly, "object");
  for (const name of [
    "Module",
    "Instance",
    "Memory",
    "Table",
    "Global",
    "CompileError",
    "LinkError",
    "RuntimeError",
  ]) {
    assert(typeof WebAssembly[name] === "function", `missing ${name}`);
  }
  for (const name of [
    "compile",
    "instantiate",
    "validate",
    "compileStreaming",
    "instantiateStreaming",
  ]) {
    assert(typeof WebAssembly[name] === "function", `missing ${name}`);
  }
});

test("host wrappers keep the native name and arity", () => {
  assertEquals(WebAssembly.compile.name, "compile");
  assertEquals(WebAssembly.instantiate.name, "instantiate");
  assertEquals(WebAssembly.compileStreaming.name, "compileStreaming");
  assertEquals(WebAssembly.instantiateStreaming.name, "instantiateStreaming");
  assertEquals(WebAssembly.compile.length, 1);
});

test("validate accepts a well-formed module and rejects junk", () => {
  assertEquals(WebAssembly.validate(ADD_MODULE), true);
  assertEquals(WebAssembly.validate(new Uint8Array([0, 1, 2, 3])), false);
});

test("new Module compiles and new Instance runs an export", () => {
  const mod = new WebAssembly.Module(ADD_MODULE);
  assert(mod instanceof WebAssembly.Module);
  const inst = new WebAssembly.Instance(mod);
  assert(inst instanceof WebAssembly.Instance);
  assertEquals(inst.exports.add(2, 3), 5);
  assertEquals(inst.exports.add(20, 22), 42);
});

test("i32 arithmetic wraps at the 32-bit boundary", () => {
  const { add } = new WebAssembly.Instance(new WebAssembly.Module(ADD_MODULE))
    .exports;
  assertEquals(add(2147483647, 1), -2147483648);
});

test("compiling malformed bytes throws CompileError", () => {
  assertThrows(
    () => new WebAssembly.Module(new Uint8Array([0, 1, 2, 3])),
    "CompileError",
  );
});

test("Module.exports describes the module's exports", () => {
  const found = WebAssembly.Module.exports(new WebAssembly.Module(ADD_MODULE));
  assertEquals(found.length, 1);
  assertEquals(found[0].name, "add");
  assertEquals(found[0].kind, "function");
});

test("Module.imports describes the module's imports", () => {
  const found = WebAssembly.Module.imports(new WebAssembly.Module(HOST_MODULE));
  assertEquals(found.length, 1);
  assertEquals(found[0].module, "env");
  assertEquals(found[0].name, "log");
});

test("an imported host function is callable from wasm", () => {
  let seen = null;
  const inst = new WebAssembly.Instance(new WebAssembly.Module(HOST_MODULE), {
    env: {
      log: (v) => {
        seen = v;
      },
    },
  });
  inst.exports.callLog(99);
  assertEquals(seen, 99);
});

// The two failure modes are distinct in the spec: a missing import *namespace*
// fails while reading the import object (TypeError), whereas a namespace that is
// present but whose member is not callable fails during linking (LinkError).
test("instantiating without the import namespace throws TypeError", () => {
  assertThrows(
    () => new WebAssembly.Instance(new WebAssembly.Module(HOST_MODULE), {}),
    "TypeError",
  );
});

test("instantiating with a non-callable import throws LinkError", () => {
  assertThrows(
    () =>
      new WebAssembly.Instance(new WebAssembly.Module(HOST_MODULE), {
        env: { log: 42 },
      }),
    "LinkError",
  );
});

test("an exported global carries its value", () => {
  const inst = new WebAssembly.Instance(new WebAssembly.Module(HOST_MODULE), {
    env: { log: () => {} },
  });
  assertEquals(inst.exports.answer.value, 42);
});

test("exported memory is readable, writable, and growable", () => {
  const inst = new WebAssembly.Instance(new WebAssembly.Module(HOST_MODULE), {
    env: { log: () => {} },
  });
  const mem = inst.exports.mem;
  assert(mem instanceof WebAssembly.Memory);
  assertEquals(mem.buffer.byteLength, 65536);

  const view = new Uint8Array(mem.buffer);
  view[0] = 7;
  assertEquals(new Uint8Array(mem.buffer)[0], 7);

  mem.grow(1);
  assertEquals(mem.buffer.byteLength, 131072);
});

test("a wasm trap surfaces as RuntimeError", () => {
  const inst = new WebAssembly.Instance(new WebAssembly.Module(HOST_MODULE), {
    env: { log: () => {} },
  });
  assertThrows(() => inst.exports.div(1, 0), "RuntimeError");
});

test("Memory, Table and Global are constructible standalone", () => {
  const mem = new WebAssembly.Memory({ initial: 1 });
  assertEquals(mem.buffer.byteLength, 65536);

  const table = new WebAssembly.Table({ initial: 2, element: "anyfunc" });
  assertEquals(table.length, 2);
  assertEquals(table.get(0), null);

  const g = new WebAssembly.Global({ value: "i32", mutable: true }, 5);
  assertEquals(g.value, 5);
  g.value = 6;
  assertEquals(g.value, 6);
});

test("an immutable Global rejects assignment", () => {
  const g = new WebAssembly.Global({ value: "i32", mutable: false }, 5);
  assertThrows(() => {
    g.value = 6;
  });
});

test("streaming entry points reject a non-Response source", async () => {
  let err = null;
  await WebAssembly.compileStreaming(ADD_MODULE).catch((e) => {
    err = e;
  });
  assert(err instanceof TypeError, `expected TypeError, got ${err}`);
});

test("streaming entry points reject a non-wasm Content-Type", async () => {
  let err = null;
  await WebAssembly.compileStreaming(
    new Response(ADD_MODULE, { headers: { "content-type": "text/plain" } }),
  ).catch((e) => {
    err = e;
  });
  assert(err instanceof TypeError, `expected TypeError, got ${err}`);
  assert(
    /application\/wasm/.test(err.message),
    `message should name the expected type, got: ${err.message}`,
  );
});
