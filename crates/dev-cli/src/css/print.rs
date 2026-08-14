//! The tree back into CSS.
//!
//! Two modes, and the difference between them is the whole design:
//!
//! [`print`] is **lossless**. Every token carries its verbatim text, and this
//! concatenates them in order, so `print(parse(x)) == x` for any input at all —
//! including input that is not valid CSS. That is the guarantee a pass relies
//! on: whatever it does not touch comes out exactly as it went in, so a build
//! cannot quietly rewrite a construct nothing here understands.
//!
//! [`print_minified`] drops what carries no meaning: comments, and the
//! whitespace that is not doing work.
//!
//! # Which whitespace is doing work
//!
//! Two separate questions, and conflating them is how minifiers break pages.
//!
//! **Would removing it change the tokens?** `main a` → `maina` is one
//! identifier instead of two; `and (min-width:0)` → `and(min-width:0)` is a
//! function instead of a keyword and a block. This is decidable from the token
//! kinds either side, which is what [`must_separate`] answers, and it is the
//! majority of cases.
//!
//! **Would removing it change the meaning while leaving the tokens alone?**
//! Only two, and both need to know *where* in the tree we are — which is
//! exactly what a token stream cannot tell you and a syntax tree can:
//!
//! * In a **selector**, whitespace *is* the descendant combinator: `main a`,
//!   `main :hover` and `& a` all tokenize identically when closed up, and all
//!   mean something else. [`Context::Selector`] therefore keeps every space,
//!   because no token-level rule can pick out the ones that matter.
//! * `calc(100% - 1px)` and `calc(100%-1px)` tokenize identically and the
//!   second is invalid, because the math functions require whitespace around
//!   `+` and `-`. It matters inside `calc`, `min`, `max` and `clamp`, so
//!   [`Context::Math`] keeps it.

use super::ast::*;
use super::token::{Kind, Token};

/// Prints the tree exactly as it was parsed.
pub fn print(sheet: &Stylesheet) -> String {
    let mut out = Printer {
        out: String::new(),
        minify: false,
        last: None,
    };
    out.stylesheet(sheet);
    out.out
}

/// Prints the tree without comments or unnecessary whitespace.
pub fn print_minified(sheet: &Stylesheet) -> String {
    let mut out = Printer {
        out: String::new(),
        minify: true,
        last: None,
    };
    out.stylesheet(sheet);
    out.out
}

/// Prints a single component value, losslessly.
///
/// For a pass that needs to read a fragment of a prelude back as text — an
/// `@import`'s media conditions, say — rather than to interpret it.
pub fn value_text(value: &ComponentValue) -> String {
    let mut printer = Printer {
        out: String::new(),
        minify: false,
        last: None,
    };
    printer.value(value, Context::Value);
    printer.out
}

/// Where in the tree a component-value list sits, for the two whitespace rules
/// that cannot be decided from tokens alone.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Context {
    /// A selector. Whitespace **is** the descendant combinator here, so it is
    /// kept between anything and anything: `main a`, `main :hover` and `& a`
    /// each mean something different closed up, and each tokenizes identically
    /// either way. Only a `,` is safe to sit against.
    Selector,
    /// An at-rule prelude: `@media screen and (…)`. Conservative for the same
    /// reason — its keywords are separated by whitespace and nothing else — but
    /// a parenthesised group inside one is a media feature, which reads like a
    /// declaration and prints as [`Context::Value`].
    AtPrelude,
    /// Inside `calc()` and friends: a space around `+` and `-` is required.
    Math,
    /// A declaration value or anything else.
    Value,
}

impl Context {
    /// Whether whitespace is meaningful throughout, rather than only where it
    /// keeps two tokens from merging.
    fn keeps_whitespace(self) -> bool {
        matches!(self, Context::Selector | Context::AtPrelude)
    }
}

struct Printer {
    out: String,
    minify: bool,
    /// The last significant token written, or `None` if what was written last
    /// was a closing bracket that nothing can merge with.
    ///
    /// Needed because a space can be significant at the *boundary* of a list —
    /// `@media` and its prelude are printed by different calls, and dropping
    /// the space between them yields `@mediascreen`.
    last: Option<Token>,
}

