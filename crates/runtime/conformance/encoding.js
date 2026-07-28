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

test("TextDecoder decodes DataView and ArrayBuffer", () => {
  const enc = new TextEncoder();
  const buf = enc.encode("test").buffer;
  assertEquals(new TextDecoder().decode(buf), "test");
  assertEquals(new TextDecoder().decode(new DataView(buf)), "test");
});

test("TextEncoder encodeInto writes into Uint8Array", () => {
  const enc = new TextEncoder();
  const dest = new Uint8Array(10);
  const res = enc.encodeInto("hello", dest);
  assertEquals(res.read, 5);
  assertEquals(res.written, 5);
  assertEquals(new TextDecoder().decode(dest.subarray(0, 5)), "hello");
});


test("decode({ stream: true }) buffers a split multi-byte sequence", () => {
  const d = new TextDecoder();
  const head = d.decode(new Uint8Array([0xe2, 0x82]), { stream: true });
  const tail = d.decode(new Uint8Array([0xac]), { stream: true });
  assertEquals(head + tail, "€");
});

test("a streaming decoder flushes a dangling sequence on the final decode", () => {
  const d = new TextDecoder();
  d.decode(new Uint8Array([0xe2, 0x82]), { stream: true });
  assertEquals(d.decode(new Uint8Array([])), "�");
});

test("a four-byte code point split three ways decodes once", () => {
  const d = new TextDecoder();
  let out = "";
  for (const b of [0xf0, 0x9f, 0x98, 0x80]) {
    out += d.decode(new Uint8Array([b]), { stream: true });
  }
  out += d.decode();
  assertEquals(out, "\u{1F600}");
});

test("an invalid lead byte is not held back as a partial sequence", () => {
  const d = new TextDecoder();
  assertEquals(d.decode(new Uint8Array([0xff]), { stream: true }), "\ufffd");
});

test("the BOM is stripped once per stream, not once per chunk", () => {
  const d = new TextDecoder();
  let out = d.decode(new Uint8Array([0xef, 0xbb, 0xbf, 0x61]), { stream: true });
  // A second chunk that happens to start with a BOM keeps it: the BOM belongs
  // to the start of the stream only.
  out += d.decode(new Uint8Array([0xef, 0xbb, 0xbf, 0x62]), { stream: true });
  out += d.decode();
  assertEquals(out, "a\ufeffb");
});

test("a non-streaming decode ends the stream and resets the decoder", () => {
  const d = new TextDecoder();
  d.decode(new Uint8Array([0xe2, 0x82]), { stream: true });
  // Ends the stream: the dangling bytes flush as a replacement character.
  assertEquals(d.decode(), "\ufffd");
  // The next decode starts fresh, so its BOM is stripped again.
  assertEquals(d.decode(new Uint8Array([0xef, 0xbb, 0xbf, 0x61])), "a");
});

test("a fatal decoder still buffers a split sequence rather than erroring", () => {
  const d = new TextDecoder("utf-8", { fatal: true });
  assertEquals(d.decode(new Uint8Array([0xe2, 0x82]), { stream: true }), "");
  assertEquals(d.decode(new Uint8Array([0xac]), { stream: true }), "\u20ac");
});
