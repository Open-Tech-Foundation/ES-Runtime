//! A declaration bundler: a library's `.d.ts` files, linked into one.
//!
//! `--lib` emits a declaration beside every module, which is what a package
//! with subpath exports needs — `import "@you/pkg/pool"` has to find a real
//! `pool.d.ts`. A package whose `exports` map has one entry wants the opposite:
//! **one** `index.d.ts`, so the published tree is a file rather than a mirror of
//! a source layout nobody outside the package should have to know about.
//!
//! Neither `tsc` nor rolldown can do this. `tsc` emits one declaration per
//! source file and has no declaration-bundling mode at all (`--outFile` is a
//! legacy `module: none/amd/system` feature); rolldown's Rust crates have no
//! `.d.ts` support — the `rolldown-plugin-dts` its ecosystem uses is an npm
//! package, not a crate, and it works by running a *second* bundling pass over
//! declarations tsc already emitted. So this is that second pass, written here.
//!
//! # The pipeline
//!
//! ```text
//!  src/**.ts ──► declarations::declarations_for ──► one .d.ts per module
//!                                                        │
//!                        ┌───────────────────────────────┘
//!                        ▼
//!            analyze  ── decls + bindings + exports, as text and byte ranges
//!                        │
//!                        ▼
//!            resolve  ── ./foo.js → src/foo.ts   |   react → external
//!                        │
//!                        ▼
//!              graph  ── every module reachable from the entry
//!                        │
//!                        ▼
//!               link  ── public API → reachable symbols → output names
//!                        │
//!                        ▼
//!               emit  ── imports, declarations, one export block
//!                        │
//!                        ▼
//!                    index.d.ts
//! ```
//!
//! # Three decisions worth defending
//!
//! **Text, not AST.** A declaration travels as the bytes that produced it, with
//! the ranges where a module-scope name appears. JSDoc therefore survives byte
//! for byte, which matters more here than anywhere else in the toolchain: the
//! comments in a `.d.ts` are what an editor shows on hover. See
//! [`analyze`][mod@analyze].
//!
//! **The public API is the entry's exports, and nothing else is exported.** A
//! type only reachable *through* a public one is inlined without an `export`
//! modifier — it has to be present, or the public type is meaningless, but
//! exporting it would widen the package's surface beyond what its author wrote.
//!
//! **Anything ambiguous is refused, never guessed.** A `.d.ts` is believed:
//! nothing runs it, no test covers it, and a consumer's editor treats it as
//! fact. So a construct this cannot link — a namespace import, `export =`, a
//! module augmentation — stops the build and names itself, rather than being
//! dropped into output that looks fine.

pub mod analyze;
mod link;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use analyze::{Analysis, ExportSource};

/// The extensions a relative specifier may name, in resolution order.
///
/// A TypeScript specifier is written against the *emitted* file (`./pool.js`)
/// while the file on disk is `./pool.ts` — the rule `moduleResolution: bundler`
/// and `node16` both use. So the extension is replaced rather than appended, and
/// only then is a bare specifier tried.
const SOURCE_EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "mjs", "jsx"];

/// A module in the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleId(pub usize);

/// What a specifier turned out to name.
#[derive(Debug, Clone, Copy)]
enum Resolution {
    /// A module of this library, which is inlined.
    Internal(ModuleId),
    /// A package, which is left as an import for the consumer to resolve.
    External,
}

struct Module {
    /// How the file is named in errors.
    label: String,
    analysis: Analysis,
    /// Every specifier this module mentions, and what it resolved to.
    resolved: HashMap<String, Resolution>,
}

struct Graph {
    modules: Vec<Module>,
    /// Canonical source path → id, so a module imported twice is one node.
    by_path: HashMap<PathBuf, ModuleId>,
    /// The order modules were finished in — dependencies before dependents, so
    /// a declaration is emitted after everything it names.
    order: Vec<ModuleId>,
}

/// Links the declarations of the library rooted at `entry` into one `.d.ts`.
///
/// `declarations` maps a **source** path (`src/pool.ts`) to the text of the
/// declaration generated for it. Nothing here reads a `.d.ts` off disk: the
/// declarations were produced in memory moments ago, and going back through the
/// filesystem would mean resolving `./pool.js` against the output tree, where
/// the answer depends on a build that may not have run yet.
pub fn bundle(entry: &Path, declarations: &HashMap<PathBuf, String>) -> Result<String, String> {
    let mut graph = Graph {
        modules: Vec::new(),
        by_path: HashMap::new(),
        order: Vec::new(),
    };
    let entry_id = graph.visit(entry, declarations)?;
    link::link(&graph, entry_id)
}

