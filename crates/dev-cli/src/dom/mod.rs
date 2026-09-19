//! The DOM support that belongs exclusively to `esdev test`.
//!
//! This is intentionally separate from the build-time HTML rewriter: a DOM
//! needs a tree and predictable parse failures, while the rewriter needs only
//! token spans and must preserve an author's bytes exactly.

// The parser is deliberately introduced before its DOM decoder. Keeping it
// compiled and tested now makes its strict grammar a stable boundary for the
// tree phase that follows.
#[allow(dead_code, reason = "the DOM decoder is the next implementation phase")]
pub mod html;
