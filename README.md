# ES-Runtime
V8-based ECMAScript runtime, WinterTC-compliant, I/O-injectable, capability-secured.

It ships in two shapes from the same core:

- **Embeddable library** (`es-runtime`) — a driven (tick/poll) runtime with
  all I/O injected via provider traits and V8 kept behind an engine abstraction.
- **Standalone CLI** (`esrun`) — a thin binary that wires the default tokio
  providers and runs JavaScript files end-to-end.

## Quick start (run JavaScript)

**Install** the latest release (Linux/macOS) — downloads a prebuilt binary and
verifies its checksum:

```sh
curl -fsSL https://raw.githubusercontent.com/Open-Tech-Foundation/ES-Runtime/main/install.sh | bash
```

Or **build once** from source (npm-script-style alias; produces a single
self-contained binary at `target/release/esrun` — no extra files or asset
directory needed):

```sh
cargo build-cli           # alias for: cargo build --release -p es-runtime-cli
```

**Then run JS like `node`/`bun`** with the `esrun` binary:

```sh
./target/release/esrun examples/hello.js
./target/release/esrun examples/crypto.js
./target/release/esrun examples/modules/main.mjs   # ES module: import/export + top-level await
./target/release/esrun -e "console.log(6 * 7)"
./target/release/esrun --help
```

To call it simply as `esrun file.js` from anywhere, install it onto your `PATH`:

```sh
cargo install --path crates/runtime-cli   # installs `esrun` to ~/.cargo/bin
esrun examples/timers.js
```

The full implemented WinterTC surface is available (console, URL, fetch, crypto,
streams, encoding, timers, events); all host capabilities are granted. Inputs run
as **ES modules**: static `import`/`export`, dynamic `import()`,
`import.meta.url`, and native top-level `await` all work. Imports resolve as
**local files** (relative or absolute paths, or `file:` URLs) and **bare
specifiers through `node_modules`** for ES module packages (run `npm install`
yourself — nothing is fetched). CommonJS packages and `node:` builtins are
rejected with a clear message; import attributes and remote modules are not
supported yet.

## Common tasks

| Task | Command |
| --- | --- |
| Build the `esrun` binary | `cargo build-cli` |
| Build everything (lib + CLI) | `cargo build-all` |
| Install `esrun` on `PATH` | `cargo install --path crates/runtime-cli` |
| Run a JS file | `esrun <file.js>` (or `./target/release/esrun <file.js>`) |
| Run tests | `cargo test --workspace` |
| Lints + format check | `cargo clippy --workspace --all-targets -- -D warnings` · `cargo fmt --check` |
| Supply-chain gates | `cargo deny check` · `cargo audit` |
| Startup/throughput microbenchmark | `cargo run --release -p es-runtime-default-providers --example bench` |
| Cross-runtime benchmark (vs Node/Bun/Deno) | `bench/run.sh` (see `bench/README.md`) |

## Documentation

- **[API reference](docs/API.md)** — globals, scope/non-goals, the `runtime:` modules and their exports (canonical).
- **[Architecture](docs/ARCHITECTURE.md)** · **[Spec](docs/SPEC.md)** · **[Decisions](docs/DECISIONS.md)** · **[Security review](docs/SECURITY-REVIEW.md)** · **[Licensing](docs/LICENSING.md)**
- **Marketing site** (`@opentf/web`, in [`site/`](site/)) — run with `bun install && bun run dev`.

Every public API is documented in both [`docs/API.md`](docs/API.md) and the site
under `site/app/docs/**`, kept in sync (DECISIONS D27).

## License

Licensed under the [Apache License, Version 2.0](LICENSE). See the [NOTICE](NOTICE) file for attribution.

```
ES-Runtime
Copyright 2026 Open Tech Foundation <https://opentechf.org> and its contributors
```
