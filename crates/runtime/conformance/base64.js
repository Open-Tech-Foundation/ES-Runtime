// WinterTC §2.3 — atob / btoa.

test("btoa encodes ASCII", () => {
  assertEquals(btoa("hello"), "aGVsbG8=");
});

test("atob decodes base64", () => {
  assertEquals(atob("aGVsbG8="), "hello");
});

test("btoa/atob round-trip", () => {
  const s = "The quick brown fox.";
  assertEquals(atob(btoa(s)), s);
});

test("btoa throws on non-Latin1 characters", () => {
  assertThrows(() => btoa("Ā"), "InvalidCharacterError");
});

test("atob throws on invalid base64", () => {
  assertThrows(() => atob("a"), "InvalidCharacterError");
});

test("btoa of empty string is empty", () => {
  assertEquals(btoa(""), "");
  assertEquals(atob(""), "");
});
