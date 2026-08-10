// Bun's built-in SQL client, rather than postgres.js — the user asked about
// Bun's own mapping.
import { SQL } from "bun";
import { TYPES_SQL, COLUMNS, describe } from "./types-shared.mjs";

const sql = new SQL(process.env.PG_URL);
const [row] = await sql.unsafe(TYPES_SQL);
for (const c of COLUMNS) console.log(`${c}\t${describe(row[c])}`);
await sql.close();
