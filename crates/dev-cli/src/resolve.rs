//! What a build asserts about where its output will run.
//!
//! Module resolution is not a detail a build system gets to leave at its
//! dependency's defaults. A package with an `exports` map offers several builds
//! of itself and picks between them on the *conditions* the bundler asserts, so
//! the set of conditions is a claim about the destination — and a build that
//! asserts nothing gets whichever build the package author put last, which for
//! most of the registry is the one written for Node.
//!
//! That failure is silent. Nothing is missing, nothing is unresolved, the
//! bundle is produced; it dies later on an `import` of `node:stream` that this
//! runtime does not have. So the defaults live here rather than at each call
//! site, and every path that builds — the `build` subcommand, the client bundle
//! it emits for a browser, and `runtime:build` in guest JS — reads them from
//! one place.
//!
//! `runtime:build` did not, once, and the divergence was exactly this: a guest
//! bundling a server entry got no `worker` and no `main` fields, so the same
//! project built one way through `esdev build` and another way through the
//! module it is supposed to be the same bundler as.

/// Where a build's output is meant to run.
///
/// Deliberately not the bundler's own platform enum: this is what *we* assert,
/// and it survives changing what is underneath. The two map onto each other at
/// the two call sites that speak to a bundler, and nowhere else.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Target {
    /// This runtime — neither a browser nor Node, which is the whole reason a
    /// condition has to be asserted rather than implied by a platform. The
    /// default, because a build run here is a build for here unless it says
    /// otherwise.
    #[default]
    Server,
    /// A browser, where a `document` and a `window` exist.
    Browser,
    /// Node, whose conditions and main fields the bundler already knows. Only
    /// `runtime:build` can ask for this, and only a caller bundling something
    /// to run somewhere else would.
    Node,
    /// An input to somebody else's build. A library asserts **no** condition:
    /// `worker` decides which build of a dependency is inlined, a library
    /// inlines none of them, and baking one in publishes a package that has
    /// already chosen for its consumer (D59).
    Library,
}

/// The condition a Web-API-targeting package uses for the build that does not
/// reach for `node:` modules.
///
/// `import` and `default` are the bundler's own and always present. `worker` is
/// ours, and it is the one that matters: React's `react-dom/server` resolves to
/// its Web Streams implementation under it, and to a `node:stream` one without.
///
/// This is deliberately *not* the runtime's condition set. D40 keeps that
/// standards-only (`import`/`default`) and that stays true; a condition changes
/// which code runs, so the place to choose one is a build the developer ran on
/// purpose, not a server resolving imports under load.
pub const WORKER_CONDITIONS: &[&str] = &["worker"];

/// The conditions a **browser** target asserts instead.
///
/// The other half of the same story: `browser` is the key a package uses for
/// the build that expects a `document` and a `window`. A client bundle built
/// with `worker` asserted gets the one that expects neither, and the failure is
/// at runtime in someone's browser rather than here.
///
/// The two are alternatives, not additions: a package that offers both means
/// them for different places, and conditions match in the order the *package
/// author* wrote them (D40), so the wrong one being present at all is enough to
/// win.
pub const BROWSER_CONDITIONS: &[&str] = &["browser"];

/// The `package.json` fields to fall back on when a package has no `exports`
/// map, ESM first.
///
/// A neutral platform leaves these empty, which breaks any package old enough
/// to predate `exports` — and a good deal of the registry is. Taking `main`
/// after `module` is fine, because converting CommonJS is the point.
pub const MAIN_FIELDS: &[&str] = &["module", "main"];

/// The conditions a target asserts, with the caller's own appended.
///
/// The caller's come last so that a project can add to what we assert without
/// having to restate it, and cannot silently lose it by naming one condition of
/// its own.
pub fn conditions(target: Target, extra: impl IntoIterator<Item = String>) -> Vec<String> {
    let base: &[&str] = match target {
        Target::Server => WORKER_CONDITIONS,
        Target::Browser => BROWSER_CONDITIONS,
        // The bundler's Node platform asserts `node` and `require` itself, and
        // asserting them twice is not better than once.
        Target::Node => &[],
        // Not ours to choose. See the variant.
        Target::Library => &[],
    };
    let mut names: Vec<String> = base.iter().map(|c| (*c).to_string()).collect();
    names.extend(extra);
    names
}

/// The main fields a target falls back on, or `None` to leave the bundler's own.
pub fn main_fields(target: Target) -> Option<Vec<String>> {
    match target {
        // A library still has to *resolve* what it externalises, so it takes
        // the same fallback: choosing a build and finding one are different
        // questions.
        Target::Server | Target::Browser | Target::Library => {
            Some(MAIN_FIELDS.iter().map(|f| (*f).to_string()).collect())
        }
        // Node's are the bundler's to know, and it does.
        Target::Node => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The condition that decides whether a package hands over its Web-API build
    /// or its `node:` one. Losing it would not fail the build — it would produce
    /// a bundle that imports `node:stream` and dies at runtime.
    #[test]
    fn a_server_build_asserts_worker() {
        assert_eq!(conditions(Target::Server, []), vec!["worker"]);
    }

    /// Asserting `worker` while bundling for a browser hands over a build
    /// written for somewhere without a `document`.
    #[test]
    fn a_browser_build_asserts_browser_and_not_worker() {
        assert_eq!(conditions(Target::Browser, []), vec!["browser"]);
    }

    /// A project adding a condition of its own must not lose ours: the two are
    /// answering different questions.
    #[test]
    fn the_callers_conditions_are_appended_not_substituted() {
        assert_eq!(
            conditions(Target::Server, ["development".to_string()]),
            vec!["worker", "development"]
        );
    }

    /// ESM first. A package with both is offering the CommonJS one to callers
    /// that cannot take the other.
    #[test]
    fn main_fields_prefer_module() {
        assert_eq!(
            main_fields(Target::Server),
            Some(vec!["module".to_string(), "main".to_string()])
        );
        assert_eq!(main_fields(Target::Browser), main_fields(Target::Server));
    }

    /// Node is the one target whose resolution the bundler already knows, and
    /// restating it here would be a second copy to keep current.
    /// A library is an input to somebody else's build, and a condition asserted
    /// here is one its consumer can no longer choose.
    #[test]
    fn a_library_asserts_nothing_of_its_own() {
        assert!(conditions(Target::Library, []).is_empty());
        assert_eq!(
            conditions(Target::Library, ["custom".to_string()]),
            vec!["custom"]
        );
    }

    #[test]
    fn node_is_left_to_the_bundler() {
        assert!(conditions(Target::Node, []).is_empty());
        assert_eq!(main_fields(Target::Node), None);
    }
}