impl Graph {
    /// Reads `path`, resolves what it names, and recurses. Returns its id.
    ///
    /// Cycle-safe by inserting the id *before* recursing: two modules that
    /// import each other is ordinary in a type graph (a `Node` whose parent is a
    /// `Tree` whose children are `Node`s), and it must not be a stack overflow.
    fn visit(
        &mut self,
        path: &Path,
        declarations: &HashMap<PathBuf, String>,
    ) -> Result<ModuleId, String> {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(id) = self.by_path.get(&canonical) {
            return Ok(*id);
        }

        let label = path.display().to_string();
        let source = declarations
            .get(&canonical)
            .or_else(|| declarations.get(path))
            .ok_or_else(|| {
                format!(
                    "{label} has no declarations to bundle.\n\n\
                     A .d.ts is derived from TypeScript annotations, and a .js module has \
                     none to derive from — so a library bundled into one declaration file \
                     cannot import one."
                )
            })?;
        let analysis = analyze::analyze(&label, source)?;

        let id = ModuleId(self.modules.len());
        self.modules.push(Module {
            label: label.clone(),
            analysis,
            resolved: HashMap::new(),
        });
        self.by_path.insert(canonical, id);

        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        for specifier in self.specifiers(id) {
            let resolution = match resolve(&dir, &specifier) {
                Some(target) => Resolution::Internal(self.visit(&target, declarations)?),
                None if specifier.starts_with('.') => {
                    return Err(format!(
                        "{label}: cannot resolve {specifier}.\n\n\
                         A relative specifier names a module of this library, and no source \
                         file matches it."
                    ));
                }
                // A bare specifier is a package. The same rule `--lib` uses for
                // JavaScript, so a dependency is a dependency in both halves of
                // what gets published.
                None => Resolution::External,
            };
            self.modules[id.0].resolved.insert(specifier, resolution);
        }
        self.order.push(id);
        Ok(id)
    }

    /// Every specifier a module mentions, deduplicated, in a stable order.
    fn specifiers(&self, id: ModuleId) -> Vec<String> {
        let analysis = &self.modules[id.0].analysis;
        let mut found: Vec<String> = Vec::new();
        let push = |specifier: &String, found: &mut Vec<String>| {
            if !found.contains(specifier) {
                found.push(specifier.clone());
            }
        };
        for import in &analysis.imports {
            push(&import.specifier, &mut found);
        }
        for export in &analysis.exports {
            if let ExportSource::Reexport { specifier, .. } = &export.from {
                push(specifier, &mut found);
            }
        }
        for specifier in &analysis.star_exports {
            push(specifier, &mut found);
        }
        found
    }

    fn module(&self, id: ModuleId) -> &Module {
        &self.modules[id.0]
    }

    /// What `specifier` resolves to from `module`, if the module mentions it.
    fn resolution(&self, module: ModuleId, specifier: &str) -> Option<Resolution> {
        self.modules[module.0].resolved.get(specifier).copied()
    }
}

/// The source file a relative specifier names, or `None` for a package.
///
/// Deliberately narrow: only `./` and `../` are followed. That is the same line
/// `--lib` draws between what it emits and what it leaves external, and drawing
/// it anywhere else would mean a library whose JavaScript keeps a dependency
/// while its types inline one.
fn resolve(dir: &Path, specifier: &str) -> Option<PathBuf> {
    if !specifier.starts_with('.') {
        return None;
    }
    let base = dir.join(specifier);
    // `./pool.js` → `./pool.ts`: the specifier names the emitted file.
    if let Some(stem) = base.extension().map(|_| base.with_extension("")) {
        for extension in SOURCE_EXTENSIONS {
            let candidate = stem.with_extension(extension);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    // `./pool` → `./pool.ts`, then `./pool/index.ts`.
    for extension in SOURCE_EXTENSIONS {
        let candidate = base.with_extension(extension);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    for extension in SOURCE_EXTENSIONS {
        let candidate = base.join("index").with_extension(extension);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("esdev_dts_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create fixture");
        dir
    }

    #[test]
    fn a_specifier_naming_the_emitted_file_resolves_to_the_source() {
        let dir = fixture("resolve");
        std::fs::write(dir.join("pool.ts"), "").expect("write");
        std::fs::create_dir_all(dir.join("nested")).expect("create");
        std::fs::write(dir.join("nested/index.ts"), "").expect("write");

        assert_eq!(resolve(&dir, "./pool.js"), Some(dir.join("pool.ts")));
        assert_eq!(resolve(&dir, "./pool"), Some(dir.join("pool.ts")));
        assert_eq!(resolve(&dir, "./nested"), Some(dir.join("nested/index.ts")));
        assert_eq!(resolve(&dir, "./missing.js"), None);
        // A package is not this library's, whatever is on disk.
        assert_eq!(resolve(&dir, "react"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
