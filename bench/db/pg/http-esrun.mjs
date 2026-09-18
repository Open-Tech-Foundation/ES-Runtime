// DB-backed endpoint on esrun: runtime:http + @opentf/esrun-postgres
// (staged at ./.driver by bench/db/pg/run.sh, like the driver benchmark).
import { serve } from "runtime:http";
import { connect } from "runtime:db";
import { driver as postgres } from "./.driver/index.js";
import { env } from "runtime:process";
import { POINT, POINT_ID, body, checkRow } from "./http-shared.mjs";

const PORT = Number(env.BENCH_PORT) || 3000;
const db = await connect(env.PG_URL, { driver: postgres });

const first = await (await db.query(POINT, [POINT_ID])).first();
checkRow(first);

serve({ hostname: "127.0.0.1", port: PORT }, async () => {
  const row = await (await db.query(POINT, [POINT_ID])).first();
  return new Response(body(row), { headers: { "content-type": "application/json" } });
});
