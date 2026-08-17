import { assertEquals, test } from "runtime:test";

import { withoutBody } from "./method.ts";

test("a HEAD response keeps the status and every header", async () => {
  const head = withoutBody(
    new Response("hello", {
      status: 206,
      headers: { "content-type": "text/plain", "content-length": "5", etag: '"abc"' },
    }),
  );

  assertEquals(head.status, 206);
  assertEquals(head.headers.get("content-type"), "text/plain");
  // The point of a HEAD: the size, without the bytes.
  assertEquals(head.headers.get("content-length"), "5");
  assertEquals(head.headers.get("etag"), '"abc"');
  assertEquals(head.body, null);
  assertEquals(await head.text(), "");
});

test("the body it dropped is cancelled, not left open", async () => {
  let cancelled = false;
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(new TextEncoder().encode("chunk"));
    },
    cancel() {
      cancelled = true;
    },
  });

  withoutBody(new Response(body, { status: 200 }));
  // The cancel is not awaited by `withoutBody` — a response is being returned,
  // not a stream drained — so give the microtask queue a turn.
  await Promise.resolve();

  assertEquals(cancelled, true);
});

test("a response that never had a body is returned as it is", () => {
  const original = new Response(null, { status: 304, headers: { etag: '"abc"' } });
  const head = withoutBody(original);

  assertEquals(head, original);
  assertEquals(head.status, 304);
});
