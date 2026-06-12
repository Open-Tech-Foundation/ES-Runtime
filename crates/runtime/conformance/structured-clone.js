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
