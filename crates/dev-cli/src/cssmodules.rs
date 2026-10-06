//! The bundler half of CSS Modules: making `import styles from "./x.module.css"`
//! mean something.
//!
//! [`crate::css::modules`] does the CSS half — rename the classes, report the
//! mapping. This is what puts that in front of the bundler, because a
//! stylesheet the JavaScript *imports* is not a file the document links; it is
//! a module in the graph, and only the bundler knows the graph.
//!
//! # One pass, one hook
//!
//! This is a [`Pass`](crate::contract::Pass) against this project's own plugin
//! contract — the same contract a plugin written in guest JavaScript
//! implements, and the same one adapted onto whatever bundler is underneath.
//! It declares a `transform`, acts on any module whose type is `css` — a
//! `.css` file, or what a plugin compiled to CSS (D149) — and replaces the
//! module's source:
//!
//! * **`*.module.css`** (or `*.module.scss` once compiled) becomes a JavaScript object literal — the name mapping
//!   — so the graph sees an ordinary module and the importing component gets
//!   `styles.button` with no runtime.
//! * **any other `.css`** becomes an empty module. Nothing is exported because
//!   there are no scoped names to export, but it still has to *be* a module:
//!   the import is what put the stylesheet in the build, and an edge with no
//!   exports is what stops tree-shaking deciding it was unused.
//!
//! The second case is why `import "some-package/dist/style.css"` works. A
//! third-party stylesheet **cannot** be a CSS Module: the library's own
//! JavaScript emits its class names as hardcoded strings, so scoping them would
//! rename half of a contract the library has with itself. The alternative — copy
//! the file out of `node_modules` and `<link>` it — goes stale on the next
//! upgrade, silently.
//!
//! The CSS itself has nowhere to go in a JavaScript bundle, so it is pushed
//! into [`Collected`], which [`crate::html`] drains afterwards to write one
//! stylesheet and add the `<link>` that loads it.
//!
//! # Why the CSS is collected rather than injected
//!
//! The alternative is what many bundlers do: emit a `<style>` element from
//! JavaScript at runtime. That costs a flash of unstyled content on every first
//! paint, it puts styling behind script execution, and it needs
//! `style-src 'unsafe-inline'` — which the template's Content-Security-Policy
//! deliberately does not grant. A `<link>` in the head is fetched in parallel
//! with the bundle and blocks rendering exactly as a stylesheet should.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::contract::{self, Answer, HookSpec, Hooks, ModuleResult};

/// The scoped CSS gathered during a bundler run, in the order the modules were
/// transformed.
///
/// Shared with the pass rather than returned from it, because a hook has
/// nowhere to return something the bundle does not contain.
#[derive(Debug, Default, Clone)]
pub struct Collected(Arc<Mutex<Vec<Sheet>>>);

/// One stylesheet a module imported, and the files it referenced.
#[derive(Debug)]
pub struct Sheet {
    /// Where it came from, for ordering.
    pub path: PathBuf,
    /// The CSS, with a placeholder at every local `url()`.
    pub code: String,
    /// The files those placeholders stand for. [`crate::html`] writes them and
    /// substitutes the real names — the same contract a `<link>`ed stylesheet
    /// has, because the problem is the same: the CSS moves to `assets/` and a
    /// relative `url()` moves with it.
    pub referenced: Vec<crate::css::Referenced>,
    /// The modules a `composes … from` in this one names. An application's
    /// stylesheet holds them all; a library ships one file per module, and
    /// imports these from it so its classes arrive with their rules.
    pub composes: Vec<PathBuf>,
}

impl Collected {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every stylesheet collected, in a stable order.
    ///
    /// Ordered by path rather than by whenever the bundler happened to reach
    /// it: CSS resolves ties by source order, so two rules of equal specificity
    /// must land in the same sequence on every build, or a component's
    /// appearance depends on the bundler's scheduling.
    pub fn take(&self) -> Vec<Sheet> {
        let mut collected =
            std::mem::take(&mut *self.0.lock().expect("no panic while holding the lock"));
        collected.sort_by(|a, b| a.path.cmp(&b.path));
        collected
    }

    /// Records a stylesheet, once.
    ///
    /// A module can arrive twice — imported by a component *and* named by
    /// another module's `composes` — and its rules must appear once. Emitting
    /// them twice is not merely wasteful: the second copy would win every tie
    /// against anything declared between them.
    fn push(&self, sheet: Sheet) {
        let mut collected = self.0.lock().expect("no panic while holding the lock");
        if collected.iter().any(|seen| seen.path == sheet.path) {
            return;
        }
        collected.push(sheet);
    }
}

