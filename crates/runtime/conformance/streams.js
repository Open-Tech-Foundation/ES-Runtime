// WinterTC §2.8 — ReadableStream / WritableStream / TransformStream.

test("ReadableStream yields enqueued chunks then closes", async () => {
  const rs = new ReadableStream({
    start(c) { c.enqueue(1); c.enqueue(2); c.close(); },
  });
  const reader = rs.getReader();
  assertEquals((await reader.read()).value, 1);
  assertEquals((await reader.read()).value, 2);
  assertEquals((await reader.read()).done, true);
});

test("WritableStream receives written chunks", async () => {
  const seen = [];
  const ws = new WritableStream({ write(chunk) { seen.push(chunk); } });
  const w = ws.getWriter();
  await w.write("a");
  await w.write("b");
  await w.close();
  assertEquals(seen.join(""), "ab");
});

test("TransformStream maps chunks", async () => {
  const ts = new TransformStream({
    transform(chunk, c) { c.enqueue(chunk * 10); },
  });
  const reader = ts.readable.getReader();
  const w = ts.writable.getWriter();
  await w.write(3);
  await w.close();
  assertEquals((await reader.read()).value, 30);
});

test("ReadableStream.tee produces two independent branches", async () => {
  const rs = new ReadableStream({ start(c) { c.enqueue("x"); c.close(); } });
  const [a, b] = rs.tee();
  assertEquals((await a.getReader().read()).value, "x");
  assertEquals((await b.getReader().read()).value, "x");
});

test("pipeTo moves data into a writable sink", async () => {
  const out = [];
  const rs = new ReadableStream({ start(c) { c.enqueue(1); c.enqueue(2); c.close(); } });
  const ws = new WritableStream({ write(chunk) { out.push(chunk); } });
  await rs.pipeTo(ws);
  assertEquals(out.join(","), "1,2");
});

test("byte stream default reader yields Uint8Array chunks", async () => {
  const rs = new ReadableStream({
    type: "bytes",
    start(c) { c.enqueue(new Uint8Array([1, 2, 3])); c.close(); },
  });
  const reader = rs.getReader();
  const r1 = await reader.read();
  assert(r1.value instanceof Uint8Array);
  assertEquals(r1.value.length, 3);
  assertEquals(r1.value[0], 1);
  assertEquals((await reader.read()).done, true);
});

test("BYOB reader fills a caller-supplied view", async () => {
  const rs = new ReadableStream({
    type: "bytes",
    start(c) { c.enqueue(new Uint8Array([10, 20, 30, 40])); c.close(); },
  });
  const reader = rs.getReader({ mode: "byob" });
  const res = await reader.read(new Uint8Array(4));
  assertEquals(res.value.length, 4);
  assertEquals(res.value[3], 40);
});

test("BYOB reads drain a chunk across two views", async () => {
  const rs = new ReadableStream({
    type: "bytes",
    start(c) { c.enqueue(new Uint8Array([1, 2, 3, 4, 5])); c.close(); },
  });
  const reader = rs.getReader({ mode: "byob" });
  const a = await reader.read(new Uint8Array(2));
  assertEquals([...a.value].join(","), "1,2");
  const b = await reader.read(new Uint8Array(2));
  assertEquals([...b.value].join(","), "3,4");
});

test("byte stream pull + autoAllocate + byobRequest.respond", async () => {
  const rs = new ReadableStream({
    type: "bytes",
    autoAllocateChunkSize: 8,
    pull(c) {
      c.byobRequest.view[0] = 99;
      c.byobRequest.respond(1);
      c.close();
    },
  });
  const r = await rs.getReader().read();
  assertEquals(r.value.length, 1);
  assertEquals(r.value[0], 99);
});

test("byte-stream globals are exposed", () => {
  assertEquals(typeof ReadableByteStreamController, "function");
  assertEquals(typeof ReadableStreamBYOBReader, "function");
  assertEquals(typeof ReadableStreamBYOBRequest, "function");
});

test("TextEncoderStream / TextDecoderStream round-trip via pipeThrough", async () => {
  const rs = new ReadableStream({ start(c) { c.enqueue("hé"); c.enqueue("llo"); c.close(); } });
  const decoded = rs.pipeThrough(new TextEncoderStream()).pipeThrough(new TextDecoderStream());
  const reader = decoded.getReader();
  let s = "";
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    s += value;
  }
  assertEquals(s, "héllo");
});
