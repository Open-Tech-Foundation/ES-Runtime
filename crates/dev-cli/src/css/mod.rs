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
//! bundle. Two things follow from having read the file at all:
//!
//! * **`@import` is resolved**, so bundling changes what the browser fetches
//!   and not what it renders.
//! * **`url()` references are followed**, which is the half that is easy to
//!   forget. A stylesheet that moves to `assets/` takes its font and its
//!   background image with it, or it arrives pointing at two 404s.
//!
//! # Written here rather than taken from a crate
//!
//! The obvious dependency is lightningcss, and it was tried first. It works,
//! and it is **MPL-2.0** — the licence reaches only its own files, but it is
//! copyleft, and it brings a copyleft subtree with it. For a project whose
//! `deny.toml` opens by saying copyleft fails the gate, that is a standing
//! exception to explain to every downstream commercial user who runs a licence
//! scanner over `esdev` (maintainer, 2026-08-14: no licensing complexity at any
//! layer). Apache-2.0 alternatives exist — swc's CSS crates are the strongest —
//! but they cost 121 crates of supply chain to buy features described below as
//! deliberately out of scope.
//!
//! What is actually needed turns out to be small, because of one decision:
//!
//! # It splices; it never re-prints
//!
//! Every pass here replaces the byte spans it understands and copies the rest
//! through untouched. Nothing is parsed into a structure and printed back.
//!
//! That is what makes this tractable at a few hundred lines. Modelling CSS
//! means modelling *all* of it, because a printer that meets a construct it has
//! no representation for emits something else instead — silently, in a build,
//! in a file nobody reads. A splicer that meets one leaves it alone. The
//! failure modes are "did nothing" against "did something wrong", and only one
//! of those is safe to ship.
//!
//! It is also the same architecture [`crate::html`] already uses on the
//! document: html5gum finds spans, the build splices them, every other byte of
//! the file survives.
//!
//! # The modules
//!
//! * [`token`] — the tokenizer. Guarantees the spans tile the input exactly.
//! * [`bundle`] — `@import` resolution and `url()` rewriting, in one pass.
//! * [`minify`] — comments and whitespace.
//!
//! Split this way because they are what a CSS pipeline grows *along*: a value
//! minifier, a syntax lowerer, a prefixer and CSS modules are each a new module
//! beside these rather than a change to them.
//!
//! # Deliberately not done
//!
//! **Syntax lowering** (nesting, `color-mix()`, logical properties) — every one
//! is supported by every browser in the range this project targets, so lowering
//! them today is work that produces a larger file and changes nothing.
//! **Vendor prefixing** — same reason. **Value minification** — see
//! [`minify`]. **CSS modules** and `import "./x.css"` from JavaScript — one
//! feature, and it needs a stylesheet to be a *module*, which is a bundler
//! change rather than a CSS one.
//!
//! Each is a module beside these when it is asked for, and none of them is
//! blocked by anything here.

pub mod bundle;
pub mod minify;
pub mod token;

use std::path::Path;

pub use bundle::Stylesheet;

