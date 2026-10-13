# yaml-test-suite

Test data from [yaml/yaml-test-suite](https://github.com/yaml/yaml-test-suite),
release `data-2022-01-17` (commit `6e6c296ae9c9d2d5c4134b4b64d01b29ac19ff6f`),
the latest data release. MIT licensed; see `LICENSE`.

| File | Origin |
| --- | --- |
| `cases.json` | Every case's `===`, `in.yaml`, `test.event`, `in.json` and `error`, flattened by `generate.js` |
| `generate.js` | ours — writes `cases.json` from a checkout |
| `LICENSE` | the suite's licence, verbatim |

The `yaml_test_suite` test in `crates/runtime/src/yaml/tests.rs` reads
`cases.json` and is part of `cargo test`. A valid case must produce the suite's
events exactly, and its JSON value when it is a single document. An invalid
case must be rejected. **402/402 passing.**

Regenerate it from a checkout inside the repository, because the runtime only
reads files under the project root:

```sh
git clone -b data-2022-01-17 https://github.com/yaml/yaml-test-suite target/yaml-test-suite
esdev crates/runtime/yaml-test-suite/generate.js "$PWD/target/yaml-test-suite"
```
