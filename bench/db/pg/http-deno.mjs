// DB-backed endpoint on Deno: Deno.serve + postgres.js (prepared).
import postgres from "postgres";
import { POINT, POINT_ID, body, checkRow } from "./http-shared.mjs";

const PORT = Number(Deno.env.get("BENCH_PORT")) || 3000;
const sql = postgres(Deno.env.get("PG_URL"), { prepare: true, fetch_types: false });

const first = await sql.unsafe(POINT, [POINT_ID]);
checkRow(first[0]);

Deno.serve(
  { hostname: "127.0.0.1", port: PORT, onListen() {} },
  async () => {
    const rows = await sql.unsafe(POINT, [POINT_ID]);
    return new Response(body(rows[0]), { headers: { "content-type": "application/json" } });
  }
);
