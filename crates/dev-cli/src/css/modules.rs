//! CSS Modules: scoping a stylesheet's names to the file that wrote them.
//!
//! `.button` in `Button.module.css` becomes `.button_a1b2c3d4`, and the
//! JavaScript that imported the file receives `{ button: "button_a1b2c3d4" }`.
//! Two components can both call a class `.button` and neither wins, which is
//! the entire point: CSS has one global namespace and a component tree does
//! not.
//!
//! This module does the **CSS half** — rewrite the names, report the mapping.
//! The other half is making `import styles from "./x.module.css"` resolve to
//! that mapping, which is a bundler concern and lives in [`crate::build`].
//!
//! # What is scoped
//!
//! * **Class selectors** — `.button`.
//! * **Id selectors** — `#panel`. Scoped for the same reason as classes, and
//!   the reason it surprises people is that ids in a component are usually a
//!   mistake to begin with.
//! * **`@keyframes` names**, and the `animation` / `animation-name` values that
//!   refer to them. Both halves or neither: a renamed `@keyframes` with an
//!   un-renamed reference is an animation that silently stops running.
//!
//! # What is not
//!
//! Element selectors (`div`), pseudo-classes (`:hover`), pseudo-elements
//! (`::before`), attribute selectors, custom properties and `@media` features.
//! None of them is a name this file owns.
//!
//! **`:global(…)`** is the deliberate opt-out — everything inside it is left
//! alone, and the wrapper itself is **removed**, so `:global(.a) .b` reaches
//! the browser as `.a .b_hash`. It is how a module reaches a class name
//! somebody else defined.
//! The bare switch form (`:global .a .b`, scoping everything after it) is not
//! supported; the functional form is unambiguous and is what tooling has
//! standardised on.
//!
//! **`composes`** is refused rather than ignored. It resolves a name from
//! *another* module, which needs a dependency graph this pass does not have,
//! and silently dropping it produces an element missing half its styling with
//! nothing to show why.
//!
//! # The hash is over the path, not the contents
//!
//! So editing a component does not rename its classes. That keeps a rebuild
//! from invalidating every reference to a name, and — because the path is taken
//! relative to the project root — keeps two machines building the same commit
//! to the same class names.

use std::collections::{BTreeMap, HashSet};

use super::ast::*;
use super::token::{Kind, Token};

/// A stylesheet with its names scoped, and the mapping JavaScript needs.
#[derive(Debug)]
pub struct Scoped {
    pub sheet: Stylesheet,
    /// Local name to scoped name, in a stable order so the generated module is
    /// byte-identical between builds.
    pub names: BTreeMap<String, String>,
}

/// Scopes every name `sheet` owns, deriving them from `ident`.
///
/// `ident` identifies the file — a path relative to the project root — and is
/// the only thing the generated names depend on.
pub fn scope(mut sheet: Stylesheet, ident: &str) -> Result<Scoped, String> {
    let suffix = hash(ident);
    let mut names = BTreeMap::new();

    // Keyframes first: a rule can refer to an animation defined below it, so
    // the set has to be complete before any value is rewritten.
    let mut keyframes = HashSet::new();
    collect_keyframes(&sheet.items, &mut keyframes);

    for item in &mut sheet.items {
        if let Item::Rule(rule) = item {
            rewrite_rule(rule, &suffix, &keyframes, &mut names)?;
        }
    }

    Ok(Scoped { sheet, names })
}

/// The scoped form of a local name.
fn scoped(name: &str, suffix: &str) -> String {
    format!("{name}_{suffix}")
}

/// A short, stable hash of the file's identity.
///
/// FNV-1a. Nothing trusts it and nothing is protected by it — a collision costs
/// two components sharing a class name, which is the situation that existed
/// before CSS Modules — so a cryptographic hash would be cost without benefit.
fn hash(ident: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in ident.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")[..8].to_string()
}