impl Printer {
    fn push(&mut self, text: &str) {
        self.out.push_str(text);
    }

    fn stylesheet(&mut self, sheet: &Stylesheet) {
        for item in &sheet.items {
            match item {
                Item::Trivia(token) => {
                    if !self.minify {
                        self.push(&token.text);
                    }
                }
                Item::Rule(rule) => self.rule(rule),
                Item::Dangling(values) => self.values(values, Context::Selector),
            }
        }
    }

    fn rule(&mut self, rule: &Rule) {
        match rule {
            Rule::At(at) => {
                self.push(&at.at.text);
                self.last = Some(at.at.clone());
                self.values(&at.prelude, Context::AtPrelude);
                match &at.block {
                    Some(block) => self.block(block),
                    None => {
                        if at.semicolon {
                            self.push(";");
                        }
                        self.last = None;
                    }
                }
            }
            Rule::Qualified(qualified) => {
                self.values(&qualified.prelude, Context::Selector);
                self.block(&qualified.block);
            }
        }
    }

    fn block(&mut self, block: &Block) {
        self.push("{");
        self.last = None;
        if self.minify {
            self.block_minified(block);
        } else {
            for item in &block.items {
                match item {
                    BlockItem::Trivia(token) => self.push(&token.text),
                    BlockItem::Semicolon => self.push(";"),
                    BlockItem::Declaration(declaration) => self.declaration(declaration),
                    BlockItem::Rule(rule) => self.rule(rule),
                    BlockItem::Dangling(values) => self.values(values, Context::Selector),
                }
            }
        }
        if block.closed {
            self.push("}");
        }
        self.last = None;
    }

    /// A block with its trivia dropped and its separators put back.
    ///
    /// Semicolons are re-derived rather than copied: the input's may be
    /// missing, doubled, or trailing, and exactly one is wanted **between**
    /// declarations. The last one before `}` terminates nothing, which is the
    /// one place CSS makes it optional.
    fn block_minified(&mut self, block: &Block) {
        let mut first = true;
        for item in &block.items {
            match item {
                BlockItem::Trivia(_) | BlockItem::Semicolon => {}
                BlockItem::Dangling(values) => self.values(values, Context::Selector),
                BlockItem::Declaration(declaration) => {
                    if !first {
                        self.push(";");
                    }
                    self.declaration(declaration);
                    first = false;
                }
                BlockItem::Rule(rule) => {
                    // A rule needs no `;` after it, but one *before* it is
                    // needed if a declaration came first.
                    if !first {
                        self.push(";");
                    }
                    self.rule(rule);
                    // …and the rule's own `}` separates what follows.
                    first = true;
                }
            }
        }
    }

    fn declaration(&mut self, declaration: &Declaration) {
        self.push(&declaration.name.text);
        if !self.minify {
            for token in &declaration.before_colon {
                self.push(&token.text);
            }
        }
        self.push(":");
        self.last = None;
        self.values(&declaration.value, Context::Value);
    }

    /// A component-value list, applying the whitespace rules when minifying.
    fn values(&mut self, values: &[ComponentValue], context: Context) {
        if !self.minify {
            for value in values {
                self.value(value, context);
            }
            return;
        }

        // Which values carry meaning, and whether the author wrote whitespace
        // before each of them. The flag is what stops a space being *invented*
        // where there was none.
        let mut spaced = false;
        for value in values {
            if value.is_trivia() {
                spaced = true;
                continue;
            }
            if spaced && self.needs_space(first_token(value), context) {
                self.push(" ");
            }
            self.value(value, context);
            spaced = false;
        }
    }

