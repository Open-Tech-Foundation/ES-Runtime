# Licensing

ES-Runtime is distributed under the **Apache License, Version 2.0** (see
[`LICENSE`](../LICENSE) and [`NOTICE`](../NOTICE)). This page documents the
licensing of the runtime and the third-party code it links.

## The JavaScript engine: V8

ES-Runtime embeds Google's **V8** engine (via the `v8` crate, which downloads a
prebuilt static library at build time). V8 is licensed under the
**BSD-3-Clause** license — a permissive license.

This is a deliberate, meaningful difference from some other runtimes. Because
V8 is BSD-licensed, static linking carries **no copyleft relink obligation**:
distributing an `esrun` binary does not require us (or embedders) to ship object
files or relinkable artifacts. (By contrast, a runtime that statically links an
LGPL engine such as JavaScriptCore must provide a way for users to relink
against a modified engine.)

## Third-party dependencies

All Rust dependencies are gated to OSI-approved **permissive** licenses by
[`cargo-deny`](../deny.toml): Apache-2.0 (incl. the LLVM exception), MIT, MIT-0,
BSD-2/3-Clause, ISC, Zlib, and Unicode-3.0. Copyleft and unknown licenses fail
CI; any exception must be added deliberately to `deny.toml` with a rationale.
Run the gate locally with:

```sh
cargo deny check
```

## RUSTSEC advisories

A small number of advisories are deliberately accepted (e.g. the `rsa` crate's
RUSTSEC-2023-0071 Marvin timing advisory) and documented in `deny.toml`; see
[`DECISIONS.md`](./DECISIONS.md) for the rationale.

## Summary

| Component             | License        | Obligation                          |
| --------------------- | -------------- | ----------------------------------- |
| ES-Runtime            | Apache-2.0     | Standard Apache-2.0 NOTICE handling |
| V8 (engine)           | BSD-3-Clause   | Attribution; no relink obligation   |
| Rust dependencies     | Permissive     | Attribution (gated by cargo-deny)   |
