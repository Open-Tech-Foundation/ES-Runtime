// navigator (SPEC §2.1) — the WinterTC Minimum Common API's one required
// member, `navigator.userAgent`, and the interface shape around it.
//
// Cases still written as `todo` are known deviations; see RESULTS.md.

test("navigator exposes a userAgent string", () => {
  assertEquals(typeof navigator, "object");
  assertEquals(typeof navigator.userAgent, "string");
  assert(navigator.userAgent.length > 0, "userAgent must not be empty");
});

test("userAgent names the runtime and a version", () => {
  // WinterTC asks for a string identifying the runtime; the shape every other
  // implementation uses is `Name/version`.
  const [name, version] = navigator.userAgent.split("/");
  assertEquals(name, "ES-Runtime");
  assert(/^\d+\.\d+\.\d+/.test(version), `not a version: ${version}`);
});

test("navigator is a branded Navigator instance", () => {
  assert(navigator instanceof Navigator, "navigator must be a Navigator");
  assertEquals(Object.prototype.toString.call(navigator), "[object Navigator]");
});

test("userAgent lives on the prototype, not the instance", () => {
  assertEquals(Object.getOwnPropertyDescriptor(navigator, "userAgent"), undefined);
  const d = Object.getOwnPropertyDescriptor(Navigator.prototype, "userAgent");
  assertEquals(typeof d.get, "function");
  assertEquals(d.set, undefined);
});

test("Navigator is not constructible by a script", () => {
  assertThrows(() => new Navigator(), "TypeError");
});
