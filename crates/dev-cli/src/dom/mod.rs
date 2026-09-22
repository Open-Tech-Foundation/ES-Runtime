//! The DOM support that belongs exclusively to `esdev test`.
//!
//! This is intentionally separate from the build-time HTML rewriter: a DOM
//! needs a tree and predictable parse failures, while the rewriter needs only
//! token spans and must preserve an author's bytes exactly.

pub mod html;
pub mod stylesheet;

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
    HostModule {
        specifier: "runtime:dom/select",
        source: include_str!("select.js"),
    },
    HostModule {
        specifier: "runtime:dom/css",
        source: include_str!("css.js"),
    },
    HostModule {
        specifier: "runtime:dom/elements",
        source: include_str!("elements.js"),
    },
    HostModule {
        specifier: "runtime:dom/range",
        source: include_str!("range.js"),
    },
    HostModule {
        specifier: "runtime:dom/sheets",
        source: include_str!("sheets.js"),
    },
    HostModule {
        specifier: "runtime:dom/colors",
        source: include_str!("colors.js"),
    },
    // Generated (`tsr build` in `crates/dev-cli/js`): `color()` from
    // `@opentf/std`, which is where the 148 CSS colour names and the hex/rgb/hsl
    // arithmetic live. It is reachable only from `runtime:dom/colors`, which is
    // reachable only from `esdev test --dom`.
    HostModule {
        specifier: "runtime:dom/std-color",
        source: include_str!("std-color.js"),
    },
    // Generated (`tsr gen:css-table`): which value kinds each CSS property
    // takes, and what a zero serializes as, read out of Chrome.
    HostModule {
        specifier: "runtime:dom/css-table",
        source: include_str!("css-table.js"),
    },
];

impl HostExtension for DomExtension {
    fn modules(&self) -> &[HostModule] {
        MODULES
    }

    fn ops(&self, _ctx: &ExtensionContext<'_>) -> Vec<OpDecl> {
        vec![
            OpDecl::sync("dom_parse_fragment", |args| {
                let source = args
                    .first()
                    .and_then(Value::as_str)
                    .ok_or_else(|| OpError::type_error("dom_parse_fragment expects HTML text"))?;
                // The context element decides the implied structure: a `<tr>`
                // parsed into a `<table>` opens a `<tbody>`, and the same markup
                // parsed anywhere else does not.
                let context = match args.get(1) {
                    None | Some(Value::Null) => None,
                    Some(Value::String(name)) => Some(name.as_str()),
                    Some(_) => {
                        return Err(OpError::type_error(
                            "dom_parse_fragment context must be a string or null",
                        ));
                    }
                };
                html::fragment_records(source, context).map_err(|error| {
                    OpError::new(
                        es_runtime_common::ExceptionClass::SyntaxError,
                        error.to_string(),
                    )
                })
            }),
            // A whole document rather than a fragment: `DOMParser` needs the
            // doctype, and a document parse is the only place a doctype is legal.
            // A stylesheet, for the cascade. CSS has no parse errors — only
            // rules a browser drops — so this op cannot fail.
            OpDecl::sync("dom_parse_stylesheet", |args| {
                let source = args
                    .first()
                    .and_then(Value::as_str)
                    .ok_or_else(|| OpError::type_error("dom_parse_stylesheet expects CSS text"))?;
                Ok(stylesheet::stylesheet_records(source))
            }),
            OpDecl::sync("dom_parse_document", |args| {
                let source = args
                    .first()
                    .and_then(Value::as_str)
                    .ok_or_else(|| OpError::type_error("dom_parse_document expects HTML text"))?;
                html::document_records(source).map_err(|error| {
                    OpError::new(
                        es_runtime_common::ExceptionClass::SyntaxError,
                        error.to_string(),
                    )
                })
            }),
        ]
    }
}
