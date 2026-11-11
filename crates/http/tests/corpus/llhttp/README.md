# llhttp request cases

The request-parsing cases from [llhttp](https://github.com/nodejs/llhttp)
(Node.js's HTTP/1 parser), `test/request/*.md`, at the revision in `REVISION`,
under llhttp's MIT licence (`LICENSE`). Read by `crates/http/tests/llhttp.rs`.

The `lenient-*` files are left out: they test leniency llhttp offers behind
flags, which D217 refuses by design. Only the strict `"type": "request"` cases
are run.

To update: fetch the same files at a newer llhttp commit, write its SHA to
`REVISION`, and re-run the test — a case whose verdict changed is reported.
