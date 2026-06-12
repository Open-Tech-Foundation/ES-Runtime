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
