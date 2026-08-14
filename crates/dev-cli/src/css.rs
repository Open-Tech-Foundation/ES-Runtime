//! The stylesheets an `index.html` target references.
//!
//! For four increments a stylesheet was *copied*: read the bytes, hash them,
//! write them to `assets/`, point the `<link>` at the result. [`crate::html`]
//! said so in as many words, and said why it was a placeholder — a copied
//! stylesheet silently loses its `@import`s, because the browser resolves them
//! against wherever the file ended up rather than where it was written.
//!
//! This is that gap closed. The entry stylesheet and everything it imports
//! become one file, the way a module entry and everything it imports become one
//! bundle. The rest follows from having the file parsed at all:
//!
//! * **`@import` is resolved**, in order, wrapped in whatever `@media`,
//!   `@supports` and `@layer` the import specified — so bundling changes what
//!   the browser fetches and not what it renders.
//! * **Modern syntax is lowered** to what [`TARGETS`] actually ship: nesting,
//!   `color-mix()`, logical properties, `:is()`. What a developer writes is
//!   decided by the language, not by the oldest browser they support.
//! * **It is minified** in a real build and left alone in a `--watch` build,
//!   which is the same split [`crate::build`] makes for JavaScript and for the
//!   same reason: one is read by a browser, the other by a person.
//! * **`url()` references are followed**, which is the half that is easy to
//!   forget. A stylesheet that moves to `assets/` takes its font and its
//!   background image with it, or it arrives pointing at two 404s.
//!
//! # It does not do CSS modules, and does not pretend to
//!
//! lightningcss can hash class names per file. That is a *bundler*
//! feature — it needs the JavaScript that imports the stylesheet to receive the
//! mapping, and here nothing imports a stylesheet: a `<link>` in a document
//! does. `import "./x.css"` is the increment that makes CSS modules mean
//! something, and until it exists this would be a flag with nowhere to send its
//! output.

use std::path::{Path, PathBuf};

use lightningcss::bundler::{Bundler, FileProvider};
use lightningcss::dependencies::{Dependency, DependencyOptions};
use lightningcss::printer::PrinterOptions;
use lightningcss::stylesheet::{MinifyOptions, ParserOptions};
use lightningcss::targets::{Browsers, Targets};

/// The browsers a stylesheet is lowered for.
///
/// Hardcoded, and deliberately: a `browserslist` key would make what this build
/// emits depend on a query resolved against a database that changes weekly, so
/// two builds of the same commit could differ. Pinned numbers are boring and
/// reproducible, and boring is the correct property for a compiler target.
///
/// These are the last-but-one major versions of each evergreen engine at the
/// time of writing — far enough back to cover a browser that has not been
/// restarted in a while, recent enough that nesting and `color-mix()` survive
/// rather than being expanded into something four times the size. Safari is the
/// binding constraint, as it is every time.
const TARGETS: Browsers = Browsers {
    android: None,
    chrome: Some(111 << 16),
    edge: Some(111 << 16),
    firefox: Some(113 << 16),
    ie: None,
    ios_saf: Some(16 << 16 | 4 << 8),
    opera: None,
    safari: Some(16 << 16 | 4 << 8),
    samsung: None,
};

/// A file a stylesheet referenced with `url()`, waiting to be given a name.
///
/// The stylesheet cannot name it itself: the name a file gets in `assets/` is
/// its content hash, and [`crate::html`] is what computes those. So the printed
/// CSS carries lightningcss's placeholder where the URL goes, and the caller
/// swaps in the real one once it has written the file.
#[derive(Debug)]
pub struct Referenced {
    /// The file on disk, resolved against the stylesheet that named it.
    pub path: PathBuf,
    /// The opaque string standing in for its URL in [`Stylesheet::code`].
    pub placeholder: String,
}

/// One stylesheet, bundled.
#[derive(Debug)]
pub struct Stylesheet {
    /// The CSS, with a placeholder at every `url()`.
    pub code: String,
    /// The files those placeholders stand for.
    pub referenced: Vec<Referenced>,
    /// How many files were merged into it, the entry included — so a build can
    /// say that one `<link>` became four files rather than reporting the one
    /// tag it started from.
    pub sources: usize,
}