/// The files read while turning one stylesheet into a module.
///
/// Shared because `composes … from` recurses into other modules, and every file
/// any of them read belongs to the module the chain started at — that is the
/// module whose rebuild has to be triggered.
#[derive(Clone, Debug, Default)]
struct Files(Arc<Mutex<Vec<PathBuf>>>);

impl Files {
    fn extend(&self, paths: Vec<PathBuf>) {
        self.0
            .lock()
            .expect("no panic while holding the lock")
            .extend(paths);
    }

    fn take(&self) -> Vec<String> {
        let mut files = self.0.lock().expect("no panic while holding the lock");
        let mut out: Vec<String> = files
            .drain(..)
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// The ids of the plain stylesheets a `--lib` build reduced to empty modules.
#[derive(Debug, Default, Clone)]
pub struct Stubs(Arc<std::sync::Mutex<std::collections::HashSet<String>>>);

impl Stubs {
    fn record(&self, id: &str) {
        if let Ok(mut ids) = self.0.lock() {
            ids.insert(crate::adapter::guest_id(id).to_string());
        }
    }

    /// Whether `id` is one of them.
    pub fn contains(&self, id: &str) -> bool {
        self.0.lock().is_ok_and(|ids| ids.contains(id))
    }
}

/// Whether a module id names a CSS Modules stylesheet.
///
/// The `.module.css` convention rather than a config key: it is what every
/// other tool uses, it is visible at the import site, and it lets a project mix
/// scoped and global stylesheets without a list somewhere else saying which is
/// which.
///
/// Any extension, so `Button.module.scss` is scoped once a plugin has compiled
/// it to CSS (D149): the name says it is a module, the type says it is CSS.
pub fn is_css_module(id: &str) -> bool {
    std::path::Path::new(id)
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.split_once(".module."))
        .is_some_and(|(stem, extension)| !stem.is_empty() && !extension.is_empty())
}

/// The pass that turns a `.module.css` import into its name mapping.
///
/// **Written against [this project's own contract](crate::contract), not the
/// bundler's trait.** It is a [`Pass`](contract::Pass) like a plugin declared in
/// guest JavaScript is a `Pass`, goes into the same list in the same order,
/// declares its filter the same declarative way, and reaches the build through
/// the same context. Which means the claim that the bundler is replaceable
/// covers our own passes too: they move with the adapter, not with rolldown.
#[derive(Debug)]
pub struct CssModules {
    /// The project root, so a scoped name depends on a path that is the same on
    /// every machine.
    root: PathBuf,
    collected: Collected,
    minify: bool,
    /// A `--lib` build, and where it records the stylesheets it stubbed: see
    /// [`CssModules::for_library`].
    library: Option<Stubs>,
    hooks: Hooks,
}

impl CssModules {
    pub fn new(root: &Path, collected: Collected, minify: bool) -> Self {
        CssModules {
            root: root.to_path_buf(),
            collected,
            minify,
            library: None,
            // No filter on the id: a module is a stylesheet because its type
            // is `css`, which is what a plugin compiling `.scss` says it made
            // (D149), and the type is only known when the hook is called. The
            // call costs nothing across a boundary; this pass is in this
            // binary.
            //
            // `post`, so it finishes what the plugins made: a stylesheet
            // language compiled to CSS, or CSS a plugin rewrote, reaches it
            // whatever order the plugin declared.
            hooks: Hooks {
                transform: Some(HookSpec {
                    order: contract::Order::Post,
                    ..HookSpec::default()
                }),
                ..Hooks::default()
            },
        }
    }

    /// The pass a `--lib` build installs. A plain stylesheet's import is
    /// dropped from the output: the build copies every `.css` beside the
    /// modules, and a published module importing `./button.css` would fail
    /// anywhere that is not a CSS-aware bundler — Node, a server render,
    /// esrun. What would be left in its place is an empty module of its own,
    /// so the stub is marked side-effect free and the bundler removes it with
    /// the import. A consumer imports the copied stylesheet through the
    /// package's `exports`.
    ///
    /// Tree-shaking is off in a library build, so the bundler still writes the
    /// stub as a file of its own; its id goes into `stubs` for the build to
    /// remove that file once it is written.
    pub fn for_library(mut self, stubs: Stubs) -> Self {
        self.library = Some(stubs);
        self
    }
}

impl contract::Pass for CssModules {
    fn name(&self) -> &str {
        "esdev:css-modules"
    }

    fn hooks(&self) -> &Hooks {
        &self.hooks
    }

