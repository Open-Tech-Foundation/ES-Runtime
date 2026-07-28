// WinterTC §2.11 — performance.

test("performance.now returns a number", () => {
  assertEquals(typeof performance.now(), "number");
});

test("performance.timeOrigin is a number", () => {
  assertEquals(typeof performance.timeOrigin, "number");
});

test("queueMicrotask runs before a resolved promise continuation completes", async () => {
  const order = [];
  await new Promise((resolve) => {
    queueMicrotask(() => order.push("micro"));
    Promise.resolve().then(() => { order.push("promise"); resolve(); });
  });
  assertEquals(order[0], "micro");
});

test("globalThis aliases self", () => {
  assert(self === globalThis);
});

test("performance exposes the User Timing entry points", () => {
  for (const m of [
    "mark", "measure", "getEntries", "getEntriesByType", "getEntriesByName",
    "clearMarks", "clearMeasures",
  ]) {
    assertEquals(typeof performance[m], "function");
  }
});

test("mark records a PerformanceMark entry", () => {
  performance.clearMarks();
  const m = performance.mark("a", { detail: { x: 1 }, startTime: 5 });
  assert(m instanceof PerformanceMark);
  assert(m instanceof PerformanceEntry);
  assertEquals(m.name, "a");
  assertEquals(m.entryType, "mark");
  assertEquals(m.startTime, 5);
  assertEquals(m.duration, 0);
  assertEquals(m.detail.x, 1);
  assertEquals(performance.getEntriesByName("a", "mark").length, 1);
  performance.clearMarks();
});

test("measure spans two marks", () => {
  performance.clearMarks();
  performance.clearMeasures();
  performance.mark("start", { startTime: 10 });
  performance.mark("end", { startTime: 35 });
  const m = performance.measure("span", "start", "end");
  assert(m instanceof PerformanceMeasure);
  assertEquals(m.entryType, "measure");
  assertEquals(m.startTime, 10);
  assertEquals(m.duration, 25);
  performance.clearMarks();
  performance.clearMeasures();
});

test("measure accepts an options bag with start, end and duration", () => {
  performance.clearMeasures();
  assertEquals(performance.measure("a", { start: 5, end: 9 }).duration, 4);
  assertEquals(performance.measure("b", { start: 5, duration: 3 }).duration, 3);
  assertEquals(performance.measure("c", { end: 20, duration: 4 }).startTime, 16);
  assertThrows(
    () => performance.measure("d", { start: 1, end: 2, duration: 3 }),
    "TypeError",
  );
  performance.clearMeasures();
});

test("measure against a missing mark is a SyntaxError", () => {
  assertThrows(() => performance.measure("m", "no-such-mark"), "SyntaxError");
});

test("clearMarks and clearMeasures remove by name and in bulk", () => {
  performance.clearMarks();
  performance.clearMeasures();
  performance.mark("keep");
  performance.mark("drop");
  performance.clearMarks("drop");
  assertEquals(performance.getEntriesByType("mark").map((e) => e.name).join(","), "keep");
  performance.clearMarks();
  assertEquals(performance.getEntries().length, 0);
});

test("PerformanceEntry is not constructible from script", () => {
  assertThrows(() => new PerformanceEntry(), "TypeError");
  assertThrows(() => new PerformanceMark(), "TypeError");
});
