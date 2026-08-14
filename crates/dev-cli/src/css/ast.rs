//! The syntax tree — [CSS Syntax Level 3 §5][spec]'s grammar, and only that.
//!
//! # What is *not* here, deliberately
//!
//! There is no `Selector`, no `MediaQuery`, no typed `Color` or `Length`. That
//! is not a shortcut; it is the spec's own design. CSS's grammar is two layers:
//! a **generic** one that every rule obeys — a prelude, then a block of
//! declarations or nested rules — and a **per-property** one that says what
//! `grid-template-areas` accepts. The generic layer is small, closed, and
//! already complete. The per-property layer is unbounded and grows with every
//! new specification.
//!
//! So a prelude is a list of [`ComponentValue`], which is exactly what a
//! selector *is* before something chooses to interpret it. That is what lets
//! this tree hold `@supports (display: grid) and (not (display: inline-grid))`,
//! `@property`, `unicode-range: U+0025-00FF` and whatever ships next year,
//! without knowing anything about them.
//!
//! Interpretation is a pass's job. A selector rewriter would read a prelude and
//! parse selectors out of it; nothing else would change.
//!
//! # Lossless by construction
//!
//! Every token is kept, whitespace and comments included, and every token
//! carries its verbatim text ([`super::token::Token`]). So
//! [`super::print::print`] of a parsed sheet reproduces the input byte for
//! byte, and a pass that changes nothing changes nothing.
//!
//! That property is the safety net the rest of the pipeline needs. A printer
//! that normalised as it went would rewrite constructs it did not understand,
//! silently, in a build — which is the failure mode that makes CSS tooling
//! frightening. Here, whatever a pass does not touch comes out as it went in,
//! and [`super::tests`] asserts it over a corpus.
//!
//! [spec]: https://www.w3.org/TR/css-syntax-3/#parsing

use super::token::{Kind, Token};

/// A parsed stylesheet.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stylesheet {
    pub items: Vec<Item>,
}

impl Stylesheet {
    /// Every rule, in order, skipping the trivia between them.
    ///
    /// Part of the tree's surface rather than of any pass that exists today —
    /// a pass that reads rules without caring where they sit is the common
    /// shape, and the first one to need it should not have to add it.
    #[allow(dead_code, reason = "tree API; used by tests and by the next pass")]
    pub fn rules(&self) -> impl Iterator<Item = &Rule> {
        self.items.iter().filter_map(|item| match item {
            Item::Rule(rule) => Some(rule),
            Item::Trivia(_) | Item::Dangling(_) => None,
        })
    }
}

/// One thing at the top level of a stylesheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Rule(Rule),
    /// Whitespace, a comment, or a stray `<!--` / `-->`. Kept so the tree can
    /// be printed back unchanged.
    Trivia(Token),
    /// A prelude that ran to the end of the input without a block —
    /// `a[href="x" { color: red }`, where the unclosed `[` swallowed the brace
    /// that would have ended the selector.
    ///
    /// There is no rule to be made of it and a browser applies nothing, but the
    /// bytes were in the file and losslessness is not optional: they are kept
    /// here rather than dropped on the floor.
    Dangling(Vec<ComponentValue>),
}

/// A rule: `@media … { … }` or `a:hover { … }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    At(AtRule),
    Qualified(QualifiedRule),
}

/// `@import "a.css";` or `@media print { … }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtRule {
    /// The at-keyword token, `@` included. Use [`Token::name`] to match on it —
    /// at-rule names are case-insensitive.
    pub at: Token,
    /// Everything between the name and the `{` or `;`.
    pub prelude: Vec<ComponentValue>,
    /// The body, for a block at-rule. `None` for a statement one like
    /// `@import`, which ends at its semicolon.
    pub block: Option<Block>,
    /// Whether a `;` was actually there. `@media (min-width: 0` at end of input
    /// has neither block nor semicolon, and must print back without one this
    /// parser invented.
    pub semicolon: bool,
}

impl AtRule {
    /// The at-rule's name, lowercased: `"media"`, `"import"`.
    pub fn name(&self) -> String {
        self.at.name()
    }
}

