# DOM behavior matrix

This is a small, portable behavior baseline for esdev's layout-free DOM. It
executes the same cases under headless Google Chrome, esdev, jsdom, and
happy-dom. Chrome is the behavior oracle; jsdom and happy-dom reveal framework
compatibility expectations only.

```sh
cargo build -p es-runtime-dev-cli
deno test --config=wpt/dom-matrix/deno.json wpt/dom-matrix/cases_test.js
deno run --allow-env --allow-net=127.0.0.1 --allow-read --allow-run --allow-write --config=wpt/dom-matrix/deno.json wpt/dom-matrix/run.js
```

The default report is diagnostic. `--strict` exits non-zero only when esdev
differs from Chrome; `--chrome=path` selects the Chrome executable and
`--json=path` writes the report for review. Cases marked `intentional-limit`
document an esdev design boundary, such as its strict rejection of malformed
HTML rather than browser parser recovery.
