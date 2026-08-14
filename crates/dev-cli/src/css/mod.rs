//! The stylesheets an `index.html` target references.
//!
//! For four increments a stylesheet was *copied*: read the bytes, hash them,
//! write them to `assets/`, point the `<link>` at the result. [`crate::html`]
//! said so in as many words, and said why it was a placeholder — a copied
//! stylesheet silently loses its `@import`s, because the browser resolves them
//! against wherever the file ended up rather than where it was written.
//!
//! This is that gap closed, on a real parser rather than around one.
//!
//! # The layers
//!
//! * [`token`] — [CSS Syntax Level 3 §4][tok]. The spec's full token set;
//!   every token carries its verbatim text.
//! * [`ast`] — the tree. The spec's *generic* grammar and nothing beyond it:
//!   rules, blocks, declarations, component values. No selector type, no media
//!   query type, because those belong to layers that grow without limit while
//!   this one is closed and already complete.
//! * [`parse`] — [§5][parse]. Does not fail; malformed input is represented.
//! * [`print`] — the tree back to text, losslessly or minified.
//! * [`bundle`] — the two passes that exist today: `@import` and `url()`.
//!
//! A new pass — value minification, syntax lowering, prefixing, CSS modules —
//! is a module beside `bundle`, over the same tree. None of them is blocked by
//! anything here, and none of them requires touching the layers below.
//!
//! # Lossless by construction
//!
//! `print(parse(x)) == x`, for **any** input, valid CSS or not. Every token is
//! kept — whitespace, comments, unclosed blocks, bad strings — and printing is
//! concatenation.
//!
//! This is the property that makes the rest safe to build on. The frightening
//! failure mode in CSS tooling is a printer that meets a construct it has no
//! representation for and emits something else instead: silently, in a build,
//! in a file nobody reads. Here a pass that does not touch something cannot
//! change it, and the round trip is asserted below over a corpus of the exotic
//! constructs that would otherwise be where it happened.
//!
//! # Written here rather than taken from a crate
//!
//! lightningcss was tried first and works. It is **MPL-2.0**, as is the
//! `cssparser` subtree beneath it — seven copyleft crates in a project whose
//! `deny.toml` opens by saying copyleft fails the gate, and a standing
//! exception to re-explain to every downstream user who scans `esdev`
//! (maintainer, 2026-08-14: no licensing complexity at any layer). swc's CSS
//! crates are Apache-2.0 and carry no copyleft at all, but cost 121 crates to
//! buy the features listed below as out of scope.
//!
//! What is left, once those features are not wanted, is the spec's generic
//! grammar — which is small enough to own.
//!
//! # Deliberately not done
//!
//! **Syntax lowering** (nesting, `color-mix()`, logical properties) and
//! **vendor prefixing** — every one is supported by every browser in the range
//! this targets, so lowering them today produces a larger file and changes
//! nothing. **Value minification** (`#ffffff` → `#fff`) — needs a per-property
//! value grammar, which is the unbounded layer. **CSS modules** and
//! `import "./x.css"` from JavaScript — one feature, and it needs a stylesheet
//! to be a *module*, which is a bundler change rather than a CSS one.
//!
//! [tok]: https://www.w3.org/TR/css-syntax-3/#tokenization
//! [parse]: https://www.w3.org/TR/css-syntax-3/#parsing

pub mod ast;
pub mod bundle;
pub mod parse;
pub mod print;
pub mod token;

use std::path::Path;

pub use bundle::Referenced;

/// One stylesheet, bundled and printed.
#[derive(Debug)]
pub struct Stylesheet {
    /// The CSS, with a placeholder at every local `url()`.
    pub code: String,
    /// The files those placeholders stand for.
    pub referenced: Vec<Referenced>,
    /// How many files were merged into it, the entry included.
    pub sources: usize,
}

