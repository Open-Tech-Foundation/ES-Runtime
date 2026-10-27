// A type test for `runtime:serialization`'s BSON (D187). Compiled by `tsc -p .`, never run.

import { BSON } from "runtime:serialization";

const id = new BSON.ObjectId();
const bytes: Uint8Array = BSON.encode({
  _id: id,
  n: 1n,
  at: new Date(),
  tags: new Map([["a", 1]]),
});
const doc: Record<string, any> = BSON.decode(bytes);
const valid: boolean | { valid: boolean } = BSON.validate(bytes, { detailed: true });
const when: Date = id.getTimestamp();
const hex: string = new BSON.Decimal128("1.50").toString();
const vector: Float32Array = BSON.Binary.fromFloat32Array([0.5, 1]).toFloat32Array();
const t: number = new BSON.Timestamp({ t: 1, i: 2 }).t;
const re: RegExp = new BSON.BSONRegExp("^a", "i").toRegExp();
// @ts-expect-error — a document is an object, not a string.
BSON.encode("x");
// @ts-expect-error — a Timestamp is { t, i } or a bigint.
new BSON.Timestamp(1);
void [doc, valid, when, hex, vector, t, re];
