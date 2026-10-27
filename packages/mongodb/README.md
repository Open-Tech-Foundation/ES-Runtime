# @opentf/esrun-mongodb

The MongoDB driver for [ES Runtime](https://esrun.opentechf.org) (`esrun`).

**The driver is built into the runtime**: its protocol and its connection pool
run in Rust, and `import { mongodb } from "runtime:db"` is the same driver. This
package re-exports it as `driver`, together with the BSON classes and the
TypeScript types for the collection API.

```sh
npm install @opentf/esrun-mongodb
```

```js
import { connect } from "runtime:db";
import { driver, ObjectId } from "@opentf/esrun-mongodb";

await using client = await connect("mongodb://localhost:27017/app", { driver });
const users = client.db().collection("users");

const { insertedId } = await users.insertOne({ name: "Ada", joined: new Date() });
await users.updateOne({ _id: insertedId }, { $set: { verified: true } });

for await (const user of users.find({ verified: true }).sort({ joined: -1 }).limit(10)) {
  console.log(user.name);
}
```

Every operation takes `signal` and `timeoutMS`, cursors are async iterables, and
`await using` closes the client. An int64 reads as a `bigint`. Failures are
`runtime:db`'s `DbError`, with the server's code, name and labels on
`error.server`.

It connects to MongoDB 7.0 or later — one server or a replica set — with
SCRAM-SHA-256 or SCRAM-SHA-1 and TLS, and supports transactions, sessions, read
preferences and retried writes and reads. Sharded clusters, `mongodb+srv://`,
change streams and other authentication mechanisms are not supported yet. See the
[MongoDB guide](https://esrun.opentechf.org/docs/db/mongodb).

Running esrun needs `--allow-net` for the server's host and port.

## License

Apache-2.0
