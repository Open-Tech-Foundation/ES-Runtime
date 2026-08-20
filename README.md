<div align="center">

# ES-Runtime

[Website](https://esrun.opentechf.org) | [Docs](https://esrun.opentechf.org/docs) | [API](https://esrun.opentechf.org/api)

*An [Open Tech Foundation](https://opentechf.org/) project*
</div>

> ### A secure, standards-based JavaScript runtime for the server. V8-based, WinterTC-compliant, deny-by-default.


## Two binaries

- **`esrun`** — the server runtime. Runs your service and does nothing else: no
  inspector port, no watcher, no test runner, nothing that could weaken the
  capability model it exists to enforce. Deny-by-default.
- **`esdev`** — the development toolchain: TypeScript, bundling, tests, watch,
  a debugger. It runs your program on exactly `esrun`'s runtime, and is **not a
  deployment target**.

## Install

Prebuilt, checksum-verified binaries into `~/.es-runtime/bin` — **`esrun`**, the
server runtime, and **`esdev`**, the development toolchain.

Linux / macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/Open-Tech-Foundation/ES-Runtime/main/install.sh | bash

# Just one of them — a server or CI image has no use for esdev:
curl -fsSL .../install.sh | bash -s -- --only=esrun
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/Open-Tech-Foundation/ES-Runtime/main/install.ps1 | iex

# Just one of them (`irm | iex` cannot pass arguments):
$env:ES_RUNTIME_ONLY = 'esrun'; irm .../install.ps1 | iex
```

Each binary is released under its own tag — `esrun@0.24.0`, `esdev@0.1.0` — and
pins independently with `ESRUN_VERSION` / `ESDEV_VERSION`. `esrun upgrade` and
`esdev upgrade` each update their own binary in place.

Or build from source — self-contained binaries, no extra files or asset
directory:

```sh
cargo build --release -p es-runtime-cli       # or the alias: cargo build-cli
cargo build --release -p es-runtime-dev-cli   # or: cargo build-dev
```

## Run JavaScript

Run JS files like `node`/`bun`:

```sh
esrun examples/hello.js
esrun examples/modules/main.mjs   # ES module: import/export + top-level await
esrun -e='console.log(6 * 7)'
esrun --env-file=.env app.mjs     # load env vars from a .env file
esrun --allow-net app.mjs         # grant one capability at a time
esrun --allow-all app.mjs         # or grant everything (unsandboxed)
esrun --help
```

**Nothing is granted by default.** A run reaches what the command line that
started it named, and nothing else — so a program with no flags computes and
reaches nothing, including no module loader. Two modes widen it, and they cannot
be combined:

```sh
esrun --allow-imports --allow-net app.mjs         # nothing, plus these
esrun --allow-net=api.example.com app.mjs         # ...narrowed to a list
esrun --allow-all --deny-run app.mjs              # everything, minus these
```

Names: `read`, `write`, `imports`, `net`, `listen`, `env`, `run`, `signals`,
`workers`. `--deny-<name>` requires `--allow-all`, and seven of them also take a
comma-separated list that narrows the grant — paths, addresses, program names,
variable names, signal names. A denied operation throws `NotAllowedError`;
importing a `runtime:` module always works.

`esdev`, the development binary, is the opposite: it grants everything, so the
inner loop needs no flags. `esdev --trace-permissions app.mjs` prints the
`esrun` line that grants exactly what a run reached for.

What a run may *load* is a separate question from what running code may reach,
so it has a separate mechanism — `--import-policy=./import-policy.json` takes
JSON with `"allow"` and/or `"deny"` lists of package names and paths. See
[SECURITY.md](SECURITY.md).

## TypeScript

`esrun` doesn't execute TypeScript, but the `runtime:*` modules have editor types
on npm — [`@opentf/esrun-types`](https://www.npmjs.com/package/@opentf/esrun-types):

```sh
esdev --install-types   # adds the package and wires up tsconfig.json
```

See [esrun.opentechf.org/docs/esdev/typescript](https://esrun.opentechf.org/docs/esdev/typescript).

## Development

Build, test, and benchmark from source:

| Task | Command |
| --- | --- |
| Build everything (lib + CLI) | `cargo build-all` |
| Build just the `esrun` binary | `cargo build-cli` |
| Build the `esdev` binary (local development) | `cargo build-dev` |
| ...with the debugger (`--inspect`) | `ES_RUNTIME_INSPECTOR=1 cargo build-dev` |
| Run tests | `cargo test --workspace` |
| Lints + format check | `cargo clippy --workspace --all-targets -- -D warnings` · `cargo fmt --check` |
| Supply-chain gates | `cargo deny check` · `cargo audit` |
| Startup/throughput microbenchmark | `cargo run --release -p es-runtime-default-providers --example bench` |
| Cross-runtime benchmark | `bench/run.sh` (see [`bench/README.md`](bench/README.md)) |

## License

Licensed under the [Apache License, Version 2.0](LICENSE). See the [NOTICE](NOTICE) file for attribution.

```
ES-Runtime
Copyright 2026 Open Tech Foundation <https://opentechf.org> and its contributors
```
