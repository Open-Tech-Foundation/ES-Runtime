//! Test tags: `--tags-filter` expressions, tag names, and `@module-tag`
//! (DECISIONS D113).
//!
//! An expression is parsed here, once, by the process that was given it, so a
//! mistake in one is reported before any file runs. Each test file's process
//! receives the parsed tree and matches it against every test's tags.

use serde_json::{Value as Json, json};

/// A parsed `--tags-filter`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// A tag name, `*` matching any run of characters.
    Tag(String),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

impl Expr {
    /// As `runtime:test` evaluates it.
    pub fn to_json(&self) -> Json {
        match self {
            Expr::Tag(pattern) => json!({ "tag": pattern }),
            Expr::Not(inner) => json!({ "not": inner.to_json() }),
            Expr::And(a, b) => json!({ "and": [a.to_json(), b.to_json()] }),
            Expr::Or(a, b) => json!({ "or": [a.to_json(), b.to_json()] }),
        }
    }
}

/// Characters a tag name cannot hold: the expression syntax's own.
const RESERVED_CHARS: &[char] = &['(', ')', '&', '|', '!', '*'];

/// Whether `name` can be a tag: not a keyword, and none of the characters an
/// expression is written with.
pub fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("a tag needs a name".to_string());
    }
    if ["and", "or", "not"].contains(&name.to_ascii_lowercase().as_str()) {
        return Err(format!(
            "`{name}` cannot be a tag: and, or and not are how a filter combines tags"
        ));
    }
    if let Some(c) = name
        .chars()
        .find(|c| c.is_whitespace() || RESERVED_CHARS.contains(c))
    {
        return Err(format!(
            "`{name}` cannot be a tag: `{c}` is part of the filter syntax, as are spaces"
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Open,
    Close,
    And,
    Or,
    Not,
    Word(String),
}

fn tokens(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            c if c.is_whitespace() => {
                chars.next();
            }
            '(' => {
                chars.next();
                out.push(Token::Open);
            }
            ')' => {
                chars.next();
                out.push(Token::Close);
            }
            '!' => {
                chars.next();
                out.push(Token::Not);
            }
            '&' | '|' => {
                chars.next();
                // `&&` and `||`; a lone `&` or `|` reads the same, rather than
                // becoming part of a tag it cannot be.
                if chars.peek() == Some(&c) {
                    chars.next();
                }
                out.push(if c == '&' { Token::And } else { Token::Or });
            }
            _ => {
                let mut word = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || "()!&|".contains(c) {
                        break;
                    }
                    word.push(c);
                    chars.next();
                }
                out.push(match word.to_ascii_lowercase().as_str() {
                    "and" => Token::And,
                    "or" => Token::Or,
                    "not" => Token::Not,
                    _ => Token::Word(word),
                });
            }
        }
    }
    out
}

/// Parses a `--tags-filter` expression: `and`/`&&` binds tighter than
/// `or`/`||`, `not`/`!` tighter still, and parentheses group.
pub fn parse(text: &str) -> Result<Expr, String> {
    let tokens = tokens(text);
    let mut at = 0;
    let expr = or(&tokens, &mut at, text)?;
    if let Some(extra) = tokens.get(at) {
        return Err(match extra {
            Token::Close => format!("--tags-filter={text}: a `)` closes nothing"),
            _ => format!("--tags-filter={text}: two tags need `and` or `or` between them"),
        });
    }
    Ok(expr)
}

fn or(tokens: &[Token], at: &mut usize, text: &str) -> Result<Expr, String> {
    let mut left = and(tokens, at, text)?;
    while tokens.get(*at) == Some(&Token::Or) {
        *at += 1;
        left = Expr::Or(Box::new(left), Box::new(and(tokens, at, text)?));
    }
    Ok(left)
}

fn and(tokens: &[Token], at: &mut usize, text: &str) -> Result<Expr, String> {
    let mut left = unary(tokens, at, text)?;
    while tokens.get(*at) == Some(&Token::And) {
        *at += 1;
        left = Expr::And(Box::new(left), Box::new(unary(tokens, at, text)?));
    }
    Ok(left)
}

