/**
 * `@opentf/esrun-mongodb` — the MongoDB driver for `runtime:db`.
 *
 * The driver is built into the runtime (DECISIONS D186): its protocol and its
 * connection pool run in Rust, and `import { mongodb } from "runtime:db"` is
 * the same driver. This package re-exports it as `driver`, with the BSON
 * classes from `runtime:serialization`, so the names MongoDB users reach for
 * are found where they look.
 *
 * ```js
 * import { connect } from "runtime:db";
 * import { driver, ObjectId } from "@opentf/esrun-mongodb";
 *
 * await using client = await connect("mongodb://localhost:27017/app", { driver });
 * const user = await client.db().collection("users").findOne({ _id: new ObjectId(id) });
 * ```
 */
import { BSON } from "runtime:serialization";

export type {
  AggregateOptions,
  AnyBulkWriteOperation,
  BulkWriteResult,
  DeleteResult,
  Filter,
  FindOptions,
  InsertManyResult,
  InsertOneResult,
  MongoCallOptions,
  MongoCollection,
  MongoConnection,
  MongoCursor,
  MongoDb,
  MongoDocument,
  MongoOptions,
  MongoSession,
  MongoUpdateOptions,
  MongoWriteOptions,
  UpdateFilter,
  UpdateResult,
  WriteConcern,
} from "runtime:db";
export { mongodb as driver } from "runtime:db";

export const ObjectId: typeof BSON.ObjectId = BSON.ObjectId;
export const Decimal128: typeof BSON.Decimal128 = BSON.Decimal128;
export const UUID: typeof BSON.UUID = BSON.UUID;
export const Timestamp: typeof BSON.Timestamp = BSON.Timestamp;
export const Binary: typeof BSON.Binary = BSON.Binary;
export const Code: typeof BSON.Code = BSON.Code;
export const BSONRegExp: typeof BSON.BSONRegExp = BSON.BSONRegExp;
export const MinKey: typeof BSON.MinKey = BSON.MinKey;
export const MaxKey: typeof BSON.MaxKey = BSON.MaxKey;
export const Double: typeof BSON.Double = BSON.Double;
export { BSON };
