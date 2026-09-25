// A first conversation with a real server: connect, create, insert, read back.
import { connect, sql } from "runtime:db";
import { env } from "runtime:process";
import { driver } from "../dist/index.js";

const url = env.MYSQL_URL ?? "mysql://root:esrun@127.0.0.1:3307/esrun_test?ssl-mode=DISABLED&allowPublicKeyRetrieval=true";
const db = await connect(url, { driver });
console.log("server:", db.serverVersion.split("-")[0] !== "" ? "ok" : "?");
await db.execute("DROP TABLE IF EXISTS smoke");
await db.execute(
  "CREATE TABLE smoke (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(40), n BIGINT, f DOUBLE)",
);
const inserted = await db.execute("INSERT INTO smoke (name, n, f) VALUES (?, ?, ?)", [
  "héllo",
  9007199254740993n,
  1.5,
]);
console.log("inserted:", inserted.changes, inserted.lastInsertRowid);
await db.execute(sql`INSERT INTO smoke (name, n, f) VALUES (${"two"}, ${2}, ${null})`);
for (const row of await (
  await db.query("SELECT id, name, n, f FROM smoke ORDER BY id")
).toArray()) {
  console.log(row.id, row.name, typeof row.n, String(row.n), row.f);
}
const updated = await db.execute("UPDATE smoke SET name = ? WHERE id > ?", ["x", 0]);
console.log("updated:", updated.changes);
await db.execute("DROP TABLE smoke");
await db.close();
console.log("closed");
