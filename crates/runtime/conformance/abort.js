// WinterTC §2.6 — AbortController / AbortSignal.

test("AbortController starts unaborted", () => {
  const c = new AbortController();
  assertEquals(c.signal.aborted, false);
});

test("abort() flips the signal and fires the event", () => {
  const c = new AbortController();
  let fired = 0;
  c.signal.addEventListener("abort", () => fired++);
  c.abort();
  assertEquals(c.signal.aborted, true);
  assertEquals(fired, 1);
});

test("abort(reason) records the reason", () => {
  const c = new AbortController();
  c.abort("stop");
  assertEquals(c.signal.reason, "stop");
});

test("AbortSignal.abort() is pre-aborted", () => {
  const s = AbortSignal.abort("x");
  assertEquals(s.aborted, true);
  assertEquals(s.reason, "x");
});

test("throwIfAborted throws once aborted", () => {
  const c = new AbortController();
  c.signal.throwIfAborted();
  c.abort("boom");
  assertThrows(() => c.signal.throwIfAborted());
});

test("AbortSignal.any aborts when one input aborts", () => {
  const a = new AbortController();
  const b = new AbortController();
  const any = AbortSignal.any([a.signal, b.signal]);
  assertEquals(any.aborted, false);
  b.abort("b-reason");
  assertEquals(any.aborted, true);
});
