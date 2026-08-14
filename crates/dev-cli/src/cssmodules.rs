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
//! `transform` sees every module's source before it is parsed. For anything
//! ending in `.module.css` this replaces the source with a JavaScript object
//! literal — the name mapping — so the rest of the graph sees an ordinary
//! module and the importing component gets `styles.button` with no runtime.
//!
//! The scoped CSS itself has nowhere to go in a JavaScript bundle, so it is
//! pushed into [`Collected`], which [`crate::html`] drains afterwards to write
//! one stylesheet and add the `<link>` that loads it.
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
pub struct Collected(Arc<Mutex<Vec<(PathBuf, String)>>>);

impl Collected {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every stylesheet collected, concatenated in module order.
    ///
    /// Order matters and is the module graph's: CSS resolves ties by source
    /// order, so two rules of equal specificity must land in the same sequence
    /// on every build or a component's appearance depends on the bundler's mood.
    pub fn stylesheet(&self) -> String {
        let collected = self.0.lock().expect("no panic while holding the lock");
        let mut sorted: Vec<&(PathBuf, String)> = collected.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        sorted
            .iter()
            .map(|(_, css)| css.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.0
            .lock()
            .expect("no panic while holding the lock")
            .is_empty()
    }

    fn push(&self, path: PathBuf, css: String) {
        self.0
            .lock()
            .expect("no panic while holding the lock")
            .push((path, css));
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
        if !is_css_module(args.id) {
            return Ok(None);
        }

        let path = Path::new(args.id);
        // Relative to the project root, so the hash is the same on every
        // machine that builds this commit.
        let ident = path
            .strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");

        let sheet = crate::css::parse::parse(args.code);
        let scoped = crate::css::modules::scope(sheet, &ident)
            .map_err(|e| anyhow::anyhow!("{}: {e}", ident))?;

        let css = if self.minify {
            crate::css::print::print_minified(&scoped.sheet)
        } else {
            crate::css::print::print(&scoped.sheet)
        };
        self.collected.push(path.to_path_buf(), css);

        Ok(Some(HookTransformOutput {
            code: Some(module_source(&scoped.names)),
            module_type: Some(ModuleType::Js),
            ..HookTransformOutput::default()
        }))
    }
}

/// The JavaScript a `.module.css` import resolves to.
///
/// A frozen object literal, and both halves are deliberate: a literal so the
/// bundler can see through it and tree-shake an unused class name away, and
/// frozen because a component assigning to `styles.button` is a bug that should
/// say so rather than silently affect every other component importing the same
/// file.
fn module_source(names: &std::collections::BTreeMap<String, String>) -> String {
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

    /// Stylesheets concatenate in a stable order, because CSS breaks ties by
    /// source order and a build must not decide that differently each time.
    #[test]
    fn collected_stylesheets_are_ordered_by_path() {
        let collected = Collected::new();
        collected.push(PathBuf::from("/p/b.module.css"), ".b{}".to_string());
        collected.push(PathBuf::from("/p/a.module.css"), ".a{}".to_string());
        assert_eq!(collected.stylesheet(), ".a{}\n.b{}");
    }
}
