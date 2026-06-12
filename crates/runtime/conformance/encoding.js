// WinterTC §2.3 — TextEncoder / TextDecoder (UTF-8).

test("TextEncoder encodes ASCII", () => {
  const b = new TextEncoder().encode("abc");
  assert(b instanceof Uint8Array);
  assertEquals(b.length, 3);
  assertEquals(b[0], 97);
});

test("TextEncoder encoding property is utf-8", () => {
  assertEquals(new TextEncoder().encoding, "utf-8");
});

test("TextEncoder counts multibyte code points", () => {
  // "héllo😀": h(1) é(2) l(1) l(1) o(1) 😀(4) = 10 bytes.
  assertEquals(new TextEncoder().encode("héllo😀").length, 10);
});

test("TextDecoder round-trips UTF-8", () => {
  const enc = new TextEncoder();
  const dec = new TextDecoder();
  assertEquals(dec.decode(enc.encode("héllo😀")), "héllo😀");
});

test("TextDecoder default replaces invalid sequences", () => {
  // Lone 0xFF is invalid UTF-8 → U+FFFD by default.
  const out = new TextDecoder().decode(new Uint8Array([0xff]));
  assertEquals(out, "�");
});

test("TextDecoder fatal throws on invalid input", () => {
  assertThrows(() => new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array([0xff])), "TypeError");
});

test("TextDecoder decodes empty input to empty string", () => {
  assertEquals(new TextDecoder().decode(new Uint8Array(0)), "");
  assertEquals(new TextDecoder().decode(), "");
});
