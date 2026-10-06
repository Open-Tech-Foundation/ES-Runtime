//! `new URL("./worker.js", import.meta.url)` — a module named by URL, built as a
//! chunk of its own (DECISIONS D134); `new URL("./icon.svg", import.meta.url)` —
//! any other file named by URL, copied into the output beside it (D155).
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
//! **Any other file** — an image, a font, a data file — is emitted as an asset:
//! its bytes copied into the output under a content-hashed name, and the
//! expression rewritten to that file the same way. Its path is read the way a
//! URL is: literally, against the importing file's directory, with no
//! extension guessed and no alias applied — so `./data` names a file called
//! `data`, not `data.js`. The URL stays module-relative, as written, so it
//! resolves against wherever the chunk lands: a page's origin in the browser,
//! the bundle's directory on a server.
//!
//! **What it leaves alone:** a computed path (not knowable at build time), a
//! path with a query or fragment, a base other than `import.meta.url`, and a
//! path that does not resolve to a file. The last is left as written rather
//! than failed, because a file created at run time is a legitimate thing to
//! name.
//!
//! **Rejected: rolldown's `resolveNewUrlToAsset`.** It copies every such file
//! as an asset: unbundled, uncompiled, with its imports unresolved — right for
//! a PNG, wrong for every module this is about. Here the two are told apart by
//! extension, and only the non-modules are copied.

use std::path::Path;
use std::sync::{Arc, LazyLock};

use regex::Regex;

use crate::contract::{self, Answer, Filter, HookSpec, Hooks, ModuleResult, Pattern};

/// What this pass is called wherever a diagnostic names it.
pub const PASS_NAME: &str = "esdev:module-url";

/// `new URL("<./ or ../ relative path>", import.meta.url)`, with either quote.
/// A path carrying a query or a fragment is not matched: it names something
/// the file alone does not answer for.
static FILE_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"new\s+URL\s*\(\s*(["'])(\.{1,2}/[^"'\n?#]+)["']\s*,\s*import\.meta\.url\s*\)"#)
        .expect("a literal pattern")
});

/// The extensions that make a path a module, built as a chunk rather than
/// copied.
const MODULE_EXTENSIONS: &[&str] = &["js", "mjs", "cjs", "ts", "mts", "cts", "jsx", "tsx"];

/// What a matched path names.
#[derive(Debug, PartialEq, Eq)]
enum Kind {
    /// Built as a chunk of its own (D134).
    Module,
    /// Copied into the output as it is (D155).
    Asset,
}

fn kind(path: &str) -> Kind {
    let module = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| MODULE_EXTENSIONS.contains(&e));
    if module { Kind::Module } else { Kind::Asset }
}

/// The file an asset URL names: the path read literally against the
/// importing file's directory, as a URL is resolved — no extension guessed,
/// no alias applied. `None` for anything but a regular file, including a
/// virtual importer with no directory on disk.
fn asset_file(importer: &str, path: &str) -> Option<std::path::PathBuf> {
    let file = Path::new(importer).parent()?.join(path);
    file.is_file().then_some(file)
}

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

/// Every `new URL("<relative path>", import.meta.url)` in `code`: where it is,
/// and the path it names.
fn matches(code: &str) -> Vec<(std::ops::Range<usize>, String)> {
    FILE_URL
        .captures_iter(code)
        .filter_map(|c| {
            let whole = c.get(0)?;
            Some((whole.range(), c.get(2)?.as_str().to_string()))
        })
        .collect()
}

