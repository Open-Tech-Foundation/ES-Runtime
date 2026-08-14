//! Making a stylesheet smaller without changing what it means.
//!
//! Two transformations, both structural: **drop comments**, and **collapse
//! whitespace**. That is the whole of it, and the ceiling is deliberate.
//!
//! # Why it stops there
//!
//! The next things a minifier does — `#ffffff` to `#fff`, `0.5rem` to `.5rem`,
//! merging duplicate selectors, dropping overridden declarations — all require
//! *understanding values*, which means a parser, an AST, and a printer that can
//! re-emit every construct in CSS. That is a large amount of code whose failure
//! mode is emitting different CSS than it was given, silently, in a build.
//!
//! What it would buy is small. These files are served over brotli, which
//! already collapses the repetition that value-shortening targets; measured on
//! the React template's own stylesheet the remaining win is a few percent of an
//! already-small file. A few percent is not worth a printer.
//!
//! So this stays a *text* transformation over [`super::token`]'s spans, and
//! never re-prints a construct it does not understand — the same rule the rest
//! of the pipeline follows. If value-level minification is ever wanted, it
//! belongs in a new module beside this one, over a real parser, behind its own
//! flag.
//!
//! # The whitespace that cannot go
//!
//! Whitespace is significant in more places than it first appears, and each of
//! these is a rule that renders differently if the space is dropped:
//!
//! * Inside a string — `content: "a  b"` is two spaces.
//! * The descendant combinator — `a b` and `ab` are different selectors.
//! * Around `+` and `-` in `calc()` — `calc(100% -1px)` is not a subtraction,
//!   and removing the space either side of the `-` changes the value.
//! * Between a media feature and its keyword — `screen and (min-width:0)`.
//!
//! Rather than enumerate where a space may go, this keeps **one** space
//! wherever there was any, and removes it only next to a character that cannot
//! be part of an identifier or a number. That is the conservative direction: it
//! leaves bytes on the table and never changes a meaning.

use super::token::{Kind, tokenize};

/// Returns `source` with comments dropped and whitespace collapsed.
pub fn minify(source: &str) -> String {
    let tokens = tokenize(source);
    let mut out = String::with_capacity(source.len());

    for (i, token) in tokens.iter().enumerate() {
        match token.kind {
            // A comment is never meaningful to a browser. (`/*! … */`, the
            // convention for a licence banner some tools preserve, is not
            // special-cased: nothing in this project's own CSS relies on it,
            // and a banner that must survive belongs in the build's own output
            // handling rather than in a comment.)
            Kind::Comment => {}

            Kind::Whitespace => {
                // What sits either side decides whether this space is doing
                // work. Comments are looked through in both directions —
                // `a /* x */ b` is still a descendant combinator, and the two
                // runs of whitespace around the dropped comment are together
                // worth at most one space.
                let before = tokens[..i]
                    .iter()
                    .rev()
                    .find(|t| !matches!(t.kind, Kind::Comment | Kind::Whitespace))
                    .map(|t| last_char(t.text(source)));
                let after = tokens[i + 1..]
                    .iter()
                    .find(|t| !matches!(t.kind, Kind::Comment | Kind::Whitespace))
                    .map(|t| first_char(t.text(source)));

                // Leading and trailing whitespace in the file has nothing to
                // separate.
                let (Some(before), Some(after)) = (before, after) else {
                    continue;
                };
                if drop_after(before) || drop_before(after) {
                    continue;
                }
                // …and never two in a row, which is what a dropped comment
                // between two runs of whitespace would otherwise produce.
                if !out.ends_with(' ') {
                    out.push(' ');
                }
            }

            // A `;` immediately before the `}` that ends the block terminates
            // nothing. The last declaration in a rule is the one place CSS
            // makes it optional.
            Kind::Semicolon
                if tokens[i + 1..]
                    .iter()
                    .find(|t| !matches!(t.kind, Kind::Comment | Kind::Whitespace))
                    .is_some_and(|t| t.kind == Kind::CloseBrace) => {}

            // Everything else, including strings, exactly as written.
            _ => out.push_str(token.text(source)),
        }
    }

    out
}

