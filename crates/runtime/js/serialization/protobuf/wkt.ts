// Embedded google/protobuf well-known-type sources so `import` of them resolves
// without a filesystem. Binary decode/encode treats these as ordinary messages
// (their special JSON forms are a deferred concern).
export const WKT: Record<string, string> = {
  "google/protobuf/timestamp.proto": `
    syntax = "proto3"; package google.protobuf;
    message Timestamp { int64 seconds = 1; int32 nanos = 2; }
  `,
  "google/protobuf/duration.proto": `
    syntax = "proto3"; package google.protobuf;
    message Duration { int64 seconds = 1; int32 nanos = 2; }
  `,
  "google/protobuf/empty.proto": `
    syntax = "proto3"; package google.protobuf;
    message Empty {}
  `,
  "google/protobuf/field_mask.proto": `
    syntax = "proto3"; package google.protobuf;
    message FieldMask { repeated string paths = 1; }
  `,
  "google/protobuf/any.proto": `
    syntax = "proto3"; package google.protobuf;
    message Any { string type_url = 1; bytes value = 2; }
  `,
  "google/protobuf/wrappers.proto": `
    syntax = "proto3"; package google.protobuf;
    message DoubleValue { double value = 1; }
    message FloatValue { float value = 1; }
    message Int64Value { int64 value = 1; }
    message UInt64Value { uint64 value = 1; }
    message Int32Value { int32 value = 1; }
    message UInt32Value { uint32 value = 1; }
    message BoolValue { bool value = 1; }
    message StringValue { string value = 1; }
    message BytesValue { bytes value = 1; }
  `,
  "google/protobuf/struct.proto": `
    syntax = "proto3"; package google.protobuf;
    message Struct { map<string, Value> fields = 1; }
    message Value {
      oneof kind {
        NullValue null_value = 1;
        double number_value = 2;
        string string_value = 3;
        bool bool_value = 4;
        Struct struct_value = 5;
        ListValue list_value = 6;
      }
    }
    message ListValue { repeated Value values = 1; }
    enum NullValue { NULL_VALUE = 0; }
  `,
};
