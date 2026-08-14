//! [CSS Syntax Level 3 §5][spec] — tokens into a tree.
//!
//! # It does not fail
//!
//! There is no `Result` here. CSS has no parse error that stops a browser: a
//! rule it cannot understand is skipped and the rest of the sheet still
//! applies, which is the whole reason a stylesheet written for a browser that
//! does not exist yet works in one that does. A parser that refused input would
//! be stricter than the thing it exists to feed, and would reject stylesheets
//! that render correctly today.
//!
//! Malformed input is *represented* instead: an unclosed block is a
//! [`SimpleBlock`] with `closed: false`, a bad string keeps its
//! [`Kind::BadString`], and both print back exactly as written.
//!
//! # Declaration or nested rule
//!
//! The one genuinely ambiguous decision inside a block. `color: red` is a
//! declaration; `a:hover { … }` is a nested rule; both begin with an identifier
//! followed by a colon. The spec resolves it by trying a declaration and
//! rewinding on failure.
//!
//! This looks ahead instead, which is the same answer without the rewind: scan
//! from the current token to the first `;`, `{` or `}` that is not inside a
//! nested block. A `{` means a rule, anything else means a declaration. `;` and
//! `}` cannot appear unnested inside a selector, and a declaration's value
//! cannot contain an unnested `{`, so the two cases never overlap.
//!
//! [spec]: https://www.w3.org/TR/css-syntax-3/#parsing

use super::ast::*;
use super::token::{Kind, Token, tokenize};