/// Bundles `entry` and everything it imports.
pub fn bundle(entry: &Path, minify: bool) -> Result<Stylesheet, String> {
    let targets = Targets {
        browsers: Some(TARGETS),
        ..Default::default()
    };

    let provider = FileProvider::new();
    let mut bundler = Bundler::new(&provider, None, ParserOptions::default());
    let mut stylesheet = bundler
        .bundle(entry)
        .map_err(|e| format!("cannot bundle {}: {e}", entry.display()))?;

    // Merges duplicate rules, drops what the targets make redundant, and
    // shortens values. Distinct from the printer's `minify`, which only removes
    // whitespace — both are wanted, and only in a real build.
    if minify {
        stylesheet
            .minify(MinifyOptions {
                targets,
                ..Default::default()
            })
            .map_err(|e| format!("cannot minify {}: {e}", entry.display()))?;
    }

    let printed = stylesheet
        .to_css(PrinterOptions {
            minify,
            targets,
            // `remove_imports` is false because there is nothing left to remove:
            // the bundler has already inlined every `@import` it could resolve.
            // One that names a URL rather than a file survives bundling, and has
            // to survive printing too — it is a rule about what the *browser*
            // fetches, and this build was never asked to fetch it.
            analyze_dependencies: Some(DependencyOptions {
                remove_imports: false,
            }),
            ..Default::default()
        })
        .map_err(|e| format!("cannot print {}: {e}", entry.display()))?;

    let mut referenced = Vec::new();
    for dependency in printed.dependencies.into_iter().flatten() {
        let Dependency::Url(url) = dependency else {
            continue;
        };
        // Same rule the document itself follows ([`crate::html`]): a relative
        // path is a build input, anything else names something this build does
        // not control. Written once here rather than shared, because the two
        // disagree on one case — `/logo.svg` in CSS is a URL, and `/logo.svg` as
        // an `@import` would be too.
        if !is_local(&url.url) {
            continue;
        }
        // Against the file that *wrote* the `url()`, which after bundling is
        // very often not the entry. An imported `theme.css` naming
        // `url(./bg.png)` means the `bg.png` beside `theme.css`, and resolving
        // it against the entry instead would find the wrong file or no file.
        let from = Path::new(&url.loc.file_path)
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let path = from.join(strip_suffix(&url.url));
        if !path.is_file() {
            return Err(format!(
                "{} references {}, which is not there.\n\n\
                 A relative url() in a stylesheet names a file in the project. For \
                 a URL this build should leave alone, write it rooted (/{}) or \
                 absolute.",
                short(&url.loc.file_path, entry),
                url.url,
                url.url.trim_start_matches("./"),
            ));
        }
        referenced.push(Referenced {
            path,
            placeholder: url.placeholder,
        });
    }

    Ok(Stylesheet {
        code: printed.code,
        referenced,
        sources: stylesheet.sources.len(),
    })
}

/// Whether a `url()` names a file in the project rather than something the
/// browser fetches from elsewhere.
fn is_local(url: &str) -> bool {
    !url.is_empty()
        && !url.starts_with('/')
        && !url.starts_with('#')
        && !url.contains("://")
        && !url.starts_with("data:")
        && !url.starts_with("//")
}

/// `./font.woff2?v=2` and `./sprite.svg#icon` name `font.woff2` and
/// `sprite.svg`.
///
/// The query and the fragment are the browser's business and are kept in the
/// output URL; they are only in the way when the string is used as a path.
fn strip_suffix(url: &str) -> &str {
    let end = url.find(['?', '#']).unwrap_or(url.len());
    &url[..end]
}

