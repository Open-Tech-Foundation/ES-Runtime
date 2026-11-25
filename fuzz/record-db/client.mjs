// The client record.py runs against a real server: the db_wire harness's own
// connect and query (crates/runtime/src/script_fuzz.rs), at the proxy's port.
import { connect, mongodb, mysql, postgres, sql } from "runtime:db";
import { args } from "runtime:process";

const [mode, port] = [Number(args[0]), args[1]];
const URLS = [
  `postgres://fuzz:secret@127.0.0.1:${port}/app?sslmode=disable`,
  `postgres://fuzz:secret@127.0.0.1:${port}/app?sslmode=prefer`,
  `mysql://fuzz:secret@127.0.0.1:${port}/app?ssl-mode=disabled&allowPublicKeyRetrieval=true`,
  `mongodb://127.0.0.1:${port}/app?directConnection=true`,
];
const conn = await connect(URLS[mode], { driver: [postgres, postgres, mysql, mongodb][mode] });
if (mode === 3) {
  console.log(await conn.db().collection("c").find({ a: 1 }).toArray());
} else {
  console.log(await conn.query(sql`select ${1} as x, 'y' as z`));
}
await conn.close();
