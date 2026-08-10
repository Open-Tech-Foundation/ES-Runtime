// Shared workload for the Redis client comparison.
//
// Redis is not a database benchmark in the way PostgreSQL is: almost nothing
// here is the server's time. A command's cost is a round trip and a decode, so
// what these measure is the client — how many round trips it makes, and what it
// spends turning bytes into JavaScript.
//
// Hence the four shapes:
//
//   serial_set / serial_get  one command at a time. Round-trip bound; this is
//                            the floor every client shares, and a client much
//                            slower here is spending time on the boundary.
//   pipeline                 the same work batched. The gap between this and
//                            serial is the whole argument for pipelining, and
//                            each client uses its own idiom for it.
//   list                     one huge reply. Decode bound — the only workload
//                            where the reply is big enough for parsing to
//                            dominate.
//   hash                     many medium map replies. Decode bound too, but
//                            over the shape RESP3 types (a map) and RESP2 does
//                            not, so it is where the protocols can differ.
export const SERIAL = 5_000;
export const PIPELINE = 20_000;
export const LIST_LEN = 50_000;
export const HASH_FIELDS = 1_000;
export const HASH_REPEATS = 200;

export const PREFIX = "bench:redis:";
export const LIST_KEY = `${PREFIX}list`;
export const HASH_KEY = `${PREFIX}hash`;
export const VALUE = "v".repeat(32);

export function keyOf(i) {
  return `${PREFIX}k${i}`;
}

export function fieldValue(i) {
  return `field-value-${i}`;
}

/** The answers, so a runtime cannot look fast by doing less. */
export function expectedSerialGet() {
  return String(SERIAL * VALUE.length);
}

export function expectedPipeline() {
  return String(PIPELINE);
}

export function expectedList() {
  // Every element is `item-<i>`; the checksum is their total length.
  let n = 0;
  for (let i = 0; i < LIST_LEN; i++) n += `item-${i}`.length;
  return String(n);
}

export function expectedHash() {
  let per = 0;
  for (let i = 0; i < HASH_FIELDS; i++) per += fieldValue(i).length;
  return String(per * HASH_REPEATS);
}
