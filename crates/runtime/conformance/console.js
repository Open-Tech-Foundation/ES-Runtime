// console (SPEC §2.2) — the method set the Console Standard defines.
//
// Output *content* is asserted by Rust tests instead: they can inject a
// capturing Console sink, which this pure-JS harness cannot.

test("console exposes the standard method set", () => {
  const methods = [
    "assert", "clear", "count", "countReset", "debug", "dir", "dirxml",
    "error", "group", "groupCollapsed", "groupEnd", "info", "log", "table",
    "time", "timeEnd", "timeLog", "trace", "warn",
  ];
  for (const name of methods) {
    assertEquals(typeof console[name], "function", `console.${name}`);
  }
});

test("console methods return undefined", () => {
  // Every operation in the standard returns undefined; a value coming back
  // would mean something was being used as an expression.
  assertEquals(console.log("conformance: return value check"), undefined);
  assertEquals(console.group(), undefined);
  assertEquals(console.groupEnd(), undefined);
  assertEquals(console.countReset("conformance-unused"), undefined);
});

test("console.assert only reports a falsy condition", () => {
  // A truthy assertion prints nothing; the falsy branch is asserted Rust-side
  // where the sink can be read back.
  assertEquals(console.assert(true, "must not print"), undefined);
});
