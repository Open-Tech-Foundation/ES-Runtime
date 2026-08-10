import postgres from "postgres";
import { TYPES_SQL, COLUMNS, describe } from "./types-shared.mjs";

const sql = postgres(process.env.PG_URL);
const [row] = await sql.unsafe(TYPES_SQL);
for (const c of COLUMNS) console.log(`${c}\t${describe(row[c])}`);
await sql.end();
