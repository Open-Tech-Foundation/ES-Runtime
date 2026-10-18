// A broker you can hold in your hand: two byte pipes with a client on one end
// and the test on the other.
//
// This is what the `Transport` seam is for. The connection's handshake, packet
// pump, acknowledgements and keep-alive are all exercised against this — no
// port open, no capability granted (D65) — because the bytes that reach the
// connection are the same whether a socket or this delivered them. The test
// drives the other side: it reads the packets the client sent, and writes back
// the packets a broker would.
import { decodePacket, encodePacket } from "../../dist/protocol/packets.js";

function concat(a, b) {
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}

function makeChannel() {
  const queue = [];
  let waiter = null;
  let closed = false;
  return {
    push(bytes) {
      if (closed) return;
      if (waiter !== null) {
        const resolve = waiter;
        waiter = null;
        resolve(bytes);
      } else {
        queue.push(bytes);
      }
    },
    end() {
      closed = true;
      if (waiter !== null) {
        const resolve = waiter;
        waiter = null;
        resolve(null);
      }
    },
    read() {
      if (queue.length > 0) return Promise.resolve(queue.shift());
      if (closed) return Promise.resolve(null);
      return new Promise((resolve) => {
        waiter = resolve;
      });
    },
  };
}

/** One slice of a packet off the front of the buffer, or `null` if incomplete. */
function tryFrame(bytes) {
  if (bytes.length < 2) return null;
  let remaining = 0;
  let multiplier = 1;
  for (let i = 1; i <= 4; i++) {
    if (i >= bytes.length) return null;
    const octet = bytes[i];
    remaining += (octet & 0x7f) * multiplier;
    if ((octet & 0x80) === 0) {
      const headerLength = 1 + i;
      const total = headerLength + remaining;
      if (bytes.length < total) return null;
      return { total, packet: bytes.subarray(0, total) };
    }
    multiplier *= 128;
  }
  throw new Error("a Remaining Length of more than four octets");
}

/**
 * A broker the connection can talk to, over a `Transport`, with the test
 * holding the other side.
 */
export function fakeBroker(level = 5) {
  const toBroker = makeChannel();
  const toClient = makeChannel();
  let buffer = new Uint8Array(0);

  const transport = {
    read: () => toClient.read(),
    write: async (bytes) => {
      toBroker.push(bytes);
    },
    close: async () => {
      toBroker.end();
      toClient.end();
    },
  };

  return {
    transport,
    level,
    /** The next packet the client sent, or `null` if it closed the pipe. */
    async next(timeoutMs = 2000) {
      const deadline = Date.now() + timeoutMs;
      for (;;) {
        const frame = tryFrame(buffer);
        if (frame !== null) {
          buffer = buffer.subarray(frame.total);
          return decodePacket(frame.packet, level);
        }
        if (Date.now() > deadline) throw new Error("timed out waiting for a client packet");
        const chunk = await toBroker.read();
        if (chunk === null) return null;
        buffer = concat(buffer, chunk);
      }
    },
    /** Sends a packet to the client. */
    send(packet) {
      toClient.push(encodePacket(packet, level));
    },
    /** Sends raw bytes, for the malformed cases. */
    sendRaw(bytes) {
      toClient.push(bytes);
    },
    /** Ends the pipe, as a broker vanishing would. */
    close() {
      toBroker.end();
      toClient.end();
    },
  };
}