/// Bundles `entry` and everything it imports, minified or not.
///
/// The one entry point [`crate::html`] uses. The layers behind it are public so
/// that a new pass can be added without routing it through here.
pub fn build(entry: &Path, minify: bool) -> Result<Stylesheet, String> {
    let bundled = bundle::bundle(entry)?;
    let code = if minify {
        print::print_minified(&bundled.sheet)
    } else {
        print::print(&bundled.sheet)
    };
    Ok(Stylesheet {
        code,
        referenced: bundled.referenced,
        sources: bundled.sources,
    })
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

    /// **The** property of this design: anything the passes do not touch comes
    /// out as it went in. Asserted over the constructs that would otherwise be
    /// where a printer quietly rewrote something.
    #[test]
    fn parsing_and_printing_is_lossless() {
        for source in [
            "@supports (display: grid) and (not (display: inline-grid)) {\n  .a { grid-template-areas: \"a b\" \"c d\"; }\n}\n",
            "@font-face { unicode-range: U+0025-00FF, U+4??; }\n",
            "@property --x { syntax: '<length>'; inherits: false; }\n",
            ".b { background: image-set(\"a.png\" 1x) }\n",
            "@layer base, components;\n",
            "@media (400px <= width <= 700px) { a { b: c } }\n",
            "a[href^='x' i] ~ b > c + d { e: f }\n",
            ".x { --raw: { still tokens }; }\n",
            "@container card (min-width: 20em) { a { b: c } }\n",
            "a { transition: color 120ms ease, transform 120ms ease }\n",
            "/* leading */\n\n\nbody{color:red}\n\n/* trailing */\n",
            "<!-- a{b:c} -->",
            "a { color: RED; BACKGROUND: Blue }",
            ":root { --Brand: #FFF }",
            "@charset \"utf-8\";\n@import url(a.css) layer(base);\n",
            "",
            "   ",
            "}}}{{{",
            "a { color: rgb(1, 2",
        ] {
            let printed = print::print(&parse::parse(source));
            assert_eq!(printed, source, "round trip changed {source:?}");
        }
    }

    /// The round trip over every short string on the alphabet that drives the
    /// tokenizer and the parser. Exhaustive rather than sampled: the states
    /// that break a parser are the interleavings nobody thinks to write down,
    /// and at this length there are few enough to simply try them all.
    #[test]
    fn parsing_and_printing_is_lossless_for_every_short_input() {
        let alphabet: Vec<char> = r#"{}();:@"'\/*a1 "#.chars().collect();
        for len in 1..=4usize {
            let mut indices = vec![0usize; len];
            loop {
                let source: String = indices.iter().map(|&i| alphabet[i]).collect();
                assert_eq!(
                    print::print(&parse::parse(&source)),
                    source,
                    "round trip changed {source:?}"
                );

                let mut place = len;
                let mut carried = true;
                while carried && place > 0 {
                    place -= 1;
                    indices[place] += 1;
                    carried = indices[place] == alphabet.len();
                    if carried {
                        indices[place] = 0;
                    }
                }
                if carried {
                    break;
                }
            }
        }
    }

    /// The same property over the project's own stylesheet, which is the
    /// realistic input and the one a regression would actually reach.
    #[test]
    fn the_templates_own_stylesheets_round_trip() {
        for name in ["app.css", "theme.css"] {
            let path = concat!(env!("CARGO_MANIFEST_DIR"), "/templates/react/styles/");
            let source = std::fs::read_to_string(format!("{path}{name}"))
                .unwrap_or_else(|e| panic!("read {name}: {e}"));
            assert_eq!(
                print::print(&parse::parse(&source)),
                source,
                "{name} did not round-trip"
            );
        }
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
    /// the case a resolver anchored to the entry gets wrong.
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

    #[test]
    fn an_import_is_recognised_in_every_spelling() {
        for spelling in [
            "@import \"./a.css\";",
            "@import url(./a.css);",
            "@import url(\"./a.css\");",
            "@import  './a.css' ;",
            "@IMPORT \"./a.css\";",
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
        assert!(
            built
                .code
                .contains(&format!("\"{}\"", built.referenced[0].placeholder)),
            "{}",
            built.code
        );
    }

    /// A `url()` nested inside another function — `image-set()`, which is where
    /// a pass that only looked at the top level would miss it.
    #[test]
    fn a_url_nested_inside_a_function_is_still_found() {
        let entry = project(
            "nested-url",
            &[
                (
                    "styles.css",
                    "a { background: image-set(url(./a.png) 1x, url(\"./b.png\") 2x) }",
                ),
                ("a.png", "png"),
                ("b.png", "png"),
            ],
        );

        let built = build(&entry, false).expect("bundles");
        assert_eq!(built.referenced.len(), 2);
    }

    /// Two references get two slots, and no placeholder is a prefix of another
    /// — the substitution above is a plain string replace.
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
            substituted.contains("/assets/f-abc.woff2?v=2"),
            "{substituted}"
        );
    }

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
}