/// Whether the space *following* `c` can go.
///
/// These are the characters that already terminate whatever preceded them:
/// after a `{`, a `,` or a `:` there is nothing a space could be joining, so
/// dropping it cannot merge two tokens into one.
fn drop_after(c: char) -> bool {
    matches!(c, '{' | '}' | ';' | ',' | ':' | '(' | '>' | '~')
}

/// Whether the space *preceding* `c` can go.
///
/// Deliberately not the same set, and the two exceptions are the bugs this
/// module's tests caught:
///
/// * **`(`** — `screen and (min-width: 0)` becomes `and(min-width:0)`, which is
///   a function call and not a media query. Safe to drop *after*, never before.
/// * **`:`** — `main :hover` and `main:hover` are different selectors. Telling
///   a declaration's colon from a pseudo-class's needs to know whether we are
///   in a selector or a block, which is parsing. Dropping only *after* a colon
///   gets `color:red` and leaves `main :hover` intact.
///
/// Also absent from both sets: `+`, `-`, `*` and `/`, which are `calc()`
/// operators whose surrounding spaces are part of the grammar.
fn drop_before(c: char) -> bool {
    matches!(c, '{' | '}' | ';' | ',' | ')' | '>' | '~')
}

fn first_char(text: &str) -> char {
    text.chars().next().unwrap_or(' ')
}

fn last_char(text: &str) -> char {
    text.chars().next_back().unwrap_or(' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_go_and_whitespace_collapses() {
        assert_eq!(
            minify("/* a note */\nbody {\n  color: red;\n}\n"),
            "body{color:red}"
        );
    }

    /// The whole reason [`drop_after`] and [`drop_before`] are short lists, and
    /// different ones. Each of these renders differently if the space goes.
    #[test]
    fn the_whitespace_that_means_something_stays() {
        // The descendant combinator.
        assert_eq!(minify("main  a { color: red }"), "main a{color:red}");
        // `calc()` arithmetic, where the spaces are part of the grammar.
        assert_eq!(
            minify("a { width: calc(100% - 1px) }"),
            "a{width:calc(100% - 1px)}"
        );
        // A media query's keywords.
        assert_eq!(
            minify("@media screen and (min-width: 40em) { a { b: c } }"),
            "@media screen and (min-width:40em){a{b:c}}"
        );
        // `:is(a, b) > c` — the combinator after a closing paren.
        assert_eq!(minify(":is(a, b)  c { d: e }"), ":is(a,b) c{d:e}");
    }

    /// A string is bytes the author chose, and two spaces in one are two
    /// spaces on the page.
    #[test]
    fn a_string_is_left_exactly_alone() {
        assert_eq!(
            minify("a::before { content: \"a  b\" }"),
            "a::before{content:\"a  b\"}"
        );
        // …including something that looks like a comment.
        assert_eq!(
            minify("a { content: \"/* not a comment */\" }"),
            "a{content:\"/* not a comment */\"}"
        );
    }

    /// A comment between two things is still a separator between them.
    #[test]
    fn a_comment_does_not_join_what_it_separated() {
        assert_eq!(minify("main /* x */ a { b: c }"), "main a{b:c}");
    }

    /// Nesting is passed through, not lowered — this pipeline does not rewrite
    /// selectors, and the target browsers support it.
    #[test]
    fn nesting_survives_unchanged() {
        assert_eq!(
            minify("main { & a { color: red } }"),
            "main{& a{color:red}}"
        );
    }

    #[test]
    fn an_empty_or_whitespace_only_sheet_is_empty() {
        assert_eq!(minify(""), "");
        assert_eq!(minify("   \n\t "), "");
        assert_eq!(minify("/* just a note */"), "");
    }

    /// Minifying twice must not differ from minifying once, or the output
    /// depends on how many times a build ran.
    #[test]
    fn minifying_is_idempotent() {
        for source in [
            "body { color: red }",
            "@media screen and (min-width: 40em) { a { b: c } }",
            "a { width: calc(100% - 1px) }",
            ":is(a, b)  c { d: e }",
            "a { content: \"a  b\" }",
        ] {
            let once = minify(source);
            assert_eq!(minify(&once), once, "not idempotent: {source:?}");
        }
    }
}