/// Parses a stylesheet.
pub fn parse(source: &str) -> Stylesheet {
    let tokens = tokenize(source);
    let mut parser = Parser { tokens, at: 0 };
    Stylesheet {
        items: parser.rules(None),
    }
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.at).cloned();
        if token.is_some() {
            self.at += 1;
        }
        token
    }

    /// A list of rules. `stop` is `Some(Kind::CloseCurly)` when parsing the
    /// inside of a block, `None` at the top level.
    fn rules(&mut self, stop: Option<Kind>) -> Vec<Item> {
        let mut items = Vec::new();
        while let Some(token) = self.peek() {
            if Some(token.kind) == stop {
                break;
            }
            match token.kind {
                // `<!--` and `-->` are legal at the top level and mean nothing;
                // kept as trivia so they survive a round trip.
                Kind::Whitespace | Kind::Comment | Kind::Cdo | Kind::Cdc => {
                    let token = self.next().expect("peeked");
                    items.push(Item::Trivia(token));
                }
                Kind::AtKeyword => {
                    let rule = self.at_rule();
                    items.push(Item::Rule(Rule::At(rule)));
                }
                _ => match self.qualified_rule() {
                    Ok(rule) => items.push(Item::Rule(Rule::Qualified(rule))),
                    // A prelude that ran to end of input with no block. There is
                    // no rule to make of it; the tokens are kept so they print
                    // back, and there is nothing left to read.
                    Err(dangling) => {
                        items.push(Item::Dangling(dangling));
                        break;
                    }
                },
            }
        }
        items
    }

    /// `@name <prelude> ;` or `@name <prelude> { … }`.
    fn at_rule(&mut self) -> AtRule {
        let at = self.next().expect("an at-keyword");
        let mut prelude = Vec::new();

        loop {
            match self.peek().map(|t| t.kind) {
                None => break,
                Some(Kind::Semicolon) => {
                    self.at += 1;
                    return AtRule {
                        at,
                        prelude,
                        block: None,
                        semicolon: true,
                    };
                }
                Some(Kind::OpenCurly) => {
                    let block = self.block();
                    return AtRule {
                        at,
                        prelude,
                        block: Some(block),
                        semicolon: false,
                    };
                }
                Some(_) => prelude.push(self.component_value()),
            }
        }

        AtRule {
            at,
            prelude,
            block: None,
            semicolon: false,
        }
    }

    /// `<prelude> { … }`.
    ///
    /// `Err` carries the prelude back when the input ended before a block, so
    /// the caller can keep the tokens rather than lose them.
    fn qualified_rule(&mut self) -> Result<QualifiedRule, Vec<ComponentValue>> {
        let mut prelude = Vec::new();
        loop {
            match self.peek().map(|t| t.kind) {
                None => return Err(prelude),
                Some(Kind::OpenCurly) => {
                    let block = self.block();
                    return Ok(QualifiedRule { prelude, block });
                }
                Some(_) => prelude.push(self.component_value()),
            }
        }
    }

    /// The `{ … }` of a rule: declarations, nested rules, or both.
    fn block(&mut self) -> Block {
        self.at += 1; // the `{`
        let mut items = Vec::new();
        let mut closed = false;

        while let Some(token) = self.peek() {
            match token.kind {
                Kind::CloseCurly => {
                    self.at += 1;
                    closed = true;
                    break;
                }
                Kind::Whitespace | Kind::Comment => {
                    let token = self.next().expect("peeked");
                    items.push(BlockItem::Trivia(token));
                }
                Kind::Semicolon => {
                    self.at += 1;
                    items.push(BlockItem::Semicolon);
                }
                Kind::AtKeyword => {
                    let rule = self.at_rule();
                    items.push(BlockItem::Rule(Rule::At(rule)));
                }
                _ => {
                    if self.declaration_ahead() {
                        match self.declaration() {
                            Some(declaration) => {
                                items.push(BlockItem::Declaration(declaration));
                            }
                            // Something that looked like a declaration and was
                            // not — `;;` or a stray token. Keep it verbatim.
                            None => {
                                let token = self.next().expect("peeked");
                                items.push(BlockItem::Trivia(token));
                            }
                        }
                    } else {
                        match self.qualified_rule() {
                            Ok(rule) => items.push(BlockItem::Rule(Rule::Qualified(rule))),
                            Err(dangling) => {
                                items.push(BlockItem::Dangling(dangling));
                                break;
                            }
                        }
                    }
                }
            }
        }

        Block { items, closed }
    }

    /// Whether what follows is a declaration rather than a nested rule.
    ///
    /// See the module docs: the first unnested `;`, `{` or `}` decides.
    fn declaration_ahead(&self) -> bool {
        let mut depth = 0usize;
        for token in &self.tokens[self.at..] {
            match token.kind {
                Kind::OpenParen | Kind::OpenSquare => depth += 1,
                Kind::CloseParen | Kind::CloseSquare => depth = depth.saturating_sub(1),
                Kind::Function => depth += 1,
                Kind::OpenCurly if depth == 0 => return false,
                Kind::Semicolon | Kind::CloseCurly if depth == 0 => return true,
                _ => {}
            }
        }
        // End of input without either: an unterminated declaration is the more
        // useful reading, since `a { color: red` is a missing brace and not a
        // selector.
        true
    }

    /// `name : value`, up to but not including the `;` or `}`.
    fn declaration(&mut self) -> Option<Declaration> {
        let start = self.at;
        let name = self.next()?;
        if !matches!(name.kind, Kind::Ident) {
            self.at = start;
            return None;
        }

        let mut before_colon = Vec::new();
        while self.peek().is_some_and(Token::is_trivia) {
            before_colon.push(self.next().expect("peeked"));
        }

        if self.peek().map(|t| t.kind) != Some(Kind::Colon) {
            self.at = start;
            return None;
        }
        self.at += 1; // the `:`

        let mut value = Vec::new();
        while let Some(kind) = self.peek().map(|t| t.kind) {
            if matches!(kind, Kind::Semicolon | Kind::CloseCurly) {
                break;
            }
            value.push(self.component_value());
        }

        Some(Declaration {
            name,
            before_colon,
            value,
        })
    }

    /// One component value: a block, a function, or a plain token.
    fn component_value(&mut self) -> ComponentValue {
        let token = self.next().expect("a token");
        match token.kind {
            Kind::OpenParen | Kind::OpenSquare | Kind::OpenCurly => {
                let closer = match token.kind {
                    Kind::OpenParen => Kind::CloseParen,
                    Kind::OpenSquare => Kind::CloseSquare,
                    _ => Kind::CloseCurly,
                };
                let mut items = Vec::new();
                let mut closed = false;
                while let Some(kind) = self.peek().map(|t| t.kind) {
                    if kind == closer {
                        self.at += 1;
                        closed = true;
                        break;
                    }
                    items.push(self.component_value());
                }
                ComponentValue::Block(SimpleBlock {
                    open: token,
                    items,
                    closed,
                })
            }
            Kind::Function => {
                let mut arguments = Vec::new();
                let mut closed = false;
                while let Some(kind) = self.peek().map(|t| t.kind) {
                    if kind == Kind::CloseParen {
                        self.at += 1;
                        closed = true;
                        break;
                    }
                    arguments.push(self.component_value());
                }
                ComponentValue::Function(Function {
                    name: token,
                    arguments,
                    closed,
                })
            }
            _ => ComponentValue::Token(token),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::print::print;

    #[test]
    fn a_style_rule_has_a_prelude_and_declarations() {
        let sheet = parse("body { color: red; margin: 0 }");
        let rules: Vec<_> = sheet.rules().collect();
        assert_eq!(rules.len(), 1);

        let Rule::Qualified(rule) = rules[0] else {
            panic!("expected a qualified rule");
        };
        let declarations: Vec<_> = rule
            .block
            .items
            .iter()
            .filter_map(|item| match item {
                BlockItem::Declaration(d) => Some(d.name.text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(declarations, ["color", "margin"]);
    }

    #[test]
    fn a_statement_at_rule_ends_at_its_semicolon() {
        let sheet = parse("@import \"a.css\" screen;");
        let Some(Rule::At(rule)) = sheet.rules().next() else {
            panic!("expected an at-rule");
        };
        assert_eq!(rule.name(), "import");
        assert!(rule.block.is_none());
        assert!(rule.prelude.iter().any(|v| matches!(
            v, ComponentValue::Token(t) if t.kind == Kind::String
        )));
    }

    #[test]
    fn a_block_at_rule_holds_rules() {
        let sheet = parse("@media print { a { color: #000 } }");
        let Some(Rule::At(rule)) = sheet.rules().next() else {
            panic!("expected an at-rule");
        };
        assert_eq!(rule.name(), "media");
        let block = rule.block.as_ref().expect("a block");
        assert_eq!(
            block
                .items
                .iter()
                .filter(|i| matches!(i, BlockItem::Rule(_)))
                .count(),
            1
        );
    }

    /// The ambiguity the lookahead exists for. Both start `ident :`.
    #[test]
    fn a_declaration_and_a_nested_rule_are_told_apart() {
        let sheet = parse(".card { color: red; a:hover { color: blue } }");
        let Some(Rule::Qualified(rule)) = sheet.rules().next() else {
            panic!("expected a rule");
        };

        let declarations = rule
            .block
            .items
            .iter()
            .filter(|i| matches!(i, BlockItem::Declaration(_)))
            .count();
        let nested = rule
            .block
            .items
            .iter()
            .filter(|i| matches!(i, BlockItem::Rule(_)))
            .count();
        assert_eq!((declarations, nested), (1, 1));
    }

    /// CSS Nesting with `&`, which is the form the template's own stylesheet
    /// uses.
    #[test]
    fn nesting_with_an_ampersand_parses_as_a_nested_rule() {
        let sheet = parse("nav { gap: 1rem; & a { color: red; &:hover { color: blue } } }");
        let Some(Rule::Qualified(rule)) = sheet.rules().next() else {
            panic!("expected a rule");
        };
        let Some(BlockItem::Rule(Rule::Qualified(inner))) = rule
            .block
            .items
            .iter()
            .find(|i| matches!(i, BlockItem::Rule(_)))
        else {
            panic!("expected a nested rule");
        };
        // …and the nesting goes deeper than one level.
        assert!(
            inner
                .block
                .items
                .iter()
                .any(|i| matches!(i, BlockItem::Rule(_)))
        );
    }

    #[test]
    fn a_function_holds_its_arguments() {
        let sheet = parse("a { color: color-mix(in oklab, red 40%, blue) }");
        let Some(Rule::Qualified(rule)) = sheet.rules().next() else {
            panic!("expected a rule");
        };
        let BlockItem::Declaration(declaration) = &rule.block.items[1] else {
            panic!("expected a declaration, got {:?}", rule.block.items[1]);
        };
        let function = declaration
            .value
            .iter()
            .find_map(|v| match v {
                ComponentValue::Function(f) => Some(f),
                _ => None,
            })
            .expect("a function");
        assert_eq!(function.name(), "color-mix");
        assert!(function.closed);
    }

    #[test]
    fn important_is_recognised_without_being_special_cased_in_the_grammar() {
        let sheet = parse("a { color: red !important; margin: 0 }");
        let Some(Rule::Qualified(rule)) = sheet.rules().next() else {
            panic!("expected a rule");
        };
        let declarations: Vec<_> = rule
            .block
            .items
            .iter()
            .filter_map(|i| match i {
                BlockItem::Declaration(d) => Some(d),
                _ => None,
            })
            .collect();
        assert!(declarations[0].is_important());
        assert!(!declarations[1].is_important());
    }

    /// Malformed input is represented, not rejected — and prints back as it
    /// arrived.
    #[test]
    fn unterminated_input_is_represented_rather_than_refused() {
        for source in [
            "a { color: red",
            "a { color: rgb(1, 2",
            "@media (min-width: 0",
            "a { content: \"oops",
            "a[href=\"x\" { color: red }",
        ] {
            let sheet = parse(source);
            assert_eq!(print(&sheet), source, "did not round-trip: {source:?}");
        }
    }

    /// A custom property's name is case-sensitive, unlike every other name in
    /// CSS. Lowercasing it here would silently rename it.
    #[test]
    fn a_custom_propertys_case_is_preserved() {
        let sheet = parse(":root { --Brand-Ink: #111 }");
        let Some(Rule::Qualified(rule)) = sheet.rules().next() else {
            panic!("expected a rule");
        };
        let BlockItem::Declaration(declaration) = &rule.block.items[1] else {
            panic!("expected a declaration");
        };
        assert_eq!(declaration.name.text, "--Brand-Ink");
    }
}
