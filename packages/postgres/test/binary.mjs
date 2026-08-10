import { env } from "runtime:process";
import postgres from "../dist/index.js";
import { connect } from "runtime:db";

const url = env.PG_URL ?? "postgres://postgres:esrun@127.0.0.1:5433/esrun_test?sslmode=disable";

const SQL = `
  SELECT
    (-32768)::int2 AS i2, 2147483647::int4 AS i4,
    9007199254740993::int8 AS big, 42::int8 AS small_big,
    1.5::float4 AS f4, 0.1::float8 AS f8,
    true AS t, false AS f,
    '\\xdeadbeef'::bytea AS blob,
    '11111111-2222-3333-4444-555555555555'::uuid AS id,
    '2026-01-02 03:04:05.123456+00'::timestamptz AS ts,
    '2026-01-02 03:04:05.123456'::timestamp AS ts_plain,
    '1985-04-12'::date AS d,
    '13:45:30.5'::time AS t_only,
    '3 mons 4 days 05:00:00'::interval AS iv,
    12345678901234567890.0987654321::numeric AS exact,
    '{"a":[1,2]}'::jsonb AS doc,
    ARRAY[1,2,3] AS arr,
    'hello'::text AS s,
    NULL::int4 AS nothing
`;

const describe = (v) => {
  if (v === null) return "null";
  if (v instanceof Uint8Array) return `bytes(${[...v].map((b) => b.toString(16)).join("")})`;
  if (v instanceof Date) return `date(${v.toISOString()})`;
  if (typeof Temporal !== "undefined") {
    for (const [name, ctor] of Object.entries(Temporal)) {
      if (typeof ctor === "function" && v instanceof ctor) return `Temporal.${name}(${v})`;
    }
  }
  if (typeof v === "bigint") return `bigint(${v})`;
  if (Array.isArray(v)) return `array(${v.join(",")})`;
  if (typeof v === "object") return `json(${JSON.stringify(v)})`;
  return `${typeof v}(${v})`;
};

const read = async (cacheSize, temporal = true) => {
  const db = await connect(url, { driver: postgres, preparedStatementCacheSize: cacheSize, temporal });
  const row = await (await db.query(SQL)).first();
  const out = Object.entries(row.toObject()).map(([k, v]) => `${k}=${describe(v)}`);
  await db.close();
  return out;
};

// Caching off means the text path; on means binary is requested for every type
// that has one. The values must be identical — a wire format is how a value
// travels, not what it is.
const asText = await read(0);
const asBinary = await read(100);

let mismatches = 0;
for (let i = 0; i < asText.length; i++) {
  if (asText[i] !== asBinary[i]) {
    mismatches++;
    console.log(`  DIFFERS text=${asText[i]}  binary=${asBinary[i]}`);
  }
}
console.log("text and binary agree:", mismatches === 0);
console.log(asBinary.join("\n"));

// The same check for the legacy mapping: turning Temporal off must not make the
// two wire formats disagree either.
const legacyText = await read(0, false);
const legacyBinary = await read(100, false);
let legacyMismatches = 0;
for (let i = 0; i < legacyText.length; i++) {
  if (legacyText[i] !== legacyBinary[i]) {
    legacyMismatches++;
    console.log(`  LEGACY DIFFERS text=${legacyText[i]}  binary=${legacyBinary[i]}`);
  }
}
console.log("legacy text and binary agree:", legacyMismatches === 0);
console.log("legacy dates are Dates:", legacyBinary.some((v) => v.includes("date(")));
