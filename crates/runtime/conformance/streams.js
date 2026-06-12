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
