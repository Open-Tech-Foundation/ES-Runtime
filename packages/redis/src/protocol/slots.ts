/**
 * Hash slots: which of a cluster's 16384 slots a key belongs to.
 *
 * `CRC16(key) mod 16384`, where CRC16 is the XMODEM variant — polynomial
 * 0x1021, no initial value, no final XOR. Redis picked it and every client has
 * to agree byte for byte, so this is checked against published vectors rather
 * than against itself.
 *
 * The **hash tag** is the part that makes multi-key commands possible at all.
 * If a key contains `{…}` with something between the braces, only that part is
 * hashed — so `{user:1}:name` and `{user:1}:email` land on the same node and a
 * command may touch both. Getting the tag rules subtly wrong is the classic way
 * a client works everywhere except where it matters.
 */

const encoder = new TextEncoder();

/** The number of slots a Redis cluster has. Fixed by the protocol. */
export const SLOTS = 16384;

// The XMODEM table, generated once from the polynomial rather than pasted as
// 256 magic numbers — the polynomial is the thing worth being able to read.
const TABLE = (() => {
  const table = new Uint16Array(256);
  for (let i = 0; i < 256; i++) {
    let crc = i << 8;
    for (let bit = 0; bit < 8; bit++) {
      crc = (crc & 0x8000) !== 0 ? ((crc << 1) ^ 0x1021) & 0xffff : (crc << 1) & 0xffff;
    }
    table[i] = crc;
  }
  return table;
})();

/** CRC16/XMODEM over bytes. */
export function crc16(bytes: Uint8Array): number {
  let crc = 0;
  for (const byte of bytes) {
    crc = ((crc << 8) ^ TABLE[((crc >> 8) ^ byte) & 0xff]!) & 0xffff;
  }
  return crc;
}

/**
 * The part of a key that is actually hashed.
 *
 * The rule, exactly: find the first `{`; find the first `}` **after** it; if
 * both exist and there is at least one character between them, hash that.
 * Otherwise hash the whole key.
 *
 * The two edge cases that catch people out follow from "first" being meant
 * literally. `foo{}{bar}` hashes as the whole key, because the first `}` comes
 * straight after the first `{` and an empty tag does not count — it does *not*
 * go looking for a later, non-empty pair. And `foo{{bar}}` hashes `{bar`,
 * because the first `}` closes the first `{` whatever is in between.
 */
export function hashTag(key: string): string {
  const open = key.indexOf("{");
  if (open === -1) return key;
  const close = key.indexOf("}", open + 1);
  if (close === -1 || close === open + 1) return key;
  return key.slice(open + 1, close);
}

/** The slot a key belongs to. */
export function hashSlot(key: string): number {
  return crc16(encoder.encode(hashTag(key))) % SLOTS;
}
