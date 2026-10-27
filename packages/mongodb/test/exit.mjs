// Never closed: an idle client holds sockets but no pending work, so the
// program ends when its own work does. run.sh times this.
import { connect } from "runtime:db";
import { env } from "runtime:process";
import { driver } from "../dist/index.js";

const client = await connect(env.MONGODB_URL, { driver });
await Promise.all([client.ping(), client.ping(), client.ping()]);
console.log("ok exit (the client was left open)");
