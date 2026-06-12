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
