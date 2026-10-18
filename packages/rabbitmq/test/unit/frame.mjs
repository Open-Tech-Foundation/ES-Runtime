// Frame boundaries, with no socket anywhere near it.
//
// The cases worth pinning are the ones a socket hands over unpredictably: a
// frame split across chunks, several frames in one read, and the frame-end
// marker that says where a frame stops.
import { exit } from "runtime:process";
import {
  DEFAULT_FRAME_MAX,
  encodeFrame,
  encodeProtocolHeader,
  FRAME_END,
  FrameDecoder,
  FrameType,
  isFrameType,
} from "../../dist/protocol/frame.js";
import { ok, report } from "./assert.mjs";

function same(a, expected) {
  return a.length === expected.length && a.every((byte, i) => byte === expected[i]);
}

function encoder(bytes) {
  return new TextEncoder().encode(bytes);
}

// -- the protocol header ----------------------------------------------------

ok(
  same(encodeProtocolHeader(), [0x41, 0x4d, 0x51, 0x50, 0x00, 0x00, 0x09, 0x01]),
  "the protocol header spells AMQP 0-9-1",
);

// -- the frame layout -------------------------------------------------------

{
  const framed = encodeFrame({
    type: FrameType.body,
    channel: 1,
    payload: new Uint8Array([0xde, 0xad]),
  });
  ok(
    same(framed, [0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02, 0xde, 0xad, 0xce]),
    "a frame is type, channel, size, payload, frame-end",
  );
}

for (const type of [FrameType.method, FrameType.header, FrameType.body, FrameType.heartbeat]) {
  ok(isFrameType(type), `${type} is a frame type`);
}
ok(!isFrameType(4), "4 is not a frame type");

// -- decoding, one chunk at a time ------------------------------------------

{
  const decoder = new FrameDecoder();
  const framed = encodeFrame({
    type: FrameType.method,
    channel: 7,
    payload: encoder("\x00\x0a\x00\x0b"),
  });
  for (let i = 0; i < framed.length - 1; i++) {
    decoder.feed(framed.subarray(i, i + 1));
    ok(decoder.next() === null, `byte ${i} alone is not yet a frame`);
  }
  decoder.feed(framed.subarray(framed.length - 1));
  const frame = decoder.next();
  ok(frame !== null && frame.channel === 7, "the last byte completes the frame");
  ok(frame !== null && same(frame.payload, [0x00, 0x0a, 0x00, 0x0b]), "the payload is intact");
  ok(decoder.next() === null, "a consumed frame is not returned twice");
  ok(decoder.buffered === 0, "nothing is left buffered");
}

// Two frames arriving together are two frames, not one.
{
  const decoder = new FrameDecoder();
  decoder.feed(
    new Uint8Array([
      ...encodeFrame({ type: FrameType.body, channel: 1, payload: new Uint8Array([1]) }),
      ...encodeFrame({ type: FrameType.body, channel: 2, payload: new Uint8Array([2, 3]) }),
    ]),
  );
  const first = decoder.next();
  const second = decoder.next();
  ok(
    first !== null && first.channel === 1 && same(first.payload, [1]),
    "the first frame comes out first",
  );
  ok(
    second !== null && second.channel === 2 && same(second.payload, [2, 3]),
    "the second follows it",
  );
  ok(decoder.next() === null, "and then there is nothing");
}

// A frame's payload must outlive the buffer it was read from: the decoder
// compacts, and a view into it would be rewritten by the next frame.
{
  const decoder = new FrameDecoder();
  decoder.feed(encodeFrame({ type: FrameType.body, channel: 1, payload: encoder("keep me") }));
  const frame = decoder.next();
  decoder.feed(encodeFrame({ type: FrameType.body, channel: 1, payload: encoder("second!!") }));
  decoder.next();
  ok(
    frame !== null && new TextDecoder().decode(frame.payload) === "keep me",
    "a payload is a copy",
  );
}

// -- the refusals -----------------------------------------------------------

function refuses(what, fn) {
  let threw = false;
  try {
    fn();
  } catch {
    threw = true;
  }
  ok(threw, what);
}

{
  const decoder = new FrameDecoder({ frameMax: 8 });
  decoder.feed(encodeFrame({ type: FrameType.body, channel: 1, payload: new Uint8Array(16) }));
  refuses("a payload past frame-max is refused", () => decoder.next());
}

{
  const decoder = new FrameDecoder();
  decoder.feed(new Uint8Array([0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x02, 0xde, 0xad, 0x00]));
  refuses("a frame without the frame-end marker is refused", () => decoder.next());
}

{
  const decoder = new FrameDecoder();
  decoder.feed(new Uint8Array([0x05, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0xce]));
  refuses("an unknown frame type is refused", () => decoder.next());
}

refuses("encoding an unknown frame type is refused", () =>
  encodeFrame({ type: 4, channel: 0, payload: new Uint8Array(0) }),
);

refuses("encoding a channel past 65535 is refused", () =>
  encodeFrame({ type: FrameType.body, channel: 65536, payload: new Uint8Array(0) }),
);

// A heartbeat is the connection's pulse: it belongs to no channel and says
// nothing. One carrying a payload, or on a channel, is a peer that has lost its
// place.
refuses("a heartbeat on a channel is refused", () => {
  const decoder = new FrameDecoder();
  decoder.feed(encodeFrame({ type: FrameType.heartbeat, channel: 1, payload: new Uint8Array(0) }));
  decoder.next();
});
refuses("a heartbeat with a payload is refused", () => {
  const decoder = new FrameDecoder();
  decoder.feed(encodeFrame({ type: FrameType.heartbeat, channel: 0, payload: new Uint8Array(2) }));
  decoder.next();
});
{
  const decoder = new FrameDecoder();
  decoder.feed(encodeFrame({ type: FrameType.heartbeat, channel: 0, payload: new Uint8Array(0) }));
  const frame = decoder.next();
  ok(
    frame !== null && frame.type === FrameType.heartbeat && frame.channel === 0,
    "a heartbeat round-trips",
  );
}

ok(DEFAULT_FRAME_MAX === 131072, "the default frame-max is RabbitMQ's");
ok(FRAME_END === 0xce, "the frame-end marker is 0xCE");

if (report("frame") > 0) exit(1);