    /// Whether a space before `right` must survive, given what was written last.
    fn needs_space(&self, right: Option<&Token>, context: Context) -> bool {
        let left = self.last.as_ref();

        // Where whitespace is meaningful in itself, any space the author wrote
        // between two significant values is kept.
        if context.keeps_whitespace() {
            let is_comma = |t: Option<&Token>| t.is_some_and(|t| t.kind == Kind::Comma);
            return left.is_some() && !is_comma(left) && !is_comma(right);
        }

        if context == Context::Math {
            let is_sign = |t: Option<&Token>| {
                t.is_some_and(|t| t.kind == Kind::Delim && (t.text == "+" || t.text == "-"))
            };
            if is_sign(left) || is_sign(right) {
                return true;
            }
        }

        match (left, right) {
            (Some(left), Some(right)) => must_separate(left, right),
            _ => false,
        }
    }

    fn value(&mut self, value: &ComponentValue, context: Context) {
        match value {
            ComponentValue::Token(token) => {
                if self.minify && token.is_trivia() {
                    return;
                }
                self.push(&token.text);
                self.last = Some(token.clone());
            }
            ComponentValue::Function(function) => {
                self.push(&function.name.text);
                self.last = Some(function.name.clone());
                let inner = if MATH.contains(&function.name().as_str()) {
                    Context::Math
                } else if context == Context::Selector {
                    // `:is(a b)` holds a selector, so the selector rules still
                    // apply inside it.
                    Context::Selector
                } else {
                    Context::Value
                };
                self.values(&function.arguments, inner);
                if function.closed {
                    self.push(")");
                    // Nothing merges with a closing bracket.
                    self.last = None;
                }
            }
            ComponentValue::Block(block) => {
                self.push(&block.open.text);
                self.last = Some(block.open.clone());
                // `[href^='x' i]` is part of a selector and keeps its spaces;
                // `(min-width: 40em)` is a media feature and reads as a value.
                let inner = match context {
                    Context::AtPrelude => Context::Value,
                    other => other,
                };
                self.values(&block.items, inner);
                if block.closed {
                    self.push(block.close());
                    self.last = None;
                }
            }
        }
    }
}

/// The functions whose grammar requires whitespace around `+` and `-`.
const MATH: &[&str] = &["calc", "min", "max", "clamp"];

/// Whether removing the whitespace between two tokens would change how they
/// tokenize — the majority of the cases, decided from kinds alone.
fn must_separate(left: &Token, right: &Token) -> bool {
    use Kind::*;

    // A token that ends in a name can absorb whatever a name can start with.
    let name_ish = |k: Kind| matches!(k, Ident | AtKeyword | Hash | Dimension | Function | Url);
    let numeric = |k: Kind| matches!(k, Number | Percentage | Dimension);

    let starts_name = |t: &Token| {
        matches!(
            t.kind,
            Ident | Function | Url | Number | Percentage | Dimension
        ) || (t.kind == Delim && matches!(t.text.as_str(), "-" | "\\"))
    };

    if name_ish(left.kind) && starts_name(right) {
        return true;
    }
    // `and (` → `and(` is a function token where there was a keyword and a
    // block. This is the media-query case, and the reason it is easy to miss is
    // that nothing else about the two tokens changes.
    if name_ish(left.kind) && right.kind == Kind::OpenParen {
        return true;
    }
    // `10 px` must not become `10px`, and `1 -2` must not become `1-2`.
    if numeric(left.kind) && (starts_name(right) || right.kind == Percentage) {
        return true;
    }
    // `- 1px` → `-1px` turns a delimiter and a dimension into one dimension.
    if left.kind == Delim && matches!(left.text.as_str(), "-" | "+" | ".") && numeric(right.kind) {
        return true;
    }
    // `# fff` → `#fff`.
    if left.kind == Delim && left.text == "#" {
        return true;
    }
    // `/ /` → `//`, `* *` → `**`: two delimiters can form a longer operator in
    // a selector or a media query.
    if left.kind == Delim && right.kind == Delim {
        return true;
    }
    // A comment's `/` can pair with a following `*`.
    if left.kind == Delim && left.text == "/" && right.kind == Delim && right.text == "*" {
        return true;
    }
    false
}