    fn transform<'a>(
        &'a self,
        code: &'a str,
        id: &'a str,
        module_type: &'a str,
        _ctx: &'a Arc<dyn contract::Context>,
    ) -> Answer<'a, Option<ModuleResult>> {
        Box::pin(async move {
            // A stylesheet is what its type says. A plugin ordered `pre` — a
            // Tailwind compiler, a CSS-in-JS pass — may have turned a `.css`
            // into JavaScript already, and working on it anyway would undo the
            // plugin's whole reason for claiming it; a plugin compiling `.scss`
            // says it made `css`, and that is this pass's to finish (D149).
            //
            // This is what makes `order: "pre"` mean something against a
            // built-in pass rather than only against the other plugins.
            if module_type != "css" {
                return Ok(None);
            }
            let path = Path::new(id);
            let read = Files::default();
            let names = self
                .stylesheet(path, code, &read)
                .map_err(|e| format!("{}: {e}", self.ident(path)))?;
            let plain = names.is_none();

            Ok(Some(ModuleResult {
                code: match names {
                    // A CSS Module hands its mapping to the importer.
                    Some(names) => module_source(&names),
                    // A plain stylesheet exports nothing. It still has to be a
                    // module: the import is what put the CSS in the build, so
                    // an empty one keeps that edge in the graph rather than
                    // letting tree-shaking decide the stylesheet was unused.
                    None => "export {};\n".to_string(),
                },
                module_type: Some("js".to_string()),
                // Generated, so nothing in it came from a line of the
                // stylesheet. An empty map says exactly that. No map at all
                // would tell the bundler the chain is broken, and it would warn
                // on every build that asked for source maps.
                map: Some(rolldown_sourcemap::empty_sourcemap().to_json_string()),
                // Every stylesheet this read that the graph cannot see: the
                // files an `@import` chain pulled in, and the modules a
                // `composes … from` reached. Nothing imports either, so without
                // this a save to one of them rebuilds nothing and the page
                // keeps the rules it had.
                depends_on: read.take(),
                side_effects: match &self.library {
                    Some(stubs) if plain => {
                        stubs.record(id);
                        Some(false)
                    }
                    _ => None,
                },
            }))
        })
    }
}

impl CssModules {
    /// A file's identity for hashing and for error messages: its path relative
    /// to the project root, so the result is the same on every machine.
    fn ident(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Bundles one stylesheet and, if it is a module, scopes it.
    ///
    /// From the code the bundler handed over, which is what a plugin before
    /// this one made of the file; its `@import`s are then followed from the
    /// file's own directory by [`crate::css::bundle`], because the bundler
    /// hands over one file's text and knows nothing about the rest.
    fn stylesheet(
        &self,
        path: &Path,
        code: &str,
        read: &Files,
    ) -> Result<Option<BTreeMap<String, String>>, String> {
        let bundled = crate::css::load_source(path, Some(code.to_string()))?;
        // For the dev loop: a save to one of these is a stylesheet swap,
        // whatever its extension.
        crate::build::record_stylesheet_files(
            std::iter::once(path.to_path_buf()).chain(bundled.read_files.iter().cloned()),
        );
        read.extend(bundled.read_files);

        let mut composes = Vec::new();
        let (sheet, names) = if is_css_module(&path.to_string_lossy()) {
            let mut imports = Imports {
                from: path.parent().unwrap_or(Path::new(".")).to_path_buf(),
                root: self.root.clone(),
                collected: self.collected.clone(),
                minify: self.minify,
                seen: vec![path.to_path_buf()],
                read: read.clone(),
                composes: Vec::new(),
            };
            let scoped =
                crate::css::modules::scope_with(bundled.sheet, &self.ident(path), &mut imports)?;
            composes = imports.composes;
            (scoped.sheet, Some(scoped.names))
        } else {
            (bundled.sheet, None)
        };

        let code = if self.minify {
            crate::css::print::print_minified(&sheet)
        } else {
            crate::css::print::print(&sheet)
        };
        self.collected.push(Sheet {
            path: path.to_path_buf(),
            code,
            referenced: bundled.referenced,
            composes,
        });
        Ok(names)
    }
}

/// Resolves `composes … from "./other.module.css"` by scoping that module too.
///
/// `seen` is the chain of modules currently being scoped, and is what makes a
/// composition cycle terminate. Two modules composing each other has no finite
/// answer, and a build that recursed until the stack went would report it as a
/// crash rather than as the mistake it is.
struct Imports {
    from: PathBuf,
    root: PathBuf,
    collected: Collected,
    minify: bool,
    seen: Vec<PathBuf>,
    /// Everything read on the way, so the module that started it can say what
    /// it depends on. A composed module is reachable from **no import**, so
    /// nothing else in the build has heard of it.
    read: Files,
    /// The modules this one composes from, directly.
    composes: Vec<PathBuf>,
}

impl crate::css::modules::Resolve for Imports {
    fn names(&mut self, specifier: &str) -> Result<BTreeMap<String, String>, String> {
        let path = self.from.join(specifier);
        let path = path.canonicalize().unwrap_or(path);
        if self.seen.contains(&path) {
            return Err(format!(
                "`composes` cycles through {}.\n\n                 A class composing one that composes it back has no answer.",
                specifier
            ));
        }
        if !path.is_file() {
            return Err(format!("`composes … from {specifier}` names no file."));
        }

        let ident = path
            .strip_prefix(&self.root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");

        // The composed module's CSS is collected as well as read. Nothing else
        // will: `composes` is the only thing referring to it, so without this
        // the class names it hands out would name rules that are not in the
        // output. `Collected::push` dedupes, so a module that is also imported
        // still appears once.
        let bundled = crate::css::load(&path)?;
        self.read.extend(bundled.read_files);
        let mut deeper = Imports {
            from: path.parent().unwrap_or(Path::new(".")).to_path_buf(),
            root: self.root.clone(),
            collected: self.collected.clone(),
            minify: self.minify,
            seen: {
                let mut seen = self.seen.clone();
                seen.push(path.clone());
                seen
            },
            read: self.read.clone(),
            composes: Vec::new(),
        };
        let scoped = crate::css::modules::scope_with(bundled.sheet, &ident, &mut deeper)?;
        self.collected.push(Sheet {
            path: path.clone(),
            code: if self.minify {
                crate::css::print::print_minified(&scoped.sheet)
            } else {
                crate::css::print::print(&scoped.sheet)
            },
            referenced: bundled.referenced,
            composes: deeper.composes,
        });
        if !self.composes.contains(&path) {
            self.composes.push(path);
        }
        Ok(scoped.names)
    }
}

/// The JavaScript a `.module.css` import resolves to.
///
/// A frozen object literal, and both halves are deliberate: a literal so the
/// bundler can see through it and tree-shake an unused class name away, and
/// frozen because a component assigning to `styles.button` is a bug that should
/// say so rather than silently affect every other component importing the same
/// file.
fn module_source(names: &BTreeMap<String, String>) -> String {
    let mut out = String::from("export default Object.freeze({");
    for (local, scoped) in names {
        out.push_str(&format!("{}:{},", quote(local), quote(scoped)));
    }
    out.push_str("});\n");
    out
}

/// A JavaScript string literal. Class names come from a file on disk, so `"`,
/// `\` and a line separator all have to survive being written into source.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `*.module.*`, whatever the language: the name says it is a module, and
    /// the type — `css`, once a plugin has compiled it — says it is a
    /// stylesheet (D149).
    #[test]
    fn the_convention_is_the_filename() {
        assert!(is_css_module("/p/Button.module.css"));
        assert!(is_css_module("/p/Button.module.scss"));
        assert!(!is_css_module("/p/styles.css"));
        assert!(!is_css_module("/p/module.css"));
        assert!(!is_css_module("/p/.module.css"));
        assert!(!is_css_module("/p/a.module/styles.css"));
    }

