<div align="center">

# ES-Runtime

[Website](https://esrun.opentechf.org) | [Docs](https://esrun.opentechf.org/docs) | [API](https://esrun.opentechf.org/api)

*Part of the <img src="https://raw.githubusercontent.com/Open-Tech-Foundation/website/3ed7ac70ec44465eec0f94e5185cb28a9b11ed07/static/img/OTF-Logo.svg" width="24" align="center" /> [Open Tech Foundation](https://github.com/Open-Tech-Foundation) ecosystem.*
</div>

> ### V8-based ECMAScript runtime, WinterTC-compliant, I/O-injectable, capability-secured.


## Shapes

It ships in two shapes from the same core:

- **Embeddable library** (`es-runtime`) — a driven (tick/poll) runtime with
  all I/O injected via provider traits and V8 kept behind an engine abstraction.
- **Standalone CLI** (`esrun`) — a thin binary that wires the default tokio
  providers and runs JavaScript files end-to-end.

## Install

A prebuilt, checksum-verified binary:

Linux / macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/Open-Tech-Foundation/ES-Runtime/main/install.sh | bash
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/Open-Tech-Foundation/ES-Runtime/main/install.ps1 | iex
```

Or build from source — a single self-contained binary at `target/release/esrun`,
no extra files or asset directory:

```sh
cargo build --release -p es-runtime-cli   # or the alias: cargo build-cli
```

## Run JavaScript

Run JS files like `node`/`bun`:

```sh
esrun examples/hello.js
esrun examples/modules/main.mjs   # ES module: import/export + top-level await
esrun -e "console.log(6 * 7)"
esrun --env-file .env app.mjs     # load env vars from a .env file
esrun --help
```

## TypeScript

`esrun` doesn't execute TypeScript, but it ships editor types for the `runtime:*`
modules:

```sh
esrun types --install   # writes the defs into node_modules/@opentf/esrun and wires tsconfig.json
```

`esrun types` alone prints them to stdout. See
[esrun.opentechf.org/docs/typescript](https://esrun.opentechf.org/docs/typescript).

## Development

Build, test, and benchmark from source:

| Task | Command |
| --- | --- |
| Build everything (lib + CLI) | `cargo build-all` |
| Build just the `esrun` binary | `cargo build-cli` |
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
