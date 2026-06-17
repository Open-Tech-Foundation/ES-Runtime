# ES-Runtime

V8-based ECMAScript runtime, WinterTC-compliant, I/O-injectable, capability-secured.

**Website & docs → [esrun.opentechf.org](https://esrun.opentechf.org)**

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
esrun --help
```

(If you built from source, the binary is at `./target/release/esrun`. To call it
as `esrun` from anywhere, run `cargo install --path crates/runtime-cli`.)

The full implemented WinterTC surface is available (console, URL, fetch, crypto,
streams, encoding, timers, events); all host capabilities are granted. Inputs run
as **ES modules**: static `import`/`export`, dynamic `import()`, `import.meta.url`,
and native top-level `await` all work. Imports resolve as **local files**
(relative or absolute paths, or `file:` URLs) and **bare specifiers through
`node_modules`** for ES module packages (run `npm install` yourself — nothing is
fetched). CommonJS packages and `node:` builtins are rejected with a clear
message; import attributes (`with { type: "json" }`) are fully supported. Remote (`https://`) modules are explicitly unsupported by design to enforce a strict local-only security model.

## TypeScript

`esrun` doesn't execute TypeScript, but it ships editor types for the `runtime:*`
modules:

```sh
esrun types --install   # writes the defs into node_modules/@opentf/esrun and wires tsconfig.json
```

`esrun types` alone prints them to stdout. See
[esrun.opentechf.org/docs/typescript](https://esrun.opentechf.org/docs/typescript).

## Documentation

- **[esrun.opentechf.org](https://esrun.opentechf.org)** — full docs, guides, and
  cross-runtime benchmarks.
- **[API reference](docs/API.md)** — globals, scope/non-goals, the `runtime:`
  modules and their exports (canonical).

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
