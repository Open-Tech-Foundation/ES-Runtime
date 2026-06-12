# ES-Runtime
V8-based ECMAScript runtime, WinterTC-compliant, I/O-injectable, capability-secured.

It ships in two shapes from the same core:

- **Embeddable library** (`es-runtime-runtime`) — a driven (tick/poll) runtime with
  all I/O injected via provider traits and V8 kept behind an engine abstraction.
- **Standalone CLI** (`esrun`) — a thin binary that wires the default tokio
  providers and runs JavaScript files end-to-end.

## Quick start (run JavaScript)

```sh
# Build the CLI (optimized binary at target/release/esrun)
cargo build --release -p es-runtime-cli

# Run a file
cargo esrun examples/hello.js          # alias for: cargo run -p es-runtime-cli --
cargo esrun examples/crypto.js
cargo esrun examples/timers.js

# Or run the built binary directly
./target/release/esrun examples/hello.js

# Inline snippet, help, version
cargo esrun -e "console.log(6 * 7)"
cargo esrun --help
```

The full implemented WinterTC surface is available (console, URL, fetch, crypto,
streams, encoding, timers, events); all host capabilities are granted. Top-level
`await` works (the CLI wraps the script in an async context).

## Common tasks

| Task | Command |
| --- | --- |
| Run a JS file | `cargo esrun <file.js>` |
| Run tests | `cargo test --workspace` |
| Lints + format check | `cargo clippy --workspace --all-targets -- -D warnings` · `cargo fmt --check` |
| Supply-chain gates | `cargo deny check` · `cargo audit` |
| Startup/throughput benchmark | `cargo bench-run` |

## License

Licensed under the [Apache License, Version 2.0](LICENSE). See the [NOTICE](NOTICE) file for attribution.

```
ES-Runtime
Copyright 2026 Open Tech Foundation <https://opentechf.org> and its contributors
```
