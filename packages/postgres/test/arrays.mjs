import "../dist/index.js";
import { env } from "runtime:process";
import { connect } from "runtime:db";

const url = env.PG_URL ?? "postgres://postgres:esrun@127.0.0.1:5433/esrun_test?sslmode=disable";
const db = await connect(url);
const one = async (sql) => (await (await db.query(sql)).first()).v;
const show = (v) => JSON.stringify(v, (_k, x) => (typeof x === "bigint" ? `${x}n` : x));

console.log("int4[]:      ", show(await one("SELECT ARRAY[1,2,3] AS v")));
console.log("text[]:      ", show(await one("SELECT ARRAY['a','b'] AS v")));
console.log("with null:   ", show(await one("SELECT ARRAY[1,NULL,3] AS v")));
// An unquoted NULL is absence; a quoted "NULL" is four characters. Conflating
// them would turn data into absence.
console.log("quoted NULL: ", show(await one(`SELECT ARRAY['NULL','x'] AS v`)));
console.log("delimiter:   ", show(await one(`SELECT ARRAY['a,b','c}d'] AS v`)));
console.log("escapes:     ", show(await one(`SELECT ARRAY['say "hi"','back\\\\slash'] AS v`)));
console.log("nested:      ", show(await one("SELECT ARRAY[[1,2],[3,4]] AS v")));
console.log("empty:       ", show(await one("SELECT ARRAY[]::int[] AS v")));
console.log("bigint[]:    ", show(await one("SELECT ARRAY[9007199254740993]::int8[] AS v")));
console.log("bool[]:      ", show(await one("SELECT ARRAY[true,false,NULL]::bool[] AS v")));
console.log("float8[]:    ", show(await one("SELECT ARRAY[1.5,2.25]::float8[] AS v")));
console.log("numeric[]:   ", show(await one("SELECT ARRAY[1.10,2.20]::numeric[] AS v")));
console.log("timestamptz[]:", (await one("SELECT ARRAY['2026-01-02T03:04:05Z'::timestamptz] AS v"))[0] instanceof Date);
console.log("jsonb[]:     ", show(await one(`SELECT ARRAY['{"a":1}'::jsonb] AS v`)));
console.log("lower bound: ", show(await one("SELECT '[2:4]={7,8,9}'::int[] AS v")));
console.log("unknown type:", show(await one("SELECT ARRAY['(1,2)'::point] AS v")));

// Round trip: an array bound as a parameter comes back as one.
const back = await (await db.query("SELECT $1::int[] AS v", [[4, 5, 6]])).first();
console.log("round trip:  ", show(back.v));
await db.close();
