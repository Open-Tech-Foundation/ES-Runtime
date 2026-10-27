# MongoDB specification tests

Runs MongoDB's driver specification tests, unmodified, against esrun.

```sh
./mongodb-specs/fetch.sh     # pinned sparse checkout → mongodb-specs/upstream
tsr test:bson-corpus         # fetches, builds esrun, runs bson-corpus.js
```

The tests are fetched rather than committed: the
[specifications repository](https://github.com/mongodb/specifications) is
licensed CC BY-NC-SA 3.0, which this Apache-2.0 tree does not redistribute. For
the same reason nothing derived from them (fuzz seeds included) is committed.

| Runner | Suites | Against |
| --- | --- | --- |
| `bson-corpus.js` | `bson-corpus`, `bson-binary-vector` | `BSON` in `runtime:serialization` |

Extended JSON is not implemented, so its cases are not run. A case whose bytes
cannot come back because of how JavaScript represents a value is listed in the
runner's `DIFFERENCES` with the reason; any other failure fails the run, and so
does a listed difference that starts passing.

The driver suites (connection string, server discovery and monitoring, CRUD,
transactions) join `DIRS` in `fetch.sh` as the MongoDB driver (D186) reaches
them.
