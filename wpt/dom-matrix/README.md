# DOM behavior matrix

This is a small, portable behavior baseline for esdev's layout-free DOM. It
executes the same cases under esdev, jsdom, and happy-dom; browsers/specs remain
the correctness oracle, while the two Node implementations reveal compatibility
expectations in framework test suites.

```sh
cargo build -p es-runtime-dev-cli
deno test --config=wpt/dom-matrix/deno.json wpt/dom-matrix/cases_test.js
deno run --allow-read --allow-run --allow-write --config=wpt/dom-matrix/deno.json wpt/dom-matrix/run.js
```

The default report is diagnostic. `--strict` exits non-zero for an esdev gap or
a jsdom/happy-dom disagreement; `--json=path` writes the report for review.
Cases marked `intentional-limit` document an esdev design boundary, such as its
strict rejection of malformed HTML rather than browser parser recovery.
