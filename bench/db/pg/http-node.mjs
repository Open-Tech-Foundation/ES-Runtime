// DB-backed endpoint on Node: node:http + postgres.js (prepared).
import http from "node:http";
import postgres from "postgres";
import { POINT, POINT_ID, body, checkRow } from "./http-shared.mjs";

const PORT = Number(process.env.BENCH_PORT) || 3000;
const sql = postgres(process.env.PG_URL, { prepare: true, fetch_types: false });

const first = await sql.unsafe(POINT, [POINT_ID]);
checkRow(first[0]);

http
  .createServer(async (req, res) => {
    const rows = await sql.unsafe(POINT, [POINT_ID]);
    const payload = body(rows[0]);
    res.setHeader("content-type", "application/json");
    res.end(payload);
  })
  .listen(PORT, "127.0.0.1");
