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

`baseline.json` is the last recorded run, and every run is compared against it
by case name. A case whose four columns no longer read the way the record says
counts as `drift` — including one of the emulators moving, which is the only way
a changed compatibility expectation ever announces itself. Re-record with
`--update-baseline`, in the commit that explains the change.

`--strict` exits non-zero on a gap or on drift; without it the report is
diagnostic. `--chrome=path` selects the Chrome executable, `--esdev=path` the
binary under test, and `--json=path` writes the report somewhere else as well.

Cases marked `intentional-limit` document an esdev design boundary, such as its
strict rejection of malformed HTML rather than browser parser recovery. Chrome
alone decides `match` and `gap`; `emulatorDisagreement` records that jsdom or
happy-dom went its own way, which is context for framework compatibility and
never a verdict on esdev.
