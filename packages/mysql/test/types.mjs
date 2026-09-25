// Every column family, through the binary protocol, both ways.
import { connect } from "runtime:db";
import { env } from "runtime:process";
import { driver } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.MYSQL_URL ?? "mysql://root:esrun@127.0.0.1:3307/esrun_test?ssl-mode=DISABLED";
const db = await connect(url, { driver });
await db.execute("DROP TABLE IF EXISTS types");
await db.execute(`CREATE TABLE types (
  ti TINYINT, tu TINYINT UNSIGNED, si SMALLINT, mi MEDIUMINT, i INT, iu INT UNSIGNED,
  bi BIGINT, bu BIGINT UNSIGNED, f FLOAT, d DOUBLE, dec_ DECIMAL(30,10), y YEAR,
  dt DATE, dtt DATETIME(6), ts TIMESTAMP(6) NULL, tm TIME(6),
  s VARCHAR(20), t TEXT, b VARBINARY(8), bl BLOB, j JSON, e ENUM('a','b'), st SET('x','y'), bit_ BIT(8)
)`);
await db.execute("INSERT INTO types VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)", [
  -128,
  255,
  -32768,
  -8388608,
  -2147483648,
  4294967295,
  -9223372036854775808n,
  18446744073709551615n,
  1.5,
  0.1,
  "12345678901234567890.0987654321",
  2026,
  Temporal.PlainDate.from("1985-04-12"),
  Temporal.PlainDateTime.from("2026-01-02T03:04:05.123456"),
  Temporal.Instant.from("2026-01-02T03:04:05.654321Z"),
  Temporal.Duration.from({ hours: -29, minutes: -58, seconds: -58 }),
  "héllo",
  "text",
  new Uint8Array([0xde, 0xad]),
  new Uint8Array([1, 2, 3]),
  { a: [1, 2] },
  "b",
  "x,y",
  5,
]);
const row = await (await db.query("SELECT * FROM types")).first();
is(row.ti, -128, "tinyint");
is(row.tu, 255, "tinyint unsigned");
is(row.si, -32768, "smallint");
is(row.mi, -8388608, "mediumint");
is(row.i, -2147483648, "int");
is(row.iu, 4294967295, "int unsigned");
is(String(row.bi), "-9223372036854775808", "bigint min");
is(typeof row.bi, "bigint", "bigint past 2^53 is a bigint");
is(String(row.bu), "18446744073709551615", "bigint unsigned max");
is(row.f, 1.5, "float");
is(row.d, 0.1, "double");
is(row.dec_, "12345678901234567890.0987654321", "decimal stays exact text");
is(row.y, 2026, "year");
ok(
  row.dt instanceof Temporal.PlainDate && row.dt.toString() === "1985-04-12",
  "date is a PlainDate",
);
ok(
  row.dtt instanceof Temporal.PlainDateTime && row.dtt.toString() === "2026-01-02T03:04:05.123456",
  "datetime is a PlainDateTime",
);
ok(
  row.ts instanceof Temporal.Instant && row.ts.toString() === "2026-01-02T03:04:05.654321Z",
  "timestamp is an Instant",
);
ok(
  row.tm instanceof Temporal.Duration && row.tm.toString() === "-PT29H58M58S",
  `time is a Duration (${row.tm})`,
);
is(row.s, "héllo", "varchar");
is(row.t, "text", "text");
is([...row.b], [0xde, 0xad], "varbinary is bytes");
is([...row.bl], [1, 2, 3], "blob is bytes");
is(row.j, { a: [1, 2] }, "json is parsed");
is(row.e, "b", "enum");
is(row.st, "x,y", "set");
is([...row.bit_], [5], "bit is bytes");

// NULL in every column, and the bitmap past the first byte.
await db.execute("DELETE FROM types");
await db.execute("INSERT INTO types () VALUES ()");
const nulls = await (await db.query("SELECT * FROM types")).first();
ok(
  Object.values(nulls.toObject()).every((v) => v === null),
  "every column NULL",
);

// The same parameter slot as a number, then as a string: types go with every
// execution. (Through CONCAT, because `SELECT ?` alone fixes its *result* type at
// the first execution — the server's rule, not the driver's.)
const back = [];
for (const v of [7, "seven", 7n, true, null]) {
  back.push((await (await db.query("SELECT CONCAT(?, '') AS v", [v])).first()).v);
}
is(back.map(String), ["7", "seven", "7", "1", "null"], "a slot's type may change between calls");

// temporal: false
const legacy = await connect(url, { driver, temporal: false });
await legacy.execute(
  "INSERT INTO types (dt, dtt, tm) VALUES ('2020-01-02', '2020-01-02 03:04:05', '-01:02:03')",
);
const old = await (
  await legacy.query("SELECT dt, dtt, tm FROM types WHERE dt IS NOT NULL")
).first();
is(old.dt, "2020-01-02", "legacy date is text");
ok(
  old.dtt instanceof Date && old.dtt.toISOString() === "2020-01-02T03:04:05.000Z",
  "legacy datetime is a Date",
);
is(old.tm, "-01:02:03", "legacy time is text");
await legacy.close();

await db.execute("DROP TABLE types");
await db.close();
if (report("types") > 0) throw new Error("types failed");
