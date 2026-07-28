# Web Platform Tests — `urlpattern`

Vendored from [web-platform-tests/wpt](https://github.com/web-platform-tests/wpt),
`urlpattern/`, at commit `23aac9278460a73394585ff5a15b6a04dfcd5ec8` (2026-06-12).

| File | Origin |
| --- | --- |
| `urlpatterntestdata.json` | `urlpattern/resources/urlpatterntestdata.json`, verbatim |
| `urlpatterntests.js` | `urlpattern/resources/urlpatterntests.js`, verbatim |
| `harness.js` | ours — the slice of `testharness.js` the runner needs |
| `run.js` | ours — loads the data and reports a tally |

Run it:

```
cargo build -p es-runtime-cli
./target/debug/esrun crates/runtime/wpt/urlpattern/run.js
```

**369/369 passing.**

This is **not** wired into `cargo test`. A standard WPT subset is post-1.0
(SPEC §7); this is a reference check, and the gated signal for URLPattern stays
`conformance/urlpattern.js`. Keeping the two vendored files verbatim is what
makes re-syncing from upstream a copy rather than a merge.
