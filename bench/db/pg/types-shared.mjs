// One query, run by every driver, so the type mappings can be put side by side.
//
// Values are chosen to expose where drivers disagree: an int8 past 2^53, a
// numeric with more digits than a double holds, a timestamp with microseconds
// (PostgreSQL keeps six places; a JS Date holds three), and a date, which is a
// calendar day rather than an instant.
export const TYPES_SQL = `
  SELECT
    true                                             AS bool_t,
    32767::int2                                      AS int2_t,
    2147483647::int4                                 AS int4_t,
    42::int8                                         AS int8_small,
    9007199254740993::int8                           AS int8_big,
    1.5::float4                                      AS float4_t,
    0.1::float8                                      AS float8_t,
    12345678901234567890.0987654321::numeric         AS numeric_t,
    'hello'::text                                    AS text_t,
    '\\xdeadbeef'::bytea                              AS bytea_t,
    '11111111-2222-3333-4444-555555555555'::uuid     AS uuid_t,
    '{"a":1}'::json                                  AS json_t,
    '{"a":1}'::jsonb                                 AS jsonb_t,
    '1985-04-12'::date                               AS date_t,
    '13:45:30.5'::time                               AS time_t,
    '2026-01-02 03:04:05.123456'::timestamp          AS timestamp_t,
    '2026-01-02 03:04:05.123456+00'::timestamptz     AS timestamptz_t,
    '3 mons 4 days 05:00:00'::interval               AS interval_t,
    ARRAY[1,2,3]::int4[]                             AS int4_array,
    ARRAY['a','b']::text[]                           AS text_array,
    NULL::int4                                       AS null_t
`;

/** `TYPE(value)` for one column, in a form every runtime prints identically. */
export function describe(v) {
  if (v === null || v === undefined) return "null";
  if (typeof v === "bigint") return `bigint ${v}`;
  if (typeof v === "number") return `number ${v}`;
  if (typeof v === "boolean") return `boolean ${v}`;
  if (typeof v === "string") return `string ${JSON.stringify(v)}`;
  if (v instanceof Date) return `Date ${v.toISOString()}`;
  if (v instanceof Uint8Array) return `Uint8Array ${[...v].map((b) => b.toString(16).padStart(2, "0")).join("")}`;
  if (ArrayBuffer.isView(v)) return `${v.constructor.name} ${[...new Uint8Array(v.buffer, v.byteOffset, v.byteLength)].map((b) => b.toString(16).padStart(2, "0")).join("")}`;
  if (Array.isArray(v)) return `Array [${v.map((x) => describe(x)).join(", ")}]`;
  if (typeof Temporal !== "undefined") {
    for (const [name, ctor] of Object.entries(Temporal)) {
      if (typeof ctor === "function" && v instanceof ctor) return `Temporal.${name} ${v.toString()}`;
    }
  }
  if (typeof v === "object") {
    const tag = v.constructor?.name ?? "object";
    return `${tag} ${JSON.stringify(v)}`;
  }
  return `${typeof v} ${String(v)}`;
}

export const COLUMNS = [
  "bool_t", "int2_t", "int4_t", "int8_small", "int8_big", "float4_t", "float8_t",
  "numeric_t", "text_t", "bytea_t", "uuid_t", "json_t", "jsonb_t", "date_t",
  "time_t", "timestamp_t", "timestamptz_t", "interval_t", "int4_array",
  "text_array", "null_t",
];