fn unary(tokens: &[Token], at: &mut usize, text: &str) -> Result<Expr, String> {
    match tokens.get(*at) {
        Some(Token::Not) => {
            *at += 1;
            Ok(Expr::Not(Box::new(unary(tokens, at, text)?)))
        }
        Some(Token::Open) => {
            *at += 1;
            let inner = or(tokens, at, text)?;
            if tokens.get(*at) != Some(&Token::Close) {
                return Err(format!("--tags-filter={text}: a `(` is never closed"));
            }
            *at += 1;
            Ok(inner)
        }
        Some(Token::Word(word)) => {
            *at += 1;
            Ok(Expr::Tag(word.clone()))
        }
        _ => Err(format!(
            "--tags-filter={text}: expected a tag{}",
            if tokens.is_empty() {
                ""
            } else {
                " where the expression ends or an operator stands"
            }
        )),
    }
}

/// The tags a file gives every test in it: each `@module-tag <name>` in its
/// `/** … */` comments.
pub fn module_tags(source: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let mut rest = source;
    while let Some(open) = rest.find("/**") {
        let after = &rest[open + 3..];
        let Some(close) = after.find("*/") else {
            break;
        };
        for line in after[..close].lines() {
            let line = line.trim().trim_start_matches('*').trim();
            if let Some(rest) = line.strip_prefix("@module-tag")
                && let Some(name) = rest.split_whitespace().next()
                && !tags.iter().any(|tag| tag == name)
            {
                tags.push(name.to_string());
            }
        }
        rest = &after[close + 2..];
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(name: &str) -> Box<Expr> {
        Box::new(Expr::Tag(name.to_string()))
    }

    #[test]
    fn precedence_is_not_then_and_then_or() {
        assert_eq!(
            parse("a || b && !c").unwrap(),
            Expr::Or(
                tag("a"),
                Box::new(Expr::And(tag("b"), Box::new(Expr::Not(tag("c")))))
            )
        );
        assert_eq!(
            parse("(unit or e2e) and not slow").unwrap(),
            Expr::And(
                Box::new(Expr::Or(tag("unit"), tag("e2e"))),
                Box::new(Expr::Not(tag("slow")))
            )
        );
        assert_eq!(parse("api/*").unwrap(), Expr::Tag("api/*".to_string()));
        assert_eq!(parse("NOT flaky").unwrap(), Expr::Not(tag("flaky")));
    }

    #[test]
    fn a_broken_expression_says_what_is_wrong() {
        assert!(parse("a b").unwrap_err().contains("need `and` or `or`"));
        assert!(parse("(a or b").unwrap_err().contains("never closed"));
        assert!(parse("a)").unwrap_err().contains("closes nothing"));
        assert!(parse("a and").unwrap_err().contains("expected a tag"));
        assert!(parse("").unwrap_err().contains("expected a tag"));
    }

    #[test]
    fn the_tree_is_what_runtime_test_reads() {
        assert_eq!(
            parse("db && !flaky").unwrap().to_json(),
            json!({ "and": [{ "tag": "db" }, { "not": { "tag": "flaky" } }] })
        );
    }

    #[test]
    fn a_tag_name_cannot_be_syntax() {
        assert!(check_name("db").is_ok());
        assert!(check_name("unit/components").is_ok());
        assert!(check_name("And").unwrap_err().contains("combines tags"));
        assert!(check_name("a b").unwrap_err().contains("spaces"));
        assert!(check_name("a*").unwrap_err().contains("`*`"));
    }

    #[test]
    fn module_tags_come_from_jsdoc_anywhere_in_the_file() {
        let source = "/**\n * Auth tests\n * @module-tag admin/pages\n * @module-tag acceptance\n */\n\
                      test('x', () => {});\n// @module-tag not-jsdoc\n/** @module-tag db */\n";
        assert_eq!(module_tags(source), ["admin/pages", "acceptance", "db"]);
    }
}