/// A source path as it is worth showing in an error: relative to the entry's
/// directory, so a message names `theme/dark.css` rather than an absolute path
/// that is mostly somebody's home directory.
fn short<'a>(path: &'a str, entry: &Path) -> &'a str {
    entry
        .parent()
        .and_then(|base| Path::new(path).strip_prefix(base).ok())
        .and_then(|relative| relative.to_str())
        .unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes a stylesheet tree under a fresh directory and returns the entry.
    fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("esdev-css-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, contents) in files {
            let file = dir.join(path);
            std::fs::create_dir_all(file.parent().expect("a parent")).expect("mkdir");
            std::fs::write(&file, contents).expect("write");
        }
        dir.join(files[0].0)
    }

    /// The whole reason the crate is here: a copied stylesheet loses these.
    #[test]
    fn an_import_becomes_part_of_the_file() {
        let entry = project(
            "import",
            &[
                (
                    "styles.css",
                    "@import \"./theme.css\";\nbody { color: var(--ink) }",
                ),
                ("theme.css", ":root { --ink: #111 }"),
            ],
        );

        let bundled = bundle(&entry, true).expect("bundles");
        assert!(bundled.code.contains("--ink"), "{}", bundled.code);
        assert!(
            !bundled.code.contains("@import"),
            "the import survived: {}",
            bundled.code
        );
        assert_eq!(bundled.sources, 2);
    }

    /// An `@import` one directory down naming a sibling of *its own*, which is
    /// the case a resolver that anchors everything to the entry gets wrong.
    #[test]
    fn an_import_resolves_against_the_file_that_wrote_it() {
        let entry = project(
            "nested",
            &[
                ("styles.css", "@import \"./theme/dark.css\";"),
                ("theme/dark.css", "@import \"./vars.css\";"),
                ("theme/vars.css", ":root { --bg: #000 }"),
            ],
        );

        let bundled = bundle(&entry, true).expect("bundles");
        assert!(bundled.code.contains("--bg"), "{}", bundled.code);
        assert_eq!(bundled.sources, 3);
    }

    /// A `url()` is reported so the file can travel with the stylesheet, and is
    /// resolved against whichever file named it.
    #[test]
    fn a_url_is_reported_against_the_file_that_named_it() {
        let entry = project(
            "url",
            &[
                ("styles.css", "@import \"./theme/dark.css\";"),
                ("theme/dark.css", "body { background: url(./bg.png) }"),
                ("theme/bg.png", "not really a png"),
            ],
        );

        let bundled = bundle(&entry, true).expect("bundles");
        assert_eq!(bundled.referenced.len(), 1);
        assert!(
            bundled.referenced[0].path.ends_with("theme/bg.png"),
            "{:?}",
            bundled.referenced[0].path
        );
        // The URL is gone from the output until the caller substitutes it.
        assert!(
            bundled.code.contains(&bundled.referenced[0].placeholder),
            "{}",
            bundled.code
        );
    }

    /// The escape hatch, and the reason it has to be one: a build that tried to
    /// resolve these would fail on every stylesheet that uses a CDN font.
    #[test]
    fn a_url_this_build_does_not_control_is_left_alone() {
        let entry = project(
            "external",
            &[(
                "styles.css",
                "body { background: url(/logo.svg) }\n\
                 div { background: url(https://cdn.example/x.png) }\n\
                 span { background: url(data:image/gif;base64,R0lGOD) }",
            )],
        );

        let bundled = bundle(&entry, true).expect("bundles");
        assert!(bundled.referenced.is_empty(), "something was resolved");
    }

    /// A `url()` naming nothing is the build's mistake to report, not a 404 to
    /// discover in a browser later.
    #[test]
    fn a_url_naming_nothing_is_an_error_that_says_where() {
        let entry = project(
            "missing",
            &[("styles.css", "body { background: url(./nope.png) }")],
        );

        let message = bundle(&entry, true).expect_err("no such file");
        assert!(message.contains("nope.png"), "{message}");
        assert!(message.contains("not there"), "{message}");
    }

    /// The query is for the browser and only in the way when resolving a path.
    #[test]
    fn a_query_and_a_fragment_are_not_part_of_the_filename() {
        assert_eq!(strip_suffix("./font.woff2?v=2"), "./font.woff2");
        assert_eq!(strip_suffix("./sprite.svg#icon"), "./sprite.svg");
        assert_eq!(strip_suffix("./plain.png"), "./plain.png");
    }

    /// Syntax lowering is the second reason to parse the file, and the one a
    /// developer feels: nesting is what they would otherwise hand-expand.
    #[test]
    fn modern_syntax_is_lowered_to_what_the_targets_ship() {
        let entry = project("lower", &[("styles.css", "main { & a { color: red } }")]);

        let bundled = bundle(&entry, true).expect("bundles");
        assert!(
            bundled.code.contains("main a"),
            "nesting survived: {}",
            bundled.code
        );
    }

    /// A parse error has to name the file and the line, or it is a worse
    /// message than the one a browser gives by silently dropping the rule.
    #[test]
    fn a_broken_stylesheet_is_an_error_that_says_where() {
        let entry = project("broken", &[("styles.css", "@import ;")]);

        let message = bundle(&entry, true).expect_err("does not parse");
        assert!(message.contains("styles.css"), "{message}");
    }
}
