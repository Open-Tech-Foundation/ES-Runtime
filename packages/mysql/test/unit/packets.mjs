// Framing, against what a socket actually does: chunks unrelated to packets.
import { exit } from "runtime:process";
import { MAX_PAYLOAD, PacketReader, Payload, Writer } from "../../dist/protocol/packets.js";
import { is, ok, report, throws } from "./assert.mjs";

function streamOf(chunks) {
  return new ReadableStream({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(chunk);
      controller.close();
    },
  });
}

const packet = (seq, bytes) => new Writer().bytes(new Uint8Array(bytes)).finish(seq);
const join = (...parts) => {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
};

// Header: three bytes of length, little-endian, then the sequence id.
is([...packet(7, [1, 2, 3])], [3, 0, 0, 7, 1, 2, 3], "a packet's header");

// One byte at a time, and the sequence id is tracked.
{
  const bytes = join(packet(0, [0xa, 0xb]), packet(1, [0xc]));
  const r = new PacketReader(streamOf([...bytes].map((b) => new Uint8Array([b]))));
  is([...(await r.packet())], [0xa, 0xb], "byte-at-a-time first");
  is(r.seq, 0, "its sequence id");
  is([...(await r.packet())], [0xc], "byte-at-a-time second");
  is(r.seq, 1, "the next sequence id");
  await throws(() => r.packet(), "reading past the end is an error");
}

// `take` hands over the run of packets with the lead byte, and stops at the rest.
{
  const r = new PacketReader(
    streamOf([join(packet(1, [0, 1]), packet(2, [0, 2]), packet(3, [0xfe, 9]), packet(4, [0, 3]))]),
  );
  is([...(await r.packet())], [0, 1], "prime the buffer");
  const seen = [];
  r.take(0x00, (bytes, _view, start) => {
    seen.push(bytes[start + 1]);
    return true;
  });
  is(seen, [2], "the run up to the terminator");
  is([...r.poll()], [0xfe, 9], "the terminator is left for the ordinary path");
  const stop = [];
  r.take(0x00, (bytes, _v, start) => {
    stop.push(bytes[start + 1]);
    return false;
  });
  is(stop, [3], "returning false stops after that row");
}

// Past 16 MiB: split on the way out, joined on the way in, with the empty
// packet that ends an exact multiple.
{
  const size = MAX_PAYLOAD + 10;
  const payload = new Uint8Array(size);
  payload[0] = 1;
  payload[size - 1] = 2;
  const framed = new Writer().bytes(payload).finish(3);
  is(framed.length, size + 8, "two headers");
  is([framed[0], framed[1], framed[2], framed[3]], [0xff, 0xff, 0xff, 3], "the first part is full");
  is(framed[MAX_PAYLOAD + 7], 4, "the second part's sequence id follows");
  const r = new PacketReader(streamOf([framed.subarray(0, 1000), framed.subarray(1000)]), 64);
  const back = await r.packet();
  ok(back.length === size && back[0] === 1 && back[size - 1] === 2, "joined back into one payload");
  is(r.seq, 4, "the reader's sequence id is the last part's");

  const exact = new Writer().bytes(new Uint8Array(MAX_PAYLOAD)).finish(0);
  is(exact.length, MAX_PAYLOAD + 8, "an exact multiple ends with an empty packet");
  const r2 = new PacketReader(streamOf([exact]));
  is((await r2.packet()).length, MAX_PAYLOAD, "and reads back whole");
}

// Length-encoded integers, every width, and skipping strings by them.
{
  const w = new Writer()
    .lenenc(250)
    .lenenc(251)
    .lenenc(0xffff)
    .lenenc(0x10000)
    .lenenc(0x1000000)
    .lenencString("héllo")
    .lenencString("x");
  const p = new Payload(w.finish(0).subarray(4));
  is(
    [p.lenenc(), p.lenenc(), p.lenenc(), p.lenenc(), p.lenenc()],
    [250, 251, 0xffff, 0x10000, 0x1000000],
    "lenenc round trip",
  );
  p.skipLenenc();
  is(p.lenencString(), "x", "skipLenenc moves past the length and the string");
  is(new Payload(new Uint8Array([0xfb])).lenenc(), null, "0xFB is NULL");
}

if (report("packets") > 0) exit(1);