/// Bundles `entry` and everything it imports, minified or not.
///
/// The one entry point [`crate::html`] uses; the modules behind it are public
/// so that a future pass can be added without routing it through here.
pub fn build(entry: &Path, minify: bool) -> Result<Stylesheet, String> {
    let mut stylesheet = bundle::bundle(entry)?;
    if minify {
        stylesheet.code = minify::minify(&stylesheet.code);
    }
    Ok(stylesheet)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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

    /// The whole reason this module exists: a copied stylesheet loses these.
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

        let built = build(&entry, true).expect("bundles");
        assert!(built.code.contains("--ink"), "{}", built.code);
        assert!(
            !built.code.contains("@import"),
            "the import survived: {}",
            built.code
        );
        assert_eq!(built.sources, 2);
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

        let built = build(&entry, true).expect("bundles");
        assert!(built.code.contains("--bg"), "{}", built.code);
        assert_eq!(built.sources, 3);
    }

    /// Both spellings, because a stylesheet in the wild uses either.
    #[test]
    fn an_import_is_recognised_as_a_string_or_a_url() {
        for spelling in [
            "@import \"./a.css\";",
            "@import url(./a.css);",
            "@import url(\"./a.css\");",
            "@import  './a.css' ;",
        ] {
            let entry = project(
                "spelling",
                &[("styles.css", spelling), ("a.css", "a{color:red}")],
            );
            let built = build(&entry, true).expect("bundles");
            assert!(
                built.code.contains("color:red") && !built.code.contains("@import"),
                "{spelling} produced {}",
                built.code
            );
        }
    }

    /// The condition applied to the whole imported sheet, so it has to apply to
    /// the whole of what replaces it.
    #[test]
    fn a_conditional_import_keeps_its_condition() {
        let entry = project(
            "conditional",
            &[
                ("styles.css", "@import \"./print.css\" print;"),
                ("print.css", "body { color: #000 }"),
            ],
        );

        let built = build(&entry, true).expect("bundles");
        assert_eq!(built.code, "@media print{body{color:#000}}");
    }

    /// `layer()` and `supports()` order the cascade in ways a `@media` wrapper
    /// cannot reproduce, so they are left for the browser rather than mangled.
    #[test]
    fn an_import_this_pass_cannot_reproduce_is_left_alone() {
        let entry = project(
            "layered",
            &[
                (
                    "styles.css",
                    "@import \"./a.css\" layer(base);\nbody{color:red}",
                ),
                ("a.css", "a{color:blue}"),
            ],
        );

        let built = build(&entry, false).expect("bundles");
        assert!(
            built.code.contains("@import \"./a.css\" layer(base);"),
            "{}",
            built.code
        );
        assert!(!built.code.contains("color:blue"), "{}", built.code);
        assert_eq!(built.sources, 1);
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

        let built = build(&entry, true).expect("bundles");
        assert_eq!(built.referenced.len(), 1);
        assert!(
            built.referenced[0].path.ends_with("theme/bg.png"),
            "{:?}",
            built.referenced[0].path
        );
        assert!(
            built.code.contains(&built.referenced[0].placeholder),
            "{}",
            built.code
        );
    }

    /// A quoted `url()` is a different token shape and has to be found too.
    #[test]
    fn a_quoted_url_is_rewritten_inside_its_quotes() {
        let entry = project(
            "quoted-url",
            &[
                ("styles.css", "body { background: url(\"./bg.png\") }"),
                ("bg.png", "png"),
            ],
        );

        let built = build(&entry, false).expect("bundles");
        assert_eq!(built.referenced.len(), 1);
        // The quotes survive; only what was between them was replaced.
        assert!(
            built
                .code
                .contains(&format!("url(\"{}\")", built.referenced[0].placeholder)),
            "{}",
            built.code
        );
    }

    /// Two references get two slots, and neither placeholder is a prefix of the
    /// other — the substitution above is a plain string replace.
    #[test]
    fn many_urls_get_placeholders_that_cannot_collide() {
        let mut css = String::new();
        for i in 0..12 {
            css.push_str(&format!(".a{i} {{ background: url(./{i}.png) }}\n"));
        }
        let owned: Vec<(String, String)> = std::iter::once(("styles.css".to_string(), css))
            .chain((0..12).map(|i| (format!("{i}.png"), "png".to_string())))
            .collect();
        let borrowed: Vec<(&str, &str)> = owned
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let entry = project("many-urls", &borrowed);

        let built = build(&entry, false).expect("bundles");
        assert_eq!(built.referenced.len(), 12);
        // Substituting in order must not corrupt a later one — `…url_1__`
        // must not match inside `…url_11__`.
        let mut code = built.code.clone();
        for (i, referenced) in built.referenced.iter().enumerate() {
            code = code.replace(&referenced.placeholder, &format!("/assets/{i}.png"));
        }
        assert!(
            !code.contains("__esdev_url"),
            "a placeholder survived: {code}"
        );
        for i in 0..12 {
            assert!(
                code.contains(&format!("/assets/{i}.png")),
                "{i} missing: {code}"
            );
        }
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
                 span { background: url(data:image/gif;base64,R0lGOD) }\n\
                 @import url(https://cdn.example/f.css);",
            )],
        );

        let built = build(&entry, false).expect("bundles");
        assert!(built.referenced.is_empty(), "something was resolved");
        assert!(
            built
                .code
                .contains("@import url(https://cdn.example/f.css);")
        );
    }

    /// The query is the browser's and is not part of the filename.
    #[test]
    fn a_query_survives_the_rewrite_without_being_part_of_the_path() {
        let entry = project(
            "query",
            &[
                ("styles.css", "@font-face { src: url(./f.woff2?v=2) }"),
                ("f.woff2", "font"),
            ],
        );

        let built = build(&entry, false).expect("bundles");
        assert_eq!(built.referenced.len(), 1);
        assert!(built.referenced[0].path.ends_with("f.woff2"));
        let substituted = built
            .code
            .replace(&built.referenced[0].placeholder, "/assets/f-abc.woff2");
        assert!(
            substituted.contains("url(/assets/f-abc.woff2?v=2)"),
            "{substituted}"
        );
    }

    /// A `url()` naming nothing is the build's mistake to report, not a 404 to
    /// discover in a browser later.
    #[test]
    fn a_url_naming_nothing_is_an_error_that_says_where() {
        let entry = project(
            "missing",
            &[("styles.css", "body { background: url(./nope.png) }")],
        );

        let message = build(&entry, true).expect_err("no such file");
        assert!(message.contains("nope.png"), "{message}");
        assert!(message.contains("not there"), "{message}");
    }

    /// Two stylesheets importing each other has no bundled form. It must
    /// terminate with a message rather than recurse until the stack goes.
    #[test]
    fn an_import_cycle_is_reported_rather_than_followed() {
        let entry = project(
            "cycle",
            &[
                ("a.css", "@import \"./b.css\";"),
                ("b.css", "@import \"./a.css\";"),
            ],
        );

        let message = build(&entry, false).expect_err("a cycle");
        assert!(message.contains("imports itself"), "{message}");
    }

    /// The property the whole design rests on: bytes this pipeline has no name
    /// for come out exactly as they went in.
    #[test]
    fn what_it_does_not_understand_survives_byte_for_byte() {
        let exotic = "@supports (display: grid) and (not (display: inline-grid)) {\n  \
                      .a { grid-template-areas: \"a b\" \"c d\"; }\n}\n\
                      @font-face { unicode-range: U+0025-00FF, U+4??; }\n\
                      @property --x { syntax: '<length>'; inherits: false; }\n\
                      .b { background: image-set(\"a.png\" 1x) }\n\
                      @layer base, components;\n";
        let entry = project("exotic", &[("styles.css", exotic)]);

        let built = build(&entry, false).expect("bundles");
        assert_eq!(built.code, exotic, "the pipeline rewrote something");
    }
}