fn collect_keyframes(items: &[Item], into: &mut HashSet<String>) {
    for item in items {
        if let Item::Rule(Rule::At(at)) = item
            && at.name().ends_with("keyframes")
            && let Some(name) = keyframes_name(&at.prelude)
        {
            into.insert(name);
        }
    }
}

/// The name an `@keyframes` prelude declares.
fn keyframes_name(prelude: &[ComponentValue]) -> Option<String> {
    prelude
        .iter()
        .find(|value| !value.is_trivia())
        .and_then(|value| value.token())
        .filter(|token| matches!(token.kind, Kind::Ident))
        .map(|token| token.text.clone())
}

fn rewrite_rule(
    rule: &mut Rule,
    suffix: &str,
    keyframes: &HashSet<String>,
    names: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    match rule {
        Rule::Qualified(qualified) => {
            rewrite_selector(&mut qualified.prelude, suffix, names);
            rewrite_block(&mut qualified.block, suffix, keyframes, names)?;
        }
        Rule::At(at) => {
            // `@keyframes fade` declares a name this file owns.
            if at.name().ends_with("keyframes")
                && let Some(token) = at
                    .prelude
                    .iter_mut()
                    .find(|value| !value.is_trivia())
                    .and_then(|value| match value {
                        ComponentValue::Token(token) if token.kind == Kind::Ident => Some(token),
                        _ => None,
                    })
            {
                let local = token.text.clone();
                let renamed = scoped(&local, suffix);
                names.insert(local, renamed.clone());
                *token = Token::new(Kind::Ident, renamed);
            }
            if let Some(block) = &mut at.block {
                rewrite_block(block, suffix, keyframes, names)?;
            }
        }
    }
    Ok(())
}

fn rewrite_block(
    block: &mut Block,
    suffix: &str,
    keyframes: &HashSet<String>,
    names: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    for item in &mut block.items {
        match item {
            BlockItem::Declaration(declaration) => {
                let property = declaration.name.text.to_ascii_lowercase();
                if property == "composes" {
                    return Err("`composes` is not supported.\n\n\
                         It resolves a class from another module, which needs a \
                         dependency graph this build does not have. Compose in \
                         the markup instead — className={`${a.x} ${b.y}`} — \
                         which is explicit and needs no build step."
                        .to_string());
                }
                // `animation: fade 1s` and `animation-name: fade` name a
                // `@keyframes` this file scoped, so the reference moves with it.
                if property == "animation" || property == "animation-name" {
                    rewrite_animation(&mut declaration.value, suffix, keyframes);
                }
            }
            BlockItem::Rule(rule) => rewrite_rule(rule, suffix, keyframes, names)?,
            BlockItem::Trivia(_) | BlockItem::Semicolon | BlockItem::Dangling(_) => {}
        }
    }
    Ok(())
}

/// Renames any identifier in an animation value that names a scoped keyframes.
///
/// Matched against the collected set rather than by position, because
/// `animation: 1s ease fade` is as legal as `animation: fade 1s ease` and the
/// shorthand's order is not fixed.
fn rewrite_animation(values: &mut [ComponentValue], suffix: &str, keyframes: &HashSet<String>) {
    for value in values {
        if let ComponentValue::Token(token) = value
            && token.kind == Kind::Ident
            && keyframes.contains(&token.text)
        {
            *token = Token::new(Kind::Ident, scoped(&token.text, suffix));
        }
    }
}

