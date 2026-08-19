//! React Fast Refresh: the per-module half.
//!
//! # What this is, and what it is not
//!
//! Hot module replacement is [`crate::html::hot_runtime`]'s, and it knows
//! nothing about React: a module says `import.meta.hot.accept()` and gets
//! re-run. That is enough for a plain module and **not** enough for a React
//! component, because re-running a component module makes new function
//! identities, and React treats a new identity as a different component —
//! unmounting the old tree and taking every `useState` in it with it. The edit
//! lands and the counter you were testing is back to zero.
//!
//! Fast Refresh is React's answer: register every component under a stable id,
//! record the *shape* of its hooks, and on an update re-render in place while
//! the identities are matched up. Two things have to happen for that, and this
//! file is the second:
//!
//! 1. **The transform.** Every component gets a `$RefreshReg$` call and every
//!    hook-using function a `$RefreshSig$` signature. oxc implements it and
//!    rolldown exposes it, so esdev only has to ask ([`crate::bundler`]).
//! 2. **The per-module wrapper**, here. `$RefreshReg$` is a *global* the
//!    transform's output calls, and it has to mean "register under this
//!    module's id" while this module is evaluating and nothing after. So each
//!    module sets it, runs, and puts back what was there.
//!
//! # Why esdev does this rather than a plugin in the template
//!
//! Because the wrapper is per module, and a plugin that could inject one would
//! need the module graph and the transform pipeline — which is this binary. The
//! *React* half stays in the template, where React was chosen: the runtime
//! bootstrap that has to run before React itself loads is `src/refresh.ts`
//! there, not a string in here.
//!
//! What generalises is underneath: a framework with its own scheme writes its
//! own pass against the same [`crate::contract::Pass`] this one uses, and the
//! `import.meta.hot` API it builds on is the one every framework shares.

use std::sync::Arc;

use crate::contract::{self, Answer, Filter, HookSpec, Hooks, ModuleResult, Pattern};

/// Wraps each component module so `$RefreshReg$` means *this* module.
#[derive(Debug)]
pub struct ReactRefresh {
    hooks: Hooks,
}

impl Default for ReactRefresh {
    fn default() -> Self {
        Self::new()
    }
}

impl ReactRefresh {
    pub fn new() -> Self {
        Self {
            hooks: Hooks {
                transform: Some(HookSpec {
                    // JSX only. A `.ts` with no component in it would gain a
                    // prologue, an import of the refresh runtime and an
                    // `accept()` it never needed — and `accept()` is not
                    // harmless, it makes the module a hot boundary that
                    // silently swallows changes it cannot actually apply.
                    filter: Filter {
                        id: vec![Pattern::Regex(
                            regex::Regex::new(r"\.[jt]sx$").expect("a literal pattern"),
                        )],
                        code: Vec::new(),
                    },
                    ..HookSpec::default()
                }),
                ..Hooks::default()
            },
        }
    }
}

impl contract::Pass for ReactRefresh {
    fn name(&self) -> &str {
        "react-refresh"
    }

    fn hooks(&self) -> &Hooks {
        &self.hooks
    }

    fn transform<'a>(
        &'a self,
        code: &'a str,
        id: &'a str,
        _module_type: &'a str,
        _ctx: &'a Arc<dyn contract::Context>,
    ) -> Answer<'a, Option<ModuleResult>> {
        Box::pin(async move {
            Ok(Some(ModuleResult {
                code: wrap(code, id),
                // The body is still whatever it was — JSX, TypeScript, both —
                // and the prologue is plain JavaScript that any of those parse.
                // Saying nothing leaves the extension to decide, which is right.
                module_type: None,
                // A source map would have to be composed with the one the
                // transform after this makes, and the prologue is a fixed number
                // of lines at the top rather than an edit through the body — so
                // what a stack trace loses is an offset, not a file.
                map: None,
                depends_on: Vec::new(),
            }))
        })
    }
}

