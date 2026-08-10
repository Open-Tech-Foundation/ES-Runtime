import { args, env } from "runtime:process";
import { connect } from "runtime:db";
import postgres from "./.driver/index.js";
import { TYPES_SQL, COLUMNS, describe } from "./types-shared.mjs";

// Caching off asks for everything in text; on asks for binary where the driver
// has a binary decoder. Both must produce the same values.
const cache = args[0] === "text" ? 0 : 100;
const db = await connect(env.PG_URL, { preparedStatementCacheSize: cache }, { driver: postgres });
const row = await (await db.query(TYPES_SQL)).first();
const values = row.toObject();
for (const c of COLUMNS) console.log(`${c}\t${describe(values[c])}`);
await db.close();
