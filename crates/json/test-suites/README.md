# JSON test suites

Test data from two suites, both MIT licensed; see each `LICENSE-*` file.

| Suite | Commit | Used |
| --- | --- | --- |
| [nst/JSONTestSuite](https://github.com/nst/JSONTestSuite) | `1ef36fa01286573e846ac449e8683f8833c5b26a` (2024-11-22) | `test_parsing/` |
| [json5/json5-tests](https://github.com/json5/json5-tests) | `ceb24d4080137d70833f86c25659c1331b80a387` (2026-02-20) | every case |

| File | Origin |
| --- | --- |
| `cases.json` | every case's text, expected outcome, and the engine's `JSON.parse` result, flattened by `generate.js` |
| `generate.js` | ours — writes `cases.json` from the two checkouts |
| `LICENSE-JSONTestSuite`, `LICENSE-json5-tests` | the suites' licences, verbatim |

The `json_test_suite_and_json5_tests` test in `crates/json/src/tests.rs` reads
`cases.json` and runs every case in both dialects as part of `cargo test`:

- A JSONTestSuite `y_` case must be accepted with `JSON.parse`'s value, an `n_`
  case refused, and an `i_` case treated as `JSON.parse` treats it.
- A json5-tests `.json` or `.json5` case must be accepted, a `.js` or `.txt`
  case refused, and where an `errorSpec` gives the position, the error must be
  there. `.json` values must equal `JSON.parse`'s.
- Where JSONC or JSON5 deliberately accepts more than the suite expects (a
  trailing comma, a comment, JSON5's numbers), the case is listed in the
  test's `DIFFERENCES` with the reason. A listed case that stops differing
  fails the run.

The 25 JSONTestSuite cases that are not UTF-8 are skipped: a JavaScript string
cannot hold them, so they test a decoder, not this parser.

Regenerate from checkouts inside the repository, because the runtime only reads
files under the project root:

```sh
git clone https://github.com/nst/JSONTestSuite target/JSONTestSuite
git -C target/JSONTestSuite checkout 1ef36fa01286573e846ac449e8683f8833c5b26a
git clone https://github.com/json5/json5-tests target/json5-tests
git -C target/json5-tests checkout ceb24d4080137d70833f86c25659c1331b80a387
esdev crates/json/test-suites/generate.js "$PWD/target/JSONTestSuite" "$PWD/target/json5-tests"
```
