// WinterTC §2.7 — Event / EventTarget / CustomEvent.

test("dispatchEvent invokes listeners", () => {
  const t = new EventTarget();
  let seen = 0;
  t.addEventListener("x", () => seen++);
  t.dispatchEvent(new Event("x"));
  assertEquals(seen, 1);
});

test("removeEventListener stops delivery", () => {
  const t = new EventTarget();
  let seen = 0;
  const fn = () => seen++;
  t.addEventListener("x", fn);
  t.removeEventListener("x", fn);
  t.dispatchEvent(new Event("x"));
  assertEquals(seen, 0);
});

test("once listeners fire a single time", () => {
  const t = new EventTarget();
  let seen = 0;
  t.addEventListener("x", () => seen++, { once: true });
  t.dispatchEvent(new Event("x"));
  t.dispatchEvent(new Event("x"));
  assertEquals(seen, 1);
});

test("CustomEvent carries detail", () => {
  const t = new EventTarget();
  let got = null;
  t.addEventListener("x", (e) => { got = e.detail; });
  t.dispatchEvent(new CustomEvent("x", { detail: { n: 42 } }));
  assertEquals(got.n, 42);
});

test("Event type and default flags", () => {
  const e = new Event("test");
  assertEquals(e.type, "test");
  assertEquals(e.bubbles, false);
  assertEquals(e.cancelable, false);
  assertEquals(e.defaultPrevented, false);
});

test("preventDefault sets defaultPrevented on cancelable events", () => {
  const e = new Event("x", { cancelable: true });
  e.preventDefault();
  assertEquals(e.defaultPrevented, true);
});

test("stopImmediatePropagation halts later listeners", () => {
  const t = new EventTarget();
  let a = 0, b = 0;
  t.addEventListener("x", (e) => { a++; e.stopImmediatePropagation(); });
  t.addEventListener("x", () => { b++; });
  t.dispatchEvent(new Event("x"));
  assertEquals(a, 1);
  assertEquals(b, 0);
});

test("EventTarget accepts options object in addEventListener", () => {
  const t = new EventTarget();
  let count = 0;
  t.addEventListener("e", () => count++, { capture: true });
  t.dispatchEvent(new Event("e"));
  assertEquals(count, 1);
});

test("Event target and timeStamp properties are populated", () => {
  const t = new EventTarget();
  let target = null, timeStamp = null;
  t.addEventListener("e", (ev) => { target = ev.target; timeStamp = ev.timeStamp; });
  const e = new Event("e");
  t.dispatchEvent(e);
  assertEquals(target, t);
  assertEquals(typeof timeStamp, "number");
});


todo("Event exposes the legacy cancelBubble and returnValue accessors", () => {
  const e = new Event("x", { cancelable: true });
  assertEquals(e.cancelBubble, false);
  assertEquals(e.returnValue, true);
  e.preventDefault();
  assertEquals(e.returnValue, false);
});

todo("Event exposes the legacy initEvent method", () => {
  assertEquals(typeof new Event("x").initEvent, "function");
});

todo("dispatching an in-flight event throws InvalidStateError", () => {
  const t = new EventTarget();
  const e = new Event("x");
  let name = null;
  t.addEventListener("x", () => {
    try { t.dispatchEvent(e); } catch (err) { name = err.name; }
  });
  t.dispatchEvent(e);
  assertEquals(name, "InvalidStateError");
});
