//! The bundler half of CSS Modules: making `import styles from "./x.module.css"`
//! mean something.
//!
//! [`crate::css::modules`] does the CSS half — rename the classes, report the
//! mapping. This is what puts that in front of the bundler, because a
//! stylesheet the JavaScript *imports* is not a file the document links; it is
//! a module in the graph, and only the bundler knows the graph.
//!
//! # One rolldown plugin, one hook
//!
//! `transform` sees every module's source before it is parsed, and this
//! replaces it for anything ending in `.css`:
//!
//! * **`*.module.css`** becomes a JavaScript object literal — the name mapping
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

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rolldown::plugin::{
    HookTransformArgs, HookTransformOutput, HookTransformReturn, HookUsage, Plugin,
    SharedTransformPluginContext,
};
use rolldown_common::ModuleType;

/// The scoped CSS gathered during a bundler run, in the order the modules were
/// transformed.
///
/// Shared with the plugin rather than returned from it, because a rolldown hook
/// has nowhere to return something the bundle does not contain.
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

    pub fn is_empty(&self) -> bool {
        self.0
            .lock()
            .expect("no panic while holding the lock")
            .is_empty()
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

/// Whether a module id names a CSS Modules stylesheet.
///
/// The `.module.css` convention rather than a config key: it is what every
/// other tool uses, it is visible at the import site, and it lets a project mix
/// scoped and global stylesheets without a list somewhere else saying which is
/// which.
pub fn is_css_module(id: &str) -> bool {
    id.ends_with(".module.css")
}

/// The plugin that turns a `.module.css` import into its name mapping.
#[derive(Debug)]
pub struct CssModules {
    /// The project root, so a scoped name depends on a path that is the same on
    /// every machine.
    root: PathBuf,
    collected: Collected,
    minify: bool,
}

impl CssModules {
    pub fn new(root: &Path, collected: Collected, minify: bool) -> Self {
        CssModules {
            root: root.to_path_buf(),
            collected,
            minify,
        }
    }
}

impl Plugin for CssModules {
    fn name(&self) -> Cow<'static, str> {
        Cow::Borrowed("esdev:css-modules")
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::Transform
    }

    async fn transform(
        &self,
        _context: SharedTransformPluginContext,
        args: &HookTransformArgs<'_>,
    ) -> HookTransformReturn {
        if !args.id.ends_with(".css") {
            return Ok(None);
        }
        let path = Path::new(args.id);
        let names = self
            .stylesheet(path)
            .map_err(|e| anyhow::anyhow!("{}: {e}", self.ident(path)))?;

        Ok(Some(HookTransformOutput {
            code: Some(match names {
                // A CSS Module hands its mapping to the importer.
                Some(names) => module_source(&names),
                // A plain stylesheet exports nothing. It still has to be a
                // module: the import is what put the CSS in the build, so an
                // empty one keeps that edge in the graph rather than letting
                // tree-shaking decide the stylesheet was unused.
                None => "export {};\n".to_string(),
            }),
            module_type: Some(ModuleType::Js),
            ..HookTransformOutput::default()
        }))
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
    /// Read from disk rather than from `args.code`, because a stylesheet may
    /// `@import` others and only [`crate::css::bundle`] resolves those — the
    /// bundler hands over one file's text and knows nothing about the rest.
    fn stylesheet(&self, path: &Path) -> Result<Option<BTreeMap<String, String>>, String> {
        let bundled = crate::css::bundle::bundle(path)?;

        let (sheet, names) = if is_css_module(&path.to_string_lossy()) {
            let mut imports = Imports {
                from: path.parent().unwrap_or(Path::new(".")).to_path_buf(),
                root: self.root.clone(),
                collected: self.collected.clone(),
                minify: self.minify,
                seen: vec![path.to_path_buf()],
            };
            let scoped =
                crate::css::modules::scope_with(bundled.sheet, &self.ident(path), &mut imports)?;
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
        let bundled = crate::css::bundle::bundle(&path)?;
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
        });
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

    #[test]
    fn the_convention_is_the_filename() {
        assert!(is_css_module("/p/Button.module.css"));
        assert!(!is_css_module("/p/styles.css"));
        assert!(!is_css_module("/p/module.css"));
        assert!(!is_css_module("/p/Button.module.scss"));
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
            });
        }
        let codes: Vec<String> = collected.take().into_iter().map(|s| s.code).collect();
        assert_eq!(codes, [".a{}", ".b{}"]);
        // …and taking drains, so a second build does not inherit the first's.
        assert!(collected.is_empty());
    }

    /// A plain `.css` is a module too, or tree-shaking decides the stylesheet
    /// nothing imported a name from was unused.
    #[test]
    fn a_plain_stylesheet_is_told_apart_from_a_module() {
        assert!(!is_css_module("/p/vendor/style.css"));
        assert!(is_css_module("/p/Button.module.css"));
    }
}
