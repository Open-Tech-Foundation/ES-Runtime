//! `new URL("./worker.js", import.meta.url)` — a module named by URL, built as a
//! chunk of its own (DECISIONS D134).
//!
//! The web's way to point at a module beside the current one without importing
//! it: `new Worker(new URL("./worker.js", import.meta.url))`, and
//! `runtime:workers`' `configure({ module: new URL("./workers.js", …) })` for the
//! module a durable worker's shards import. Left alone, a bundle keeps the
//! expression and emits no `worker.js`, so the built program asks at run time
//! for a file that is not in its output.
//!
//! This pass finds each such expression whose path names a JavaScript or
//! TypeScript module, resolves it as an import would be, adds that module to the
//! build as an extra entry — bundled, compiled, its own imports followed — and
//! rewrites the expression to the emitted file, relative to the chunk that ends
//! up containing it. `import.meta.ROLLUP_FILE_URL_<ref>` is the bundler's own
//! placeholder for exactly that path.
//!
//! **What it leaves alone:** a computed path (not knowable at build time), a
//! path to anything but a module (an image or a data file is the asset pass's),
//! a base other than `import.meta.url`, and a path that does not resolve. The
//! last is left as written rather than failed, because a file created at run
//! time is a legitimate thing to name.
//!
//! **Rejected: rolldown's `resolveNewUrlToAsset`.** It copies the file as an
//! asset: unbundled, uncompiled, with its imports unresolved — right for a PNG,
//! wrong for every module this is about.

use std::path::Path;
use std::sync::{Arc, LazyLock};

use regex::Regex;

use crate::contract::{self, Answer, Filter, HookSpec, Hooks, ModuleResult, Pattern};

/// What this pass is called wherever a diagnostic names it.
pub const PASS_NAME: &str = "esdev:module-url";

/// `new URL("<./ or ../ relative module path>", import.meta.url)`, with either
/// quote. The path must end in a module extension; anything else is not ours.
static MODULE_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"new\s+URL\s*\(\s*(["'])(\.{1,2}/[^"'\n]+?\.(?:[cm]?[jt]s|[jt]sx))["']\s*,\s*import\.meta\.url\s*\)"#,
    )
    .expect("a literal pattern")
});

#[derive(Debug)]
pub struct ModuleUrl {
    hooks: Hooks,
}

impl ModuleUrl {
    pub fn new() -> Self {
        ModuleUrl {
            // Only modules that mention `import.meta.url` are handed to the
            // hook, which is almost none of them.
            hooks: Hooks {
                transform: Some(HookSpec {
                    filter: Filter {
                        id: Vec::new(),
                        code: vec![Pattern::Regex(
                            Regex::new(r"import\.meta\.url").expect("a literal pattern"),
                        )],
                    },
                    ..HookSpec::default()
                }),
                ..Hooks::default()
            },
        }
    }
}

/// Every `new URL("<module>", import.meta.url)` in `code`: where it is, and the
/// path it names.
fn matches(code: &str) -> Vec<(std::ops::Range<usize>, String)> {
    MODULE_URL
        .captures_iter(code)
        .filter_map(|c| {
            let whole = c.get(0)?;
            Some((whole.range(), c.get(2)?.as_str().to_string()))
        })
        .collect()
}

/// The name an emitted chunk is given: the module's file stem, which the
/// output's `[name]` pattern turns into its file name.
fn chunk_name(id: &str) -> Option<String> {
    Path::new(id)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
}

impl contract::Pass for ModuleUrl {
    fn name(&self) -> &str {
        PASS_NAME
    }

    fn hooks(&self) -> &Hooks {
        &self.hooks
    }

    fn transform<'a>(
        &'a self,
        code: &'a str,
        id: &'a str,
        module_type: &'a str,
        ctx: &'a Arc<dyn contract::Context>,
    ) -> Answer<'a, Option<ModuleResult>> {
        Box::pin(async move {
            // Not JavaScript any more — a stylesheet or asset some pass turned
            // into something else is not ours to read as source.
            if !matches!(module_type, "js" | "jsx" | "ts" | "tsx") {
                return Ok(None);
            }
            let found = matches(code);
            if found.is_empty() {
                return Ok(None);
            }
            let mut out = String::with_capacity(code.len());
            let mut last = 0;
            let mut changed = false;
            for (range, path) in found {
                let resolved = ctx.resolve(&path, Some(id), false).await?;
                let Some(resolved) = resolved.filter(|r| !r.external) else {
                    continue;
                };
                let reference = ctx.emit(contract::Emit::Chunk {
                    id: resolved.id.clone(),
                    name: chunk_name(&resolved.id),
                    file_name: None,
                })?;
                out.push_str(&code[last..range.start]);
                out.push_str(&format!("new URL(import.meta.ROLLUP_FILE_URL_{reference})"));
                last = range.end;
                changed = true;
            }
            if !changed {
                return Ok(None);
            }
            out.push_str(&code[last..]);
            Ok(Some(ModuleResult {
                code: out,
                module_type: None,
                map: None,
                depends_on: Vec::new(),
            }))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_module_url_and_nothing_else() {
        let code = r#"
            new Worker(new URL("./worker.js", import.meta.url));
            configure({ module: new URL('../server/workers.ts', import.meta.url) });
            const logo = new URL("./logo.png", import.meta.url);
            const other = new URL("./x.js", base);
            const computed = new URL(name, import.meta.url);
            const absolute = new URL("/x.js", import.meta.url);
        "#;
        let paths: Vec<String> = matches(code).into_iter().map(|(_, p)| p).collect();
        assert_eq!(paths, ["./worker.js", "../server/workers.ts"]);
    }

    #[test]
    fn takes_every_module_extension() {
        for ext in ["js", "mjs", "cjs", "ts", "mts", "cts", "jsx", "tsx"] {
            let code = format!(r#"new URL("./m.{ext}", import.meta.url)"#);
            assert_eq!(matches(&code).len(), 1, "{ext}");
        }
    }

    #[test]
    fn a_chunk_is_named_after_its_module() {
        assert_eq!(
            chunk_name("/p/src/server/workers.js").as_deref(),
            Some("workers")
        );
    }
}
