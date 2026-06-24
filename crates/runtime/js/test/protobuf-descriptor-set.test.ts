import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { Schema } from "../serialization/protobuf/schema.js";

// The .proto the fixture was compiled from (protoc --descriptor_set_out
// --include_imports). Kept in sync with scratchpad/proto/order.proto.
const ORDER_PROTO = `
  syntax = "proto3";
  package shop;
  import "google/protobuf/timestamp.proto";
  enum Tier { FREE = 0; PRO = 1; }
  message Order {
    string id = 1;
    uint64 total_cents = 2;
    repeated Item items = 3;
    map<string, int32> counts = 4;
    oneof payment { string card = 5; string invoice = 6; }
    Tier tier = 7;
    google.protobuf.Timestamp created_at = 8;
    bytes signature = 9;
    optional int32 discount = 10;
    message Item { string sku = 1; int32 qty = 2; }
  }`;

const fdset = new Uint8Array(readFileSync(new URL("./fixtures/order.fdset", import.meta.url)));

const value = {
  id: "o1",
  totalCents: 9007199254740993n,
  items: [{ sku: "a", qty: 2 }, { sku: "b", qty: 1 }],
  counts: { a: 2, b: 1 },
  card: "4242",            // sets the "payment" oneof
  tier: "PRO",
  createdAt: { seconds: 1704164645n },
  signature: new Uint8Array([1, 2, 3, 255]),
  discount: 0,             // proto3 optional → explicit presence, kept at 0
};

test("fromDescriptorSet encodes byte-identically to a .proto-text schema", () => {
  const fromSet = Schema.fromDescriptorSet(fdset);
  const fromText = new Schema(ORDER_PROTO);
  expect([...fromSet.encode("shop.Order", value)]).toEqual([...fromText.encode("shop.Order", value)]);
});

test("fromDescriptorSet round-trips binary and JSON (maps, oneofs, enums, nested, WKT)", () => {
  const s = Schema.fromDescriptorSet(fdset);
  expect(s.decode("shop.Order", s.encode("shop.Order", value))).toEqual(value);
  const json = s.toJson("shop.Order", value);
  expect(s.fromJson("shop.Order", json)).toEqual(value);
});

// A minimal descriptor.proto subset, used to synthesize a proto2 descriptor set
// in-memory (a proto2 FileDescriptorProto leaves `syntax` unset).
const META = `
  syntax = "proto3"; package google.protobuf;
  message FileDescriptorSet { repeated FileDescriptorProto file = 1; }
  message FileDescriptorProto {
    string name = 1; string package = 2;
    repeated DescriptorProto message_type = 4; string syntax = 12;
  }
  message DescriptorProto { string name = 1; repeated FieldDescriptorProto field = 2; }
  message FieldDescriptorProto {
    enum Label { LABEL_UNKNOWN = 0; LABEL_OPTIONAL = 1; LABEL_REQUIRED = 2; LABEL_REPEATED = 3; }
    enum Type {
      TYPE_UNKNOWN = 0; TYPE_DOUBLE = 1; TYPE_FLOAT = 2; TYPE_INT64 = 3; TYPE_UINT64 = 4;
      TYPE_INT32 = 5; TYPE_FIXED64 = 6; TYPE_FIXED32 = 7; TYPE_BOOL = 8; TYPE_STRING = 9;
      TYPE_GROUP = 10; TYPE_MESSAGE = 11; TYPE_BYTES = 12; TYPE_UINT32 = 13; TYPE_ENUM = 14;
    }
    string name = 1; int32 number = 3; Label label = 4; Type type = 5; string type_name = 6;
  }`;

test("fromDescriptorSet accepts a proto2 descriptor (unset syntax, required label)", () => {
  const meta = new Schema(META);
  const proto2Set = meta.encode("google.protobuf.FileDescriptorSet", {
    file: [{
      name: "person.proto",
      package: "people",
      // no `syntax` member → proto2
      messageType: [{
        name: "Person",
        field: [
          { name: "name", number: 1, label: "LABEL_REQUIRED", type: "TYPE_STRING" },
          { name: "age", number: 2, label: "LABEL_OPTIONAL", type: "TYPE_INT32" },
        ],
      }],
    }],
  });

  const fromSet = Schema.fromDescriptorSet(proto2Set);
  const fromText = new Schema(`syntax="proto2"; package people; message Person { required string name = 1; optional int32 age = 2; }`);
  const v = { name: "ada", age: 0 }; // proto2 optional → explicit presence, 0 is kept
  expect([...fromSet.encode("people.Person", v)]).toEqual([...fromText.encode("people.Person", v)]);
  expect(fromSet.decode("people.Person", fromSet.encode("people.Person", v))).toEqual(v);
});
