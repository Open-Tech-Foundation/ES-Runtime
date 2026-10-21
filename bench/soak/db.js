// SQLite churn through runtime:db: open a connection, write in a transaction,
// close — while a long-lived pool reads alongside. A handle, statement or page
// cache that is never released grows with every round. One writer: SQLite
// takes one at a time, and a second would only meet ERR_DB_BUSY.
import { connect, sqlite } from "runtime:db";
import { remove } from "runtime:fs";

const PATH = "soak-db.sqlite";
await remove(PATH).catch(() => {});
const pooled = await connect(`sqlite:${PATH}`, { driver: sqlite, pool: true });
await pooled.execute("CREATE TABLE IF NOT EXISTS items (id INTEGER PRIMARY KEY, body TEXT)");
let done = 0;

async function churn() {
  for (;;) {
    const db = await connect(`sqlite:${PATH}`, { driver: sqlite });
    await db.transaction(async (tx) => {
      for (let i = 0; i < 20; i++) await tx.execute("INSERT INTO items (body) VALUES (?)", ["x".repeat(256)]);
    });
    await db.execute("DELETE FROM items WHERE id < (SELECT max(id) - 1000 FROM items)");
    await db.close();
    done++;
  }
}

async function read() {
  for (;;) {
    let n = 0;
    for await (const row of await pooled.query("SELECT body FROM items ORDER BY id DESC LIMIT 200")) n += row.body.length;
    if (n === 0 && done > 0) throw new Error("pooled read saw no rows");
  }
}

setInterval(() => console.log(`progress ${done}`), 10_000);
await Promise.all([churn(), read(), read()]);
