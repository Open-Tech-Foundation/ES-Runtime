//! ES-Runtime's HTTP protocol cores (DECISIONS D217).
//!
//! Each protocol here is a **sans-I/O** state machine: it is handed bytes and
//! hands bytes back, and owns no socket, timer or task. That is what lets one
//! implementation serve both roles — the server in `default-providers` and the
//! client — and what lets the tests and fuzzers drive it without a runtime.
//! Tokio appears only in the drivers that feed these machines.
//!
//! - [`fields`]: the header model every version shares, with the one validator
//!   that decides what a field name and value may contain.
//! - [`h1`]: HTTP/1.1 (RFC 9112) — request parsing, body framing, encoding.
//!   HTTP/1.1 only: this is a modern-only runtime, and an HTTP/1.0 message is
//!   refused rather than half-supported.
//! - [`h2`]: HTTP/2 (RFC 9113) — frames, so far.
//! - [`hpack`]: HTTP/2's header compression (RFC 7541).
//! - [`date`]: the `Date` header's IMF-fixdate form.

pub mod date;
pub mod fields;
pub mod h1;
pub mod h2;
pub mod hpack;
