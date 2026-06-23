// Conformance testee: speaks the protobuf conformance wire protocol on
// stdin/stdout (4-byte LE length prefix + ConformanceRequest/Response), backed
// by our reflective Protobuf lib. Run by the official conformance_test_runner.
//
//   PB_SRC=/path/to/protobuf bun conformance-testee.mjs
//
// We implement binary<->binary for proto3 + edition 2023. JSON / JSPB /
// TEXT_FORMAT and proto2 message types are reported as `skipped`.
import { readFileSync } from "node:fs";
import { Schema } from "./serialization/protobuf/schema.ts";

const P = process.env.PB_SRC;
const read = (rel) => readFileSync(`${P}/${rel}`, "utf8");

const conf = new Schema(read("conformance/conformance.proto"));
const p3 = new Schema({ "google/protobuf/test_messages_proto3.proto": read("src/google/protobuf/test_messages_proto3.proto") });
const ed = new Schema({ "x.proto": read("conformance/test_protos/test_messages_edition2023.proto") });
const edP3 = new Schema({ "x.proto": read("editions/golden/test_messages_proto3_editions.proto") });
const edP2 = new Schema({ "x.proto": read("editions/golden/test_messages_proto2_editions.proto") });

const REQ = "conformance.ConformanceRequest";
const RESP = "conformance.ConformanceResponse";

function schemaFor(messageType) {
  // Most-specific package prefix first.
  if (messageType.startsWith("protobuf_test_messages.editions.proto3.")) return edP3;
  if (messageType.startsWith("protobuf_test_messages.editions.proto2.")) return edP2;
  if (messageType.startsWith("protobuf_test_messages.editions.")) return ed;
  if (messageType.startsWith("protobuf_test_messages.proto3.")) return p3;
  return null; // proto2 syntax (or unknown) — unsupported
}

function handle(reqBytes) {
  const req = conf.decode(REQ, reqBytes);
  const mt = req.messageType ?? "";

  if (mt === "conformance.FailureSet") {
    return conf.encode(RESP, { protobufPayload: conf.encode("conformance.FailureSet", {}) });
  }

  // We only do binary <-> binary.
  const out = req.requestedOutputFormat; // enum name
  if (req.jsonPayload !== undefined || req.jspbPayload !== undefined || req.textPayload !== undefined) {
    return conf.encode(RESP, { skipped: "non-protobuf input unsupported" });
  }
  if (out !== "PROTOBUF") {
    return conf.encode(RESP, { skipped: `output ${out} unsupported` });
  }

  const schema = schemaFor(mt);
  if (!schema) return conf.encode(RESP, { skipped: "proto2/unknown message type unsupported" });

  try {
    const msg = schema.decode(mt, req.protobufPayload ?? new Uint8Array(0));
    try {
      const bytes = schema.encode(mt, msg);
      return conf.encode(RESP, { protobufPayload: bytes });
    } catch (e) {
      return conf.encode(RESP, { serializeError: String(e?.message ?? e) });
    }
  } catch (e) {
    const msg = String(e?.message ?? e);
    if (msg.includes("unknown message")) return conf.encode(RESP, { skipped: msg });
    return conf.encode(RESP, { parseError: msg });
  }
}

// --- framed stdin/stdout loop ---
const reader = Bun.stdin.stream().getReader();
const sink = Bun.stdout.writer();
let buf = new Uint8Array(0);

function concat(a, b) {
  const out = new Uint8Array(a.length + b.length);
  out.set(a); out.set(b, a.length);
  return out;
}
async function readExact(n) {
  while (buf.length < n) {
    const { done, value } = await reader.read();
    if (done) return null;
    buf = concat(buf, value);
  }
  const out = buf.subarray(0, n);
  buf = buf.slice(n);
  return out;
}

for (;;) {
  const lenBuf = await readExact(4);
  if (!lenBuf) break;
  const len = new DataView(lenBuf.buffer, lenBuf.byteOffset, 4).getUint32(0, true);
  const reqBytes = await readExact(len);
  if (!reqBytes) break;

  let resp;
  try {
    resp = handle(reqBytes.slice());
  } catch (e) {
    resp = conf.encode(RESP, { runtimeError: String(e?.message ?? e) });
  }

  const header = new Uint8Array(4);
  new DataView(header.buffer).setUint32(0, resp.length, true);
  sink.write(header);
  sink.write(resp);
  sink.flush();
}
