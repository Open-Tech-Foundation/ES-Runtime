//! Encodes a stylesheet as the flat records the test DOM's cascade consumes.
//!
//! The CSS parser is the build pipeline's own (`crate::css`), so a stylesheet
//! means the same thing to `esdev build` and to `esdev test --dom`. It is
//! error-tolerant by design — CSS has no parse errors, only rules a browser
//! drops — so this cannot fail, and a rule nothing understands arrives as a
//! record the cascade ignores rather than as an exception.

use es_runtime_cli_common::Value;

use crate::css::ast::{Block, BlockItem, ComponentValue, Item, Rule, Stylesheet};
use crate::css::parse::parse;
use crate::css::print::value_text;

/// A style rule: `[0, selectorText, declarations, []]`.
const STYLE_RULE: f64 = 0.0;
/// A condition group — `@media`, `@supports`, `@layer` with a block, `@scope`:
/// `[1, name, conditionText, childRecords]`.
const GROUP_RULE: f64 = 1.0;
/// Anything else with a name: `[2, name, preludeText, []]`. The cascade ignores
/// these; they exist so `cssRules` can still report them.
const OTHER_RULE: f64 = 2.0;

/// The at-rules whose bodies hold style rules that apply when a condition
/// holds. `@keyframes`'s body holds keyframes, not style rules, so it is not
/// one of these.
fn is_condition_group(name: &str) -> bool {
    matches!(name, "media" | "supports" | "layer" | "scope" | "container")
}

pub fn stylesheet_records(source: &str) -> Value {
    let sheet: Stylesheet = parse(source);
    let mut records = Vec::new();
    for item in &sheet.items {
        if let Item::Rule(rule) = item {
            encode_rule(rule, None, &mut records);
        }
    }
    Value::Array(records)
}

fn encode_rule(rule: &Rule, parent_selector: Option<&str>, out: &mut Vec<Value>) {
    match rule {
        Rule::Qualified(qualified) => {
            let written = prelude_text(&qualified.prelude);
            let selector = match parent_selector {
                Some(parent) => nest(parent, &written),
                None => written,
            };
            out.push(Value::Array(vec![
                Value::Number(STYLE_RULE),
                Value::String(selector.clone()),
                declarations(&qualified.block),
                Value::Array(Vec::new()),
            ]));
            // Nesting is flattened: a nested rule becomes a rule of its own
            // whose selector names its parent through `:is()`, which is the
            // specificity the specification gives it.
            for item in &qualified.block.items {
                if let BlockItem::Rule(nested) = item {
                    encode_rule(nested, Some(&selector), out);
                }
            }
        }
        Rule::At(at) => {
            let name = at.name();
            let prelude = prelude_text(&at.prelude);
            let Some(block) = &at.block else {
                out.push(Value::Array(vec![
                    Value::Number(OTHER_RULE),
                    Value::String(name),
                    Value::String(prelude),
                    Value::Array(Vec::new()),
                ]));
                return;
            };
            if !is_condition_group(&name) {
                out.push(Value::Array(vec![
                    Value::Number(OTHER_RULE),
                    Value::String(name),
                    Value::String(prelude),
                    Value::Array(Vec::new()),
                ]));
                return;
            }
            let mut children = Vec::new();
            for item in &block.items {
                if let BlockItem::Rule(inner) = item {
                    encode_rule(inner, parent_selector, &mut children);
                }
            }
            // A conditional group inside a style rule can hold bare
            // declarations, which belong to the parent selector under the
            // condition: `.a { @media print { color: red } }`.
            if let Some(parent) = parent_selector {
                let inner = declarations(block);
                if !matches!(&inner, Value::Array(list) if list.is_empty()) {
                    children.push(Value::Array(vec![
                        Value::Number(STYLE_RULE),
                        Value::String(parent.to_string()),
                        inner,
                        Value::Array(Vec::new()),
                    ]));
                }
            }
            out.push(Value::Array(vec![
                Value::Number(GROUP_RULE),
                Value::String(name),
                Value::String(prelude),
                Value::Array(children),
            ]));
        }
    }
}

/// `&` names the parent selector, and a nested selector with no `&` is a
/// descendant of it. `:is()` is what the specification wraps the parent in, so
/// the nested rule's specificity is the parent's most specific selector.
fn nest(parent: &str, nested: &str) -> String {
    let wrapped = format!(":is({parent})");
    if nested.contains('&') {
        return nested.replace('&', &wrapped);
    }
    format!("{wrapped} {nested}")
}

fn prelude_text(prelude: &[ComponentValue]) -> String {
    prelude
        .iter()
        .map(value_text)
        .collect::<String>()
        .trim()
        .to_string()
}

fn declarations(block: &Block) -> Value {
    let mut out = Vec::new();
    for item in &block.items {
        let BlockItem::Declaration(declaration) = item else {
            continue;
        };
        let important = declaration.is_important();
        let text: String = declaration.value.iter().map(value_text).collect();
        let value = if important {
            strip_important(&text)
        } else {
            text.trim().to_string()
        };
        if value.is_empty() {
            continue;
        }
        out.push(Value::Array(vec![
            // Not lowercased: a custom property is case-sensitive, and the
            // cascade lowercases the rest where it matters.
            Value::String(declaration.name.text.clone()),
            Value::String(value),
            Value::Bool(important),
        ]));
    }
    Value::Array(out)
}

/// Removes the trailing `!important` a value ends with, leaving the value.
fn strip_important(text: &str) -> String {
    let trimmed = text.trim_end();
    let cut = trimmed.len() - "important".len();
    let head = trimmed[..cut].trim_end();
    head.strip_suffix('!').unwrap_or(head).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records(source: &str) -> String {
        format!("{:?}", stylesheet_records(source))
    }

    #[test]
    fn encodes_style_rules_with_their_declarations() {
        let encoded = records("a, .b > c { color: red; margin: 0 auto !important }");
        assert!(encoded.contains("a, .b > c"), "{encoded}");
        assert!(encoded.contains("color"), "{encoded}");
        assert!(encoded.contains("red"), "{encoded}");
        assert!(encoded.contains("0 auto"), "{encoded}");
        assert!(!encoded.contains("important\""), "{encoded}");
    }

    #[test]
    fn keeps_a_condition_group_and_its_rules() {
        let encoded = records("@media (min-width: 40em) { .a { color: red } }");
        assert!(encoded.contains("media"), "{encoded}");
        assert!(encoded.contains("(min-width: 40em)"), "{encoded}");
        assert!(encoded.contains(".a"), "{encoded}");
    }

    #[test]
    fn flattens_nesting_through_is() {
        let encoded = records(".card { color: red; & a { color: blue } b { color: green } }");
        assert!(encoded.contains(":is(.card) a"), "{encoded}");
        assert!(encoded.contains(":is(.card) b"), "{encoded}");
    }

    #[test]
    fn reports_an_at_rule_it_does_not_interpret() {
        let encoded = records("@keyframes spin { from { opacity: 0 } } @import \"a.css\";");
        assert!(encoded.contains("keyframes"), "{encoded}");
        assert!(encoded.contains("import"), "{encoded}");
    }
}