    #[test]
    fn the_generated_module_is_a_frozen_literal() {
        let names = [("button".to_string(), "button_a1b2c3d4".to_string())]
            .into_iter()
            .collect();
        let source = module_source(&names);
        assert_eq!(
            source,
            "export default Object.freeze({\"button\":\"button_a1b2c3d4\",});\n"
        );
    }

    /// A class name comes from a file somebody else wrote, and it lands in
    /// JavaScript source. It cannot be allowed to end the string it is in.
    #[test]
    fn a_name_cannot_break_out_of_the_literal_it_is_in() {
        let names = [("a\";globalThis.owned=1;\"".to_string(), "b".to_string())]
            .into_iter()
            .collect();
        let source = module_source(&names);
        assert!(!source.contains("globalThis.owned=1;\""), "{source}");
        assert!(source.contains("\\\""), "{source}");
    }

    /// Stylesheets come out in a stable order, because CSS breaks ties by
    /// source order and a build must not decide that differently each time.
    #[test]
    fn collected_stylesheets_are_ordered_by_path() {
        let collected = Collected::new();
        for name in ["b", "a"] {
            collected.push(Sheet {
                path: PathBuf::from(format!("/p/{name}.module.css")),
                code: format!(".{name}{{}}"),
                referenced: Vec::new(),
                composes: Vec::new(),
            });
        }
        let codes: Vec<String> = collected.take().into_iter().map(|s| s.code).collect();
        assert_eq!(codes, [".a{}", ".b{}"]);
        // …and taking drains, so a second build does not inherit the first's.
        // The dev loop depends on this: it holds one bundler, and so one
        // plugin, and so one collector, across every rebuild.
        assert!(collected.take().is_empty());
    }

    /// A plain `.css` is a module too, or tree-shaking decides the stylesheet
    /// nothing imported a name from was unused.
    #[test]
    fn a_plain_stylesheet_is_told_apart_from_a_module() {
        assert!(!is_css_module("/p/vendor/style.css"));
        assert!(is_css_module("/p/Button.module.css"));
    }
}
