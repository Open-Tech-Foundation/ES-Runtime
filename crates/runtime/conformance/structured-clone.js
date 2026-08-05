// WinterTC §2.1 — structuredClone.

test("structuredClone copies plain objects deeply", () => {
  const src = { a: 1, b: { c: [2, 3] } };
  const out = structuredClone(src);
  assert(out !== src);
  assert(out.b !== src.b);
  assertEquals(out.b.c[1], 3);
});

test("structuredClone handles cycles", () => {
  const a = { name: "a" };
  a.self = a;
  const out = structuredClone(a);
  assert(out.self === out);
  assertEquals(out.name, "a");
});

test("structuredClone clones Map and Set", () => {
  const m = new Map([["k", 1]]);
  const s = new Set([1, 2]);
  const om = structuredClone(m);
  const os = structuredClone(s);
  assert(om instanceof Map);
  assertEquals(om.get("k"), 1);
  assert(os instanceof Set);
  assertEquals(os.has(2), true);
});

test("structuredClone clones typed arrays and ArrayBuffer", () => {
  const u = new Uint8Array([1, 2, 3]);
  const out = structuredClone(u);
  assert(out instanceof Uint8Array);
  assert(out.buffer !== u.buffer);
  assertEquals(out[2], 3);
});

test("structuredClone clones Date", () => {
  const d = new Date(1234567890000);
  const out = structuredClone(d);
  assert(out instanceof Date);
  assertEquals(out.getTime(), 1234567890000);
});

test("structuredClone throws on functions", () => {
  assertThrows(() => structuredClone(() => 1), "DataCloneError");
});

test("structuredClone clones RegExp and DataView", () => {
  const rx = /abc/gi;
  const crx = structuredClone(rx);
  assert(crx instanceof RegExp);
  assertEquals(crx.source, "abc");
  assertEquals(crx.flags, "gi");
  const dv = new DataView(new Uint8Array([1, 2, 3, 4]).buffer);
  const cdv = structuredClone(dv);
  assert(cdv instanceof DataView);
  assertEquals(cdv.getUint8(1), 2);
});


test("structuredClone preserves an Error's cause", () => {
  assertEquals(structuredClone(new Error("m", { cause: "c" })).cause, "c");
});

test("structuredClone preserves a DOMException's name", () => {
  assertEquals(structuredClone(new DOMException("m", "AbortError")).name, "AbortError");
});

test("structuredClone clones a Blob", () => {
  const b = structuredClone(new Blob(["x"], { type: "text/plain" }));
  assertEquals(b.size, 1);
  assertEquals(b.type, "text/plain");
});

test("structuredClone detaches a transferred ArrayBuffer", () => {
  const ab = new ArrayBuffer(8);
  structuredClone(ab, { transfer: [ab] });
  assertEquals(ab.byteLength, 0);
});

test("structuredClone preserves the standard Error subclasses", () => {
  for (const Ctor of [Error, EvalError, RangeError, ReferenceError, SyntaxError, TypeError, URIError]) {
    const c = structuredClone(new Ctor("m"));
    assertEquals(c.name, Ctor.name);
    assertEquals(c.message, "m");
    assert(c instanceof Ctor);
  }
});

test("structuredClone carries an Error's stack over", () => {
  const e = new Error("m");
  assertEquals(structuredClone(e).stack, e.stack);
});

test("structuredClone clones a nested error cause", () => {
  const c = structuredClone(new Error("outer", { cause: new TypeError("inner") }));
  assertEquals(c.cause.name, "TypeError");
  assertEquals(c.cause.message, "inner");
});

test("structuredClone clones a File with its name and lastModified", () => {
  const f = structuredClone(new File(["xy"], "n.txt", { type: "text/plain", lastModified: 7 }));
  assert(f instanceof File);
  assertEquals(f.name, "n.txt");
  assertEquals(f.size, 2);
  assertEquals(f.lastModified, 7);
});

test("structuredClone moves a transferred buffer's contents into the clone", () => {
  const ab = new Uint8Array([1, 2, 3]).buffer;
  const out = structuredClone(ab, { transfer: [ab] });
  assertEquals(ab.byteLength, 0);
  assertEquals(out.byteLength, 3);
  assertEquals(new Uint8Array(out)[2], 3);
});

test("structuredClone rejects a non-ArrayBuffer in the transfer list", () => {
  assertThrows(() => structuredClone({}, { transfer: [{}] }), "DataCloneError");
});

// The spec detaches *after* serializing, so a view over a buffer in the
// transfer list is serialized while the buffer is still live: the clone carries
// the data and the source is left detached. (This asserted a DataCloneError
// while the clone was hand-written in JS, which was a misreading — the
// DataCloneError is for a buffer already detached on the way in, below.)
test("structuredClone serializes a view onto a transferred buffer", () => {
  const view = new Uint8Array([1, 2, 3]);
  const out = structuredClone(view, { transfer: [view.buffer] });
  assertEquals(out.join(","), "1,2,3");
  assertEquals(view.byteLength, 0);
  assertEquals(view.buffer.detached, true);
});

test("structuredClone rejects an already-detached buffer in the transfer list", () => {
  const buffer = new ArrayBuffer(8);
  buffer.transfer();
  assertThrows(() => structuredClone({}, { transfer: [buffer] }), "DataCloneError");
});

test("structuredClone rebuilds an ordinary object as a plain object", () => {
  // StructuredSerialize walks an ordinary object's own enumerable String-keyed
  // properties; the prototype is not carried, so the clone is a plain object.
  class Point {
    constructor() {
      this.x = 1;
    }
  }
  const out = structuredClone(new Point());
  assertEquals(out.x, 1);
  assertEquals(Object.getPrototypeOf(out), Object.prototype);
});

test("structuredClone drops symbol-keyed properties", () => {
  const src = { kept: 1 };
  Object.defineProperty(src, Symbol("dropped"), { value: 2, enumerable: true });
  const out = structuredClone(src);
  assertEquals(out.kept, 1);
  assertEquals(Object.getOwnPropertySymbols(out).length, 0);
});

test("structuredClone round-trips Blob and File", () => {
  const blob = structuredClone(new Blob(["hi"], { type: "text/plain" }));
  assertEquals(blob instanceof Blob, true);
  assertEquals(blob.type, "text/plain");
  assertEquals(blob.size, 2);

  const file = structuredClone(
    new File(["x"], "a.txt", { type: "text/plain", lastModified: 5 }),
  );
  assertEquals(file instanceof File, true);
  assertEquals(file.name, "a.txt");
  assertEquals(file.lastModified, 5);
});
