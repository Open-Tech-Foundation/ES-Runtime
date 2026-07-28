// WinterTC §2.5 — setTimeout / setInterval / queueMicrotask.
//
// Cases still written as `todo` are known deviations; see RESULTS.md.

test("setTimeout returns an opaque numeric handle", () => {
  const id = setTimeout(() => {}, 0);
  assertEquals(typeof id, "number");
  clearTimeout(id);
});

test("the timer entry points are all present", () => {
  for (const f of [setTimeout, clearTimeout, setInterval, clearInterval, queueMicrotask]) {
    assertEquals(typeof f, "function");
  }
});

test("clearTimeout on an unknown handle is a no-op", () => {
  clearTimeout(999999);
  clearInterval(999999);
});

// setTimeout's trailing-argument forwarding needs a driven event loop, which
// this harness does not run; it is gated by a Rust test instead.
