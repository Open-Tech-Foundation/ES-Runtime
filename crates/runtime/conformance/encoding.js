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

// ---- Encodings beyond UTF-8 -----------------------------------------------

test("labels resolve through the spec's table", () => {
  // The `encoding` attribute reports the canonical name, not the label given.
  assertEquals(new TextDecoder("UTF8").encoding, "utf-8");
  assertEquals(new TextDecoder("  unicode-1-1-utf-8 ").encoding, "utf-8");
  assertEquals(new TextDecoder("latin1").encoding, "windows-1252");
  assertEquals(new TextDecoder("ISO-8859-1").encoding, "windows-1252");
  assertEquals(new TextDecoder("utf-16").encoding, "utf-16le");
  assertEquals(new TextDecoder("sjis").encoding, "shift_jis");
  assertEquals(new TextDecoder("gb2312").encoding, "gbk");
  assertThrows(() => new TextDecoder("definitely-not-an-encoding"), "RangeError");
});

test("the non-UTF-8 encodings decode", () => {
  assertEquals(new TextDecoder("utf-16le").decode(new Uint8Array([0x61, 0, 0xe9, 0])), "aé");
  assertEquals(new TextDecoder("utf-16be").decode(new Uint8Array([0, 0x61, 0, 0xe9])), "aé");
  // 0x80 is the euro sign in windows-1252 — the byte that distinguishes it from
  // ISO-8859-1, which the spec maps to this same encoding.
  assertEquals(new TextDecoder("windows-1252").decode(new Uint8Array([0x61, 0xe9, 0x80])), "aé€");
  assertEquals(new TextDecoder("shift_jis").decode(new Uint8Array([0x82, 0xa0])), "あ");
  assertEquals(new TextDecoder("gb18030").decode(new Uint8Array([0xd6, 0xd0])), "中");
});

test("a character split across chunks survives the boundary", () => {
  const utf16 = new TextDecoder("utf-16le");
  assertEquals(utf16.decode(new Uint8Array([0x61, 0, 0xe9]), { stream: true }), "a");
  assertEquals(utf16.decode(new Uint8Array([0]), { stream: true }), "é");
  assertEquals(utf16.decode(), "");

  // The same for a 4-byte UTF-8 sequence cut in half.
  const utf8 = new TextDecoder();
  const emoji = new TextEncoder().encode("😀");
  assertEquals(utf8.decode(emoji.subarray(0, 2), { stream: true }), "");
  assertEquals(utf8.decode(emoji.subarray(2), { stream: true }), "😀");
  assertEquals(utf8.decode(), "");
});

test("ending a stream mid-character is an error, not a wait", () => {
  // Streaming holds a partial sequence back; ending the stream flushes it.
  const lossy = new TextDecoder("utf-16le");
  assertEquals(lossy.decode(new Uint8Array([0x61, 0, 0xe9]), { stream: false }), "a�");
  const strict = new TextDecoder("utf-16le", { fatal: true });
  assertThrows(() => strict.decode(new Uint8Array([0x61, 0, 0xe9])), "TypeError");
});

test("a BOM is stripped for its own encoding only", () => {
  assertEquals(new TextDecoder("utf-16le").decode(new Uint8Array([0xff, 0xfe, 0x61, 0])), "a");
  assertEquals(
    new TextDecoder("utf-16le", { ignoreBOM: true }).decode(new Uint8Array([0xff, 0xfe, 0x61, 0])),
    "﻿a",
  );
  // A UTF-16 BOM handed to a windows-1252 decoder is data: the decoder must not
  // morph into a decoder for another encoding.
  assertEquals(new TextDecoder("windows-1252").decode(new Uint8Array([0xff, 0xfe, 0x61])), "ÿþa");
});

test("TextDecoderStream carries the label through", async () => {
  const stream = new TextDecoderStream("utf-16le");
  assertEquals(stream.encoding, "utf-16le");
  const readable = new ReadableStream({
    start(c) {
      // Split mid-character, so the transform has to hold state too.
      c.enqueue(new Uint8Array([0x61, 0, 0xe9]));
      c.enqueue(new Uint8Array([0]));
      c.close();
    },
  });
  let out = "";
  for await (const chunk of readable.pipeThrough(stream)) out += chunk;
  assertEquals(out, "aé");
});