/// The first token a component value begins with.
fn first_token(value: &ComponentValue) -> Option<&Token> {
    match value {
        ComponentValue::Token(token) => Some(token),
        ComponentValue::Function(function) => Some(&function.name),
        ComponentValue::Block(block) => Some(&block.open),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::parse::parse;

    fn minified(source: &str) -> String {
        print_minified(&parse(source))
    }

    #[test]
    fn comments_go_and_whitespace_collapses() {
        assert_eq!(
            minified("/* a note */\nbody {\n  color: red;\n}\n"),
            "body{color:red}"
        );
    }

    /// Removing these changes the tokens, so they cannot go.
    #[test]
    fn whitespace_that_would_change_the_tokens_stays() {
        assert_eq!(minified("main  a { color: red }"), "main a{color:red}");
        assert_eq!(
            minified("@media screen and (min-width: 40em) { a { b: c } }"),
            "@media screen and (min-width:40em){a{b:c}}"
        );
        assert_eq!(minified("a { margin: 0 auto }"), "a{margin:0 auto}");
        assert_eq!(minified("a { margin: 1px  2px }"), "a{margin:1px 2px}");
    }

    /// Removing these leaves the tokens alone and changes the meaning. Both
    /// need to know where in the tree they are.
    #[test]
    fn whitespace_that_only_the_tree_can_judge_stays() {
        // A descendant of any `:hover`, not `main:hover`.
        assert_eq!(
            minified("main :hover { color: red }"),
            "main :hover{color:red}"
        );
        // …while a declaration's colon loses its space.
        assert_eq!(minified("a { color : red }"), "a{color:red}");
        // The math functions require the spaces around their operators.
        assert_eq!(
            minified("a { width: calc(100% - 1px) }"),
            "a{width:calc(100% - 1px)}"
        );
        assert_eq!(
            minified("a { width: clamp(1rem + 1px, 2vw, 3rem) }"),
            "a{width:clamp(1rem + 1px,2vw,3rem)}"
        );
        // …and a non-math function does not.
        assert_eq!(
            minified("a { color: rgb(1 , 2 , 3) }"),
            "a{color:rgb(1,2,3)}"
        );
    }

    #[test]
    fn a_string_is_left_exactly_alone() {
        assert_eq!(
            minified("a::before { content: \"a  b\" }"),
            "a::before{content:\"a  b\"}"
        );
        assert_eq!(
            minified("a { content: \"/* not a comment */\" }"),
            "a{content:\"/* not a comment */\"}"
        );
    }

    #[test]
    fn a_comment_does_not_join_what_it_separated() {
        assert_eq!(minified("main /* x */ a { b: c }"), "main a{b:c}");
    }

    #[test]
    fn semicolons_are_put_back_where_they_belong() {
        // Missing, doubled and trailing all normalise to one between each pair.
        assert_eq!(minified("a{b:c;;d:e;}"), "a{b:c;d:e}");
        assert_eq!(minified("a{b:c}"), "a{b:c}");
        // A nested rule needs one *before* it when a declaration came first,
        // and none after: its own `}` is what ends it.
        assert_eq!(minified("a{b:c; & d{e:f} g:h}"), "a{b:c;& d{e:f}g:h}");
    }

    #[test]
    fn nesting_survives_unchanged() {
        assert_eq!(
            minified("nav { gap: 1rem; & a { color: red } }"),
            "nav{gap:1rem;& a{color:red}}"
        );
    }

    #[test]
    fn an_empty_or_trivial_sheet_is_empty() {
        assert_eq!(minified(""), "");
        assert_eq!(minified("   \n\t "), "");
        assert_eq!(minified("/* just a note */"), "");
    }

    /// Minifying twice must equal minifying once, or the output depends on how
    /// many times a build ran.
    #[test]
    fn minifying_is_idempotent() {
        for source in [
            "body { color: red }",
            "@media screen and (min-width: 40em) { a { b: c } }",
            "a { width: calc(100% - 1px) }",
            "main :hover { d: e }",
            "a { content: \"a  b\" }",
            "nav { gap: 1rem; & a { color: red } }",
            "a{b:c;;d:e;}",
        ] {
            let once = minified(source);
            assert_eq!(minified(&once), once, "not idempotent: {source:?}");
        }
    }
}
