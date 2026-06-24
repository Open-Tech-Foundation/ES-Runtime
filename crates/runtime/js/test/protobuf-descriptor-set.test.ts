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
