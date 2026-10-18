/**
 * The variable byte integer: MQTT's one length form.
 *
 * Remaining Length, the property length, and every property whose value is a
 * count are a base-128 number, least-significant group first, with the top bit
 * of each octet marking "more follows". It runs from one octet to four, so the
 * largest value is 268435455 — the ceiling on how big a single MQTT packet may
 * be. A fifth octet is not a larger number, it is a malformed packet, and the
 * decoder says so rather than wrapping.
 *
 * The canonical encoding is minimal, but the decoder does not insist on it: a
 * peer that sends a redundant leading octet is unusual, not broken, and the
 * value is the same. What it does refuse is running off the end of the buffer,
 * which is a truncated packet and nothing a caller can recover from.
 */

import { ProtocolError } from "./errors.js";

/** The largest value a variable byte integer can hold (2^28 − 1). */
export const MAX_VARINT = 268435455;

/** How many octets `value` takes, or a `ProtocolError` if it cannot be encoded. */
export function varintLength(value: number): number {
  if (!Number.isInteger(value) || value < 0 || value > MAX_VARINT) {
    throw new ProtocolError(`a variable byte integer is 0 to ${MAX_VARINT}; got ${value}`);
  }
  if (value < 128) return 1;
  if (value < 16384) return 2;
  if (value < 2097152) return 3;
  return 4;
}

/** Encodes `value` as a variable byte integer. */
export function encodeVarint(value: number): Uint8Array {
  const length = varintLength(value);
  const out = new Uint8Array(length);
  let v = value;
  for (let i = 0; i < length; i++) {
    let octet = v % 128;
    v = Math.floor(v / 128);
    if (v > 0) octet |= 0x80;
    out[i] = octet;
  }
  return out;
}

/**
 * Decodes the variable byte integer starting at `offset`.
 *
 * Returns the value and how many octets it took, so a caller reading a stream
 * knows where the field after it begins.
 */
export function decodeVarint(bytes: Uint8Array, offset = 0): { value: number; length: number } {
  let value = 0;
  let multiplier = 1;
  let at = offset;
  for (let i = 0; i < 4; i++) {
    if (at >= bytes.length) {
      throw new ProtocolError("a variable byte integer ran off the end of the buffer");
    }
    const octet = bytes[at]!;
    at += 1;
    value += (octet & 0x7f) * multiplier;
    if ((octet & 0x80) === 0) return { value, length: at - offset };
    multiplier *= 128;
  }
  throw new ProtocolError("a variable byte integer is at most four octets");
}
