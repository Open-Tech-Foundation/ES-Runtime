//! The DOM support that belongs exclusively to `esdev test`.
//!
//! This is intentionally separate from the build-time HTML rewriter: a DOM
//! needs a tree and predictable parse failures, while the rewriter needs only
//! token spans and must preserve an author's bytes exactly.

pub mod html;

use es_runtime_cli_common::{ExtensionContext, HostExtension, HostModule, OpDecl, OpError, Value};

/// The private modules that make up the test runner's DOM preload.  They are
/// an extension rather than snapshot globals: an ordinary `esdev` run must not
/// accidentally acquire a browser-shaped environment.
pub struct DomExtension;

const MODULES: &[HostModule] = &[
    HostModule {
        specifier: "runtime:dom",
        source: include_str!("window.js"),
    },
    HostModule {
        specifier: "runtime:dom/events",
        source: include_str!("events.js"),
    },
    HostModule {
        specifier: "runtime:dom/tree",
        source: include_str!("tree.js"),
    },
    HostModule {
        specifier: "runtime:dom/parse",
        source: include_str!("parse.js"),
    },
];

impl HostExtension for DomExtension {
    fn modules(&self) -> &[HostModule] {
        MODULES
    }

    fn ops(&self, _ctx: &ExtensionContext<'_>) -> Vec<OpDecl> {
        vec![OpDecl::sync("dom_parse_fragment", |args| {
            let source = args
                .first()
                .and_then(Value::as_str)
                .ok_or_else(|| OpError::type_error("dom_parse_fragment expects HTML text"))?;
            // Context-sensitive parsing is deliberately not widened until the
            // element layer defines the supported content models.  Receiving
            // it now keeps the JS/Rust bridge stable when that lands.
            if args.len() > 1 && !matches!(args[1], Value::Null | Value::String(_)) {
                return Err(OpError::type_error(
                    "dom_parse_fragment context must be a string or null",
                ));
            }
            html::fragment_records(source).map_err(|error| {
                OpError::new(
                    es_runtime_common::ExceptionClass::SyntaxError,
                    error.to_string(),
                )
            })
        })]
    }
}
