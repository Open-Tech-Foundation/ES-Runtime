//! Conservative value-level CSS minification.
//!
//! The parser deliberately does not know every property's grammar.  These
//! rewrites are therefore limited to values whose spelling is independently
//! safe: six-digit colours, zero dimensions outside math, repeated shorthand
//! sides, and overridden adjacent declarations.

use super::ast::*;
use super::token::Kind;

pub fn apply(sheet: &mut Stylesheet) {
    for item in &mut sheet.items {
        if let Item::Rule(rule) = item {
            rule_apply(rule);
        }
    }
}

fn rule_apply(rule: &mut Rule) {
    let block = match rule {
        Rule::At(rule) => rule.block.as_mut(),
        Rule::Qualified(rule) => Some(&mut rule.block),
    };
    if let Some(block) = block {
        block_apply(block);
    }
}

fn block_apply(block: &mut Block) {
    for item in &mut block.items {
        match item {
            BlockItem::Declaration(declaration) => declaration_apply(declaration),
            BlockItem::Rule(rule) => rule_apply(rule),
            BlockItem::Trivia(_) | BlockItem::Semicolon | BlockItem::Dangling(_) => {}
        }
    }

    // Duplicate declarations are exceptionally common after a CSS generator
    // has emitted a fallback. Only discard one when cascade order proves the
    // later declaration wins. `all` is the one declaration that can reset an
    // unrelated property, so it ends the otherwise independent runs.
    let mut previous: Vec<(usize, String, bool)> = Vec::new();
    let mut remove = Vec::new();
    for (index, item) in block.items.iter().enumerate() {
        match item {
            BlockItem::Declaration(declaration) => {
                let name = declaration.name.name();
                let important = declaration.is_important();
                if name == "all" {
                    previous.clear();
                    continue;
                }
                if let Some((old, _, old_important)) =
                    previous.iter().find(|(_, old_name, _)| *old_name == name)
                    && (!*old_important || important)
                {
                    remove.push(*old);
                }
                previous.retain(|(_, old_name, _)| *old_name != name);
                previous.push((index, name, important));
            }
            BlockItem::Trivia(_) | BlockItem::Semicolon => {}
            BlockItem::Rule(_) | BlockItem::Dangling(_) => previous.clear(),
        }
    }
    for index in remove.into_iter().rev() {
        block.items.remove(index);
    }
}

fn declaration_apply(declaration: &mut Declaration) {
    // Custom-property values are token streams by definition.  Changing a
    // unit or a colour inside one changes what a later var() consumer sees.
    if declaration.name.text.starts_with("--") {
        return;
    }
    values_apply(&mut declaration.value, false);
    collapse_sides(declaration);
}

fn values_apply(values: &mut [ComponentValue], in_math: bool) {
    for value in values {
        match value {
            ComponentValue::Token(token) => {
                if token.kind == Kind::Hash {
                    let hex = token.text.strip_prefix('#').unwrap_or(&token.text);
                    if hex.len() == 6
                        && hex.as_bytes()[0].eq_ignore_ascii_case(&hex.as_bytes()[1])
                        && hex.as_bytes()[2].eq_ignore_ascii_case(&hex.as_bytes()[3])
                        && hex.as_bytes()[4].eq_ignore_ascii_case(&hex.as_bytes()[5])
                    {
                        token.text =
                            format!("#{}{}{}", &hex[0..1], &hex[2..3], &hex[4..5]).to_lowercase();
                    }
                }
                if !in_math && token.kind == Kind::Dimension && is_zero_dimension(&token.text) {
                    token.kind = Kind::Number;
                    token.text = "0".to_string();
                }
            }
            ComponentValue::Function(function) => {
                let math = matches!(function.name().as_str(), "calc" | "min" | "max" | "clamp");
                values_apply(&mut function.arguments, in_math || math);
            }
            ComponentValue::Block(block) => values_apply(&mut block.items, in_math),
        }
    }
}

fn is_zero_dimension(text: &str) -> bool {
    let at = text
        .char_indices()
        .find_map(|(at, c)| c.is_ascii_alphabetic().then_some(at));
    let Some(at) = at else { return false };
    text[..at].parse::<f64>().is_ok_and(|value| value == 0.0)
}

fn collapse_sides(declaration: &mut Declaration) {
    const SIDES: &[&str] = &[
        "margin",
        "padding",
        "border-width",
        "border-color",
        "border-style",
    ];
    if !SIDES.contains(&declaration.name.name().as_str()) {
        return;
    }
    let values: Vec<usize> = declaration
        .value
        .iter()
        .enumerate()
        .filter_map(|(i, value)| (!value.is_trivia()).then_some(i))
        .collect();
    if values.len() != 4 {
        return;
    }
    let text = |i: usize| match &declaration.value[i] {
        ComponentValue::Token(token) => Some(token.text.as_str()),
        _ => None,
    };
    let Some((a, b, c, d)) = text(values[0])
        .zip(text(values[1]))
        .zip(text(values[2]))
        .zip(text(values[3]))
        .map(|(((a, b), c), d)| (a, b, c, d))
    else {
        return;
    };
    let keep = if a == b && a == c && a == d {
        1
    } else if a == c && b == d {
        2
    } else if b == d {
        3
    } else {
        4
    };
    if keep < 4 {
        let mut seen = 0;
        declaration.value.retain(|value| {
            if value.is_trivia() {
                return seen < keep;
            }
            seen += 1;
            seen <= keep
        });
    }
}