/// Rewrites the class and id names in one selector.
///
/// Takes a `Vec` rather than a slice because `:global(…)` is **unwrapped**, not
/// just skipped: `:global(.a) .b` has to reach the browser as `.a .b`, since
/// `:global` is a convention of this build and not a selector any engine knows.
fn rewrite_selector(
    prelude: &mut Vec<ComponentValue>,
    suffix: &str,
    names: &mut BTreeMap<String, String>,
) {
    let mut out: Vec<ComponentValue> = Vec::with_capacity(prelude.len());
    let mut i = 0;

    while i < prelude.len() {
        // `:global(…)` — drop the `:` and the wrapper, keep the contents as
        // they were written. This is how a module reaches a name it does not own.
        if let ComponentValue::Token(colon) = &prelude[i]
            && colon.kind == Kind::Colon
            && let Some(ComponentValue::Function(function)) = prelude.get(i + 1)
            && function.name() == "global"
        {
            out.extend(function.arguments.iter().cloned());
            i += 2;
            continue;
        }

        match &mut prelude[i] {
            // `:is(.a, .b)`, `:not(.c)` — still this file's names.
            ComponentValue::Function(function) => {
                rewrite_selector(&mut function.arguments, suffix, names);
            }
            // An attribute selector holds no class names, and its contents are
            // matched against the *rendered* attribute — which holds the scoped
            // name, not the local one, so rewriting there would be wrong twice
            // over. Any other bracketing is ordinary selector text.
            ComponentValue::Block(block) if block.open.kind != Kind::OpenSquare => {
                rewrite_selector(&mut block.items, suffix, names);
            }
            // `#panel` is a single token.
            ComponentValue::Token(token) if token.kind == Kind::Hash => {
                if let Some(local) = token.text.strip_prefix('#')
                    && is_name(local)
                {
                    let renamed = scoped(local, suffix);
                    names.insert(local.to_string(), renamed.clone());
                    *token = Token::new(Kind::Hash, format!("#{renamed}"));
                }
            }
            _ => {}
        }

        // `.button` is a `.` delimiter and an identifier. The tokenizer has
        // already ruled out `.5em`, which is a number.
        let is_dot = matches!(
            &prelude[i],
            ComponentValue::Token(token) if token.kind == Kind::Delim && token.text == "."
        );
        if is_dot
            && let Some(ComponentValue::Token(next)) = prelude.get(i + 1)
            && next.kind == Kind::Ident
            && is_name(&next.text)
        {
            let local = next.text.clone();
            let renamed = scoped(&local, suffix);
            names.insert(local, renamed.clone());
            out.push(prelude[i].clone());
            out.push(ComponentValue::Token(Token::new(Kind::Ident, renamed)));
            i += 2;
            continue;
        }

        out.push(prelude[i].clone());
        i += 1;
    }

    *prelude = out;
}