/// A byte offset as the editor counts them. A module over 4 GiB is refused
/// rather than edited at the wrong place.
fn offset(at: usize) -> Result<u32, String> {
    u32::try_from(at).map_err(|_| "a module this large cannot be rewritten".to_string())
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
            // Edited in place rather than rebuilt, so every byte that is not a
            // rewritten `new URL(…)` keeps its position in the map: a stack
            // frame in this module still names the line that was written.
            let mut out = string_wizard::MagicString::new(code);
            let mut changed = false;
            let mut depends_on = Vec::new();
            for (range, path) in found {
                let reference = match kind(&path) {
                    Kind::Module => {
                        let resolved = ctx.resolve(&path, Some(id), false).await?;
                        let Some(resolved) = resolved.filter(|r| !r.external) else {
                            continue;
                        };
                        ctx.emit(contract::Emit::Chunk {
                            id: resolved.id.clone(),
                            name: chunk_name(&resolved.id),
                            file_name: None,
                        })?
                    }
                    Kind::Asset => {
                        let Some(file) = asset_file(id, &path) else {
                            continue;
                        };
                        let bytes = std::fs::read(&file)
                            .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
                        // A change to the file is a change to the output: the
                        // dev loop rebuilds on it, and the hash moves.
                        depends_on.push(file.to_string_lossy().into_owned());
                        ctx.emit(contract::Emit::Asset {
                            name: file
                                .file_name()
                                .and_then(|n| n.to_str())
                                .map(str::to_string),
                            file_name: None,
                            source: contract::Source::Bytes(bytes),
                        })?
                    }
                };
                out.update(
                    offset(range.start)?,
                    offset(range.end)?,
                    format!("new URL(import.meta.ROLLUP_FILE_URL_{reference})"),
                )?;
                changed = true;
            }
            if !changed {
                return Ok(None);
            }
            let map = out.source_map(string_wizard::SourceMapOptions {
                source: id.into(),
                ..string_wizard::SourceMapOptions::default()
            });
            Ok(Some(ModuleResult {
                code: out.to_string(),
                module_type: None,
                map: Some(map.to_json_string()),
                depends_on,
                side_effects: None,
            }))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_every_relative_file_url_and_nothing_else() {
        let code = r#"
            new Worker(new URL("./worker.js", import.meta.url));
            configure({ module: new URL('../server/workers.ts', import.meta.url) });
            const logo = new URL("./logo.png", import.meta.url);
            const data = new URL("../data", import.meta.url);
            const other = new URL("./x.js", base);
            const computed = new URL(name, import.meta.url);
            const absolute = new URL("/x.js", import.meta.url);
            const remote = new URL("https://example.com/x.svg", import.meta.url);
            const query = new URL("./x.svg?v=1", import.meta.url);
            const fragment = new URL("./sprite.svg#icon", import.meta.url);
        "#;
        let paths: Vec<String> = matches(code).into_iter().map(|(_, p)| p).collect();
        assert_eq!(
            paths,
            [
                "./worker.js",
                "../server/workers.ts",
                "./logo.png",
                "../data"
            ]
        );
    }

    #[test]
    fn every_module_extension_is_a_module_and_the_rest_are_assets() {
        for ext in MODULE_EXTENSIONS {
            let code = format!(r#"new URL("./m.{ext}", import.meta.url)"#);
            assert_eq!(matches(&code).len(), 1, "{ext}");
            assert_eq!(kind(&format!("./m.{ext}")), Kind::Module, "{ext}");
        }
        for path in [
            "./a.svg", "./a.png", "./a.json", "./a.css", "./a.wasm", "./data",
        ] {
            assert_eq!(kind(path), Kind::Asset, "{path}");
        }
    }

    #[test]
    fn an_asset_path_is_read_literally_beside_its_importer() {
        let dir = std::env::temp_dir().join(format!("esdev-module-url-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/img")).expect("dirs");
        std::fs::write(dir.join("src/img/icon.svg"), "<svg/>").expect("icon");
        std::fs::write(dir.join("data.js"), "").expect("a module beside a bare name");
        let importer = dir.join("src/main.js");
        let importer = importer.to_str().expect("utf-8");
        assert_eq!(
            asset_file(importer, "./img/icon.svg"),
            Some(dir.join("src/./img/icon.svg"))
        );
        // No extension is guessed: `../data` is not `../data.js`.
        assert_eq!(asset_file(importer, "../data"), None);
        // A directory is not a file.
        assert_eq!(asset_file(importer, "./img"), None);
        assert_eq!(asset_file(importer, "./missing.svg"), None);
        // A virtual importer has no directory to read beside.
        assert_eq!(asset_file("\0virtual", "./img/icon.svg"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_chunk_is_named_after_its_module() {
        assert_eq!(
            chunk_name("/p/src/server/workers.js").as_deref(),
            Some("workers")
        );
    }
}
