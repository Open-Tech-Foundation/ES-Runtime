// The frame reader, against the thing that actually happens on a socket: the
// bytes arrive in chunks that have nothing to do with message boundaries.

import { exit } from "runtime:process";
import { FrameBatch, FrameReader } from "../../dist/protocol/frame.js";
import { is, ok, report } from "./assert.mjs";

function streamOf(chunks) {
  return new ReadableStream({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(chunk);
      controller.close();
    },
  });
}

/** A message: one tag byte, a self-inclusive length, then the body. */
function message(tag, body) {
  const out = new Uint8Array(5 + body.length);
  out[0] = tag;
  new DataView(out.buffer).setInt32(1, 4 + body.length);
  out.set(body, 5);
  return out;
}

const a = message(0x41, new Uint8Array([1, 2, 3]));
const b = message(0x42, new Uint8Array(200).fill(7));

// Whole messages, one chunk each.
{
  const r = new FrameReader(streamOf([a, b]));
  const first = await r.message();
  is(first.tag, 0x41, "first tag");
  is(first.frame.length, 7, "first frame carries its length prefix");
  is(await r.message().then((m) => m.tag), 0x42, "second tag");
}

// One byte at a time: every message spans chunk boundaries, which is the case a
// reader that assumed a chunk was a message would get wrong.
{
  const bytes = new Uint8Array(a.length + b.length);
  bytes.set(a);
  bytes.set(b, a.length);
  const r = new FrameReader(streamOf([...bytes].map((byte) => new Uint8Array([byte]))));
  is((await r.message()).tag, 0x41, "byte-at-a-time first tag");
  const second = await r.message();
  is(second.tag, 0x42, "byte-at-a-time second tag");
  is(second.frame.length, 204, "byte-at-a-time second length");
}

// Two messages in one chunk, then a message split across three.
{
  const both = new Uint8Array(a.length * 2);
  both.set(a);
  both.set(a, a.length);
  const r = new FrameReader(streamOf([both, b.subarray(0, 2), b.subarray(2, 9), b.subarray(9)]));
  is((await r.message()).tag, 0x41, "coalesced first");
  is((await r.message()).tag, 0x41, "coalesced second");
  is((await r.message()).frame.length, 204, "split message reassembled");
}

// A message larger than the initial buffer forces it to grow.
{
  const big = message(0x43, new Uint8Array(200_000).fill(9));
  const r = new FrameReader(streamOf([big]), 64);
  const m = await r.message();
  is(m.tag, 0x43, "oversized tag");
  is(m.frame.length, 200_004, "the buffer grew to hold it");
}

// A stream that ends mid-message is an error, not a short read presented as a
// message.
{
  const r = new FrameReader(streamOf([b.subarray(0, 10)]));
  let threw = false;
  try {
    await r.message();
  } catch {
    threw = true;
  }
  ok(threw, "a truncated message is an error");
}

// The single byte the SSLRequest answer arrives as, before framing exists.
{
  const r = new FrameReader(streamOf([new Uint8Array([0x53])]));
  is(await r.byte(), 0x53, "a raw byte is readable before any framing");
}

// `poll` answers from the buffer or not at all: null until a whole message has
// arrived, and never a promise.
{
  const r = new FrameReader(streamOf([a.subarray(0, 3), a.subarray(3)]));
  is(r.poll(), null, "nothing buffered, nothing polled");
  await r.message().catch(() => {});
  is(r.poll(), null, "the only message was taken by message()");
}

// `take` moves the run of matching messages that has arrived whole, and stops
// at the first that does not match or has not arrived — leaving it for the
// ordinary path.
{
  const row = (n) => message(0x44, new Uint8Array([0, 1, 0, 0, 0, 1, n]));
  const run = [row(1), row(2), row(3), message(0x43, new Uint8Array([0])), row(4)];
  const whole = new Uint8Array(run.reduce((n, m) => n + m.length, 0));
  let at = 0;
  for (const m of run) {
    whole.set(m, at);
    at += m.length;
  }
  // The fourth row is split, so only the three before the other tag are whole.
  const r = new FrameReader(
    streamOf([whole.subarray(0, whole.length - 2), whole.subarray(whole.length - 2)]),
  );
  is((await r.message()).tag, 0x44, "the first row arrives the ordinary way");
  const batch = new FrameBatch(0);
  r.take(0x44, batch, 1 << 16);
  is(batch.count, 2, "the rest of the run is taken");
  is(batch.size, 2 * 11, "each frame is copied from its length prefix on");
  is(`${batch.gathered[10]},${batch.gathered[21]}`, "2,3", "and in order");
  is(r.poll()?.tag, 0x43, "the message that ended the run is left in place");
  r.take(0x44, batch, 1 << 16);
  is(batch.count, 2, "a split message is not taken");
  is((await r.message()).tag, 0x44, "it arrives the ordinary way once whole");
}

// `take` stops once the batch holds the limit, and a batch grows past its
// first guess rather than refusing.
{
  const row = message(0x44, new Uint8Array(300).fill(1));
  const r = new FrameReader(streamOf([row, row, row]));
  await r.message();
  const batch = new FrameBatch(0);
  // Nothing more is buffered yet: the stream's chunks arrive one read at a time.
  r.take(0x44, batch, 1);
  const again = new FrameReader(streamOf([new Uint8Array([...row, ...row, ...row])]));
  is((await again.message()).tag, 0x44, "prime the buffer");
  again.take(0x44, batch, 1);
  is(batch.count, 1, "the limit stops the run after the frame that crossed it");
  again.take(0x44, batch, 1 << 16);
  is(batch.size, 2 * 304, "the batch grew to hold both");
}

if (report("frame") > 0) exit(1);