/// Whether a name is one this pass should rewrite — an ordinary identifier and
/// not something escaped into looking like one.
fn is_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::{parse::parse, print::print_minified};

    fn scoped_css(source: &str) -> (String, BTreeMap<String, String>) {
        let out = scope(parse(source), "src/Button.module.css").expect("scopes");
        (print_minified(&out.sheet), out.names)
    }

    #[test]
    fn a_class_is_renamed_and_reported() {
        let (css, names) = scoped_css(".button { color: red }");
        let renamed = names.get("button").expect("button was reported");
        assert!(css.contains(&format!(".{renamed}")), "{css}");
        assert!(renamed.starts_with("button_"), "{renamed}");
    }

    /// The whole point: the same local name in two files must not collide.
    #[test]
    fn the_same_name_in_two_files_gets_two_names() {
        let one = scope(parse(".button{color:red}"), "src/Button.module.css").expect("scopes");
        let two = scope(parse(".button{color:blue}"), "src/Card.module.css").expect("scopes");
        assert_ne!(one.names["button"], two.names["button"]);
    }

    /// The hash is over the path, so editing a component does not rename its
    /// classes and two machines agree on the same commit.
    #[test]
    fn the_name_depends_on_the_path_and_not_the_contents() {
        let before = scope(parse(".a{color:red}"), "src/x.module.css").expect("scopes");
        let after = scope(parse(".a{color:blue;margin:0}"), "src/x.module.css").expect("scopes");
        assert_eq!(before.names["a"], after.names["a"]);
    }

    #[test]
    fn ids_are_scoped_and_elements_and_pseudos_are_not() {
        let (css, names) = scoped_css("div#panel:hover::before { color: red }");
        assert!(names.contains_key("panel"), "{names:?}");
        assert!(css.starts_with("div#panel_"), "{css}");
        assert!(css.contains(":hover::before"), "{css}");
        assert!(!names.contains_key("hover"), "a pseudo-class was scoped");
        assert!(!names.contains_key("div"), "an element was scoped");
    }

    /// Both halves of an animation move together, or it silently stops running.
    #[test]
    fn keyframes_and_the_values_that_name_them_move_together() {
        let (css, names) = scoped_css(
            "@keyframes fade { from { opacity: 0 } }\n\
             .a { animation: fade 1s ease }\n\
             .b { animation-name: fade }",
        );
        let renamed = names.get("fade").expect("fade was reported");
        assert!(css.contains(&format!("@keyframes {renamed}")), "{css}");
        assert_eq!(css.matches(renamed.as_str()).count(), 3, "{css}");
        assert!(
            !css.contains("fade 1s"),
            "the reference did not move: {css}"
        );
    }

    /// A rule can use an animation declared below it.
    #[test]
    fn an_animation_declared_after_its_use_is_still_matched() {
        let (css, _) =
            scoped_css(".a { animation-name: fade }\n@keyframes fade { from { opacity: 0 } }");
        assert!(!css.contains("animation-name:fade}"), "{css}");
    }

    /// The opt-out, and how a module reaches a name somebody else defined.
    ///
    /// The wrapper has to *go*: `:global` is a convention of this build, and a
    /// browser handed `:global(.no-js)` matches nothing at all.
    #[test]
    fn global_is_unwrapped_and_its_contents_left_alone() {
        let (css, names) = scoped_css(":global(.no-js) .button { color: red }");
        assert!(!css.contains(":global"), "the wrapper survived: {css}");
        assert!(css.starts_with(".no-js "), "{css}");
        assert!(!names.contains_key("no-js"), "{names:?}");
        assert!(names.contains_key("button"), "{names:?}");
    }

    /// `:is()` and `:not()` hold this file's own selectors, unlike `:global()`.
    #[test]
    fn a_selector_inside_is_or_not_is_still_scoped() {
        let (_, names) = scoped_css(":is(.a, .b):not(.c) { color: red }");
        for name in ["a", "b", "c"] {
            assert!(names.contains_key(name), "{name} missing from {names:?}");
        }
    }

    /// An attribute selector matches the *rendered* attribute, which already
    /// holds the scoped name — rewriting here would be wrong twice over.
    #[test]
    fn an_attribute_selector_is_left_alone() {
        let (css, names) = scoped_css("a[href='.x'][data-y=z] { color: red }");
        assert!(css.contains("[href='.x']"), "{css}");
        assert!(names.is_empty(), "{names:?}");
    }

    /// Nested rules are selectors too.
    #[test]
    fn a_nested_selector_is_scoped() {
        let (_, names) = scoped_css(".card { color: red; & .title { font-weight: 600 } }");
        assert!(
            names.contains_key("card") && names.contains_key("title"),
            "{names:?}"
        );
    }

    /// Inside `@media`, the rules are ordinary rules.
    #[test]
    fn a_selector_inside_an_at_rule_is_scoped() {
        let (_, names) = scoped_css("@media (min-width: 40em) { .wide { display: flex } }");
        assert!(names.contains_key("wide"), "{names:?}");
    }

    /// Silently dropping it would leave an element missing half its styling
    /// with nothing to show why.
    #[test]
    fn composes_is_refused_rather_than_ignored() {
        let message = scope(
            parse(".a { composes: b from './other.css' }"),
            "src/x.module.css",
        )
        .expect_err("composes is not supported");
        assert!(message.contains("composes"), "{message}");
    }

    /// A custom property is not a class, and `.5em` is not a class either.
    #[test]
    fn things_that_look_like_names_and_are_not() {
        let (css, names) = scoped_css(":root { --button: 1px; margin: .5em }");
        assert!(css.contains("--button"), "{css}");
        assert!(css.contains(".5em"), "{css}");
        assert!(names.is_empty(), "{names:?}");
    }
}
