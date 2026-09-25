// Scripts, procedures with several result sets, streaming, and packets past 16 MiB.
import { connect, DbErrorCode } from "runtime:db";
import { env } from "runtime:process";
import { driver } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.MYSQL_URL ?? "mysql://root:esrun@127.0.0.1:3307/esrun_test?ssl-mode=DISABLED&allowPublicKeyRetrieval=true";
const db = await connect(url, { driver });

// A script: several statements, each reported.
const script = await db.executeScript(`
  DROP TABLE IF EXISTS script_t;
  CREATE TABLE script_t (id INT AUTO_INCREMENT PRIMARY KEY, v INT);
  INSERT INTO script_t (v) VALUES (1), (2), (3);
  SELECT * FROM script_t;
  UPDATE script_t SET v = v + 1 WHERE v > 1;
`);
is(
  script.map((r) => r.changes),
  [0, 0, 3, 0, 2],
  "each statement of a script is reported",
);
is(script[2].lastInsertRowid, 1, "an insert reports its first id");
let failed = null;
try {
  await db.executeScript(
    "INSERT INTO script_t (v) VALUES (9); SELECT * FROM no_such_table_xyz; INSERT INTO script_t (v) VALUES (10)",
  );
} catch (e) {
  failed = e.code;
}
is(failed, DbErrorCode.UndefinedTable, "a failing script throws the failing statement's error");
is(
  (await (await db.query("SELECT COUNT(*) AS n FROM script_t WHERE v IN (9, 10)")).first()).n,
  1,
  "and statements before it stay done — MySQL has no implicit script transaction",
);

// A procedure answers with its result sets and then its status; the first is the caller's.
await db.executeScript(`
  DROP PROCEDURE IF EXISTS two_results;
  CREATE PROCEDURE two_results() BEGIN SELECT 1 AS a; SELECT 2 AS b; END
`);
is(
  (await (await db.query("CALL two_results()")).toArray()).map((r) => r.a),
  [1],
  "a CALL returns its first result set",
);
is((await db.execute("CALL two_results()")).changes, 0, "and runs through execute");
is((await (await db.query("SELECT 3 AS c")).first()).c, 3, "the rest came off the wire");

// A result larger than a batch streams; stopping early leaves the connection usable.
await db.execute("DROP TABLE IF EXISTS stream_t");
await db.execute("CREATE TABLE stream_t (id INT PRIMARY KEY, pad VARCHAR(200))");
// Five thousand rows from a cross join of digits — plain SQL both servers
// speak, where a recursive CTE needs each one's own recursion limit raised.
const digits =
  "(SELECT 0 n UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9)";
await db.execute(
  `INSERT INTO stream_t SELECT a.n * 1000 + b.n * 100 + c.n * 10 + d.n + 1, REPEAT('x', 200)
   FROM ${digits} a, ${digits} b, ${digits} c, ${digits} d WHERE a.n < 5`,
);
let seen = 0;
let ordered = true;
for await (const row of await db.query("SELECT id, pad FROM stream_t ORDER BY id")) {
  if (row.id !== ++seen || row.pad.length !== 200) ordered = false;
}
ok(seen === 5000 && ordered, `every row, in order, across batches (${seen})`);
let some = 0;
for await (const _ of await db.query("SELECT id FROM stream_t")) if (++some === 10) break;
is(
  (await (await db.query("SELECT COUNT(*) AS n FROM stream_t")).first()).n,
  5000,
  "a result abandoned mid-stream is drained",
);

// Past 16 MiB, both ways: the payload is split into continuation packets.
await db.executeScript("SET GLOBAL max_allowed_packet = 67108864");
const wide = await connect(url, { driver });
const big = new Uint8Array(17 * 1024 * 1024);
for (let i = 0; i < big.length; i += 4096) big[i] = i & 0xff;
await wide.execute("DROP TABLE IF EXISTS big_t");
await wide.execute("CREATE TABLE big_t (b LONGBLOB)");
await wide.execute("INSERT INTO big_t VALUES (?)", [big]);
const back = await (await wide.query("SELECT b, LENGTH(b) AS n FROM big_t")).first();
is(back.n, big.length, "the server received every byte");
ok(
  back.b.length === big.length && back.b.every((v, i) => v === big[i]),
  "and every byte came back",
);
await wide.execute("DROP TABLE big_t");
await wide.close();

await db.executeScript("DROP TABLE script_t; DROP TABLE stream_t; DROP PROCEDURE two_results");
await db.close();
if (report("results") > 0) throw new Error("results failed");