/// A style rule: a selector, then a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualifiedRule {
    /// The selector, uninterpreted.
    pub prelude: Vec<ComponentValue>,
    pub block: Block,
}

/// The `{ … }` of a rule.
///
/// It holds declarations *and* rules, because CSS Nesting means it can:
/// `.card { color: red; & a { color: blue } }` is one block with both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub items: Vec<BlockItem>,
    /// Whether a `}` was actually there. `a { color: red` is a stylesheet a
    /// browser accepts, and it must print back without a brace this parser
    /// invented.
    pub closed: bool,
}

impl Default for Block {
    fn default() -> Self {
        Block {
            items: Vec::new(),
            closed: true,
        }
    }
}

/// One thing inside a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockItem {
    Declaration(Declaration),
    /// A nested rule (CSS Nesting), or an at-rule such as `@media`.
    Rule(Rule),
    Trivia(Token),
    /// A `;`, kept separately so a printer can drop the redundant last one.
    Semicolon,
    /// Component values left over when the input ended mid-rule. See
    /// [`Item::Dangling`].
    Dangling(Vec<ComponentValue>),
}

/// `color: red` — a property and its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    /// The property name. Not lowercased: a custom property (`--Foo`) is
    /// case-*sensitive*, so normalising here would silently rename it.
    pub name: Token,
    /// Trivia between the name and the `:`, kept so `color : red` round-trips.
    pub before_colon: Vec<Token>,
    /// Everything after the `:`, including `!important` and surrounding trivia.
    pub value: Vec<ComponentValue>,
}

impl Declaration {
    /// Whether the value ends in `!important`.
    ///
    /// Not special-cased in the grammar — `!important` is just two tokens at
    /// the end of a value — so this is where the meaning is attached, for the
    /// passes that will need it.
    #[allow(dead_code, reason = "tree API; used by tests and by the next pass")]
    pub fn is_important(&self) -> bool {
        let mut significant = self
            .value
            .iter()
            .rev()
            .filter(|value| !matches!(value, ComponentValue::Token(t) if t.is_trivia()));
        let last = significant.next();
        let bang = significant.next();
        matches!(last, Some(ComponentValue::Token(t)) if t.kind == Kind::Ident && t.name() == "important")
            && matches!(bang, Some(ComponentValue::Token(t)) if t.kind == Kind::Delim && t.text == "!")
    }
}

/// A component value: the unit a prelude and a declaration value are made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentValue {
    /// Any token that is not the start of a function or a block.
    Token(Token),
    /// `rgb(0 0 0)`, `var(--x)`, `url("a.png")`.
    Function(Function),
    /// `( … )`, `[ … ]` or `{ … }` appearing inside a value or a prelude — an
    /// attribute selector's brackets, a media query's parentheses.
    Block(SimpleBlock),
}

impl ComponentValue {
    /// The token, if this is one.
    pub fn token(&self) -> Option<&Token> {
        match self {
            ComponentValue::Token(token) => Some(token),
            _ => None,
        }
    }

    /// Whether this is whitespace or a comment.
    pub fn is_trivia(&self) -> bool {
        self.token().is_some_and(Token::is_trivia)
    }
}

/// `name( arguments )`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    /// The function token, trailing `(` included.
    pub name: Token,
    pub arguments: Vec<ComponentValue>,
    /// Whether a `)` was actually there. An unclosed function at end of input
    /// is legal-ish CSS and must print back the way it arrived.
    pub closed: bool,
}

impl Function {
    /// The function's name, lowercased and without the `(`.
    pub fn name(&self) -> String {
        self.name.name()
    }
}

/// `( … )`, `[ … ]`, `{ … }` in a value or prelude position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleBlock {
    /// The opening token, which decides the closing one.
    pub open: Token,
    pub items: Vec<ComponentValue>,
    pub closed: bool,
}

impl SimpleBlock {
    /// The character that closes this block.
    pub fn close(&self) -> &'static str {
        match self.open.kind {
            Kind::OpenParen => ")",
            Kind::OpenSquare => "]",
            _ => "}",
        }
    }
}