/// The wrapper, around one module's source.
///
/// # There is no epilogue, and that took some finding
///
/// The obvious shape is a prologue that points `$RefreshReg$` at this module and
/// an epilogue that puts back what was there. It does not work, because **the
/// transform appends its registrations after everything this adds**:
///
/// ```text
/// globalThis.$RefreshReg$ = …            ← the prologue
/// function Home() { … }                  ← the body
/// globalThis.$RefreshReg$ = __prev       ← the epilogue, restoring
/// $RefreshReg$(_c, "Home");              ← the transform's registration
/// ```
///
/// The registration lands *after* the restore, so every component registers
/// into whatever the global was before — nothing, in practice. React then has no
/// component families to match, and an edit re-runs the module, keeps the
/// state, and renders the old component: the symptom is Fast Refresh appearing
/// to do nothing at all, with no error anywhere.
///
/// So the assignment is left standing. Modules in a bundle evaluate in sequence,
/// and each one's prologue sets the globals again before its own body, so a
/// module's registrations always run while the globals still point at it.
///
/// # The refresh is an accept callback
///
/// For the same ordering reason `performReactRefresh()` cannot be a statement at
/// the end of the module — it would run before the registrations it depends on.
/// It is the `accept` callback instead, which the runtime calls *after* the
/// module has been re-run in full, which is the moment it is actually correct.
/// That makes Fast Refresh an ordinary consumer of the generic hot API rather
/// than something the runtime knows about.
fn wrap(code: &str, id: &str) -> String {
    format!(
        "import * as __esdev_refresh from \"react-refresh/runtime\";\n\
         globalThis.$RefreshReg$ = (type, id) => \
         __esdev_refresh.register(type, {module} + \" \" + id);\n\
         globalThis.$RefreshSig$ = __esdev_refresh.createSignatureFunctionForTransform;\n\
         import.meta.hot.accept(() => __esdev_refresh.performReactRefresh());\n\
         {code}\n",
        module = json_string(id),
        code = code,
    )
}

/// A string, as a JavaScript string literal.
///
/// Module ids are paths, and a path may hold a quote or a backslash on a
/// filesystem that allows one. Interpolating one raw would end the literal and
/// turn a filename into a syntax error in somebody's build.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The globals have to be pointed at this module before its body, and
    /// **left that way** -- the transform appends its registrations after
    /// everything here, so restoring them would send every registration to
    /// whatever was there before.
    #[test]
    fn the_prologue_precedes_the_body_and_nothing_restores_it() {
        let wrapped = wrap("export const A = () => null;", "src/app/Home.tsx");
        let reg = wrapped.find("$RefreshReg$ = (type").expect("a prologue");
        let body = wrapped.find("export const A").expect("the body");
        assert!(reg < body, "{wrapped}");
        assert!(
            !wrapped.contains("__esdev_prev"),
            "restoring the globals sends the transform's registrations nowhere:\n{wrapped}"
        );
        // The refresh runs as an accept callback, after the re-run completes,
        // rather than as a statement that would precede its own registrations.
        assert!(
            wrapped.contains("accept(() => __esdev_refresh.performReactRefresh())"),
            "{wrapped}"
        );
        // And the module registers under its own id.
        assert!(wrapped.contains("\"src/app/Home.tsx\""), "{wrapped}");
    }

    /// A module id is a path, and a path can hold the character that ends a
    /// JavaScript string.
    #[test]
    fn a_quote_in_a_path_cannot_break_out_of_the_literal() {
        assert_eq!(json_string(r#"src/a"b.tsx"#), r#""src/a\"b.tsx""#);
        assert_eq!(json_string(r"src/a\b.tsx"), r#""src/a\\b.tsx""#);
    }

    /// Only JSX. A plain `.ts` gaining `accept()` would become a hot boundary
    /// that swallows changes it cannot apply.
    #[test]
    fn only_component_modules_are_wrapped() {
        let pass = ReactRefresh::new();
        let filter = &pass.hooks.transform.as_ref().expect("declared").filter;
        assert!(filter.admits("src/app/Home.tsx", None));
        assert!(filter.admits("src/app/Home.jsx", None));
        assert!(!filter.admits("src/data/posts.ts", None));
        assert!(!filter.admits("src/routes.d.ts", None));
        assert!(!filter.admits("styles/app.css", None));
    }
}
