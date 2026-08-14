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
//! # `composes`
//!
//! Reuse without repetition, resolved at build time:
//!
//! ```css
//! .button {
//!   composes: rounded from "./base.module.css";
//!   color: white;
//! }
//! ```
//!
//! `styles.button` then stops being one name and becomes two —
//! `"button_a1b2c3d4 rounded_e5f6a7b8"` — and the element carries both classes.
//! The declaration itself is **removed**, because `composes` is a convention of
//! this build and not a property any browser knows.
//!
//! Three forms: `composes: a b` (this file), `composes: a from "./x.module.css"`
//! (another module, resolved through [`Resolve`]), and `composes: a from global`
//! (a name nothing scopes).
//!
//! Two restrictions, both enforced with a message rather than silently: the rule
//! must be a **single class selector**, since otherwise there is no one name to
//! attach the composition to; and a cycle between modules is refused, since it
//! has no finite answer.
//!
//! Composition is **transitive**: if `.big` composes `.button` and `.button`
//! composes `.rounded`, `.big` carries all three, because a class only styles
//! an element that actually has it. A cycle is refused rather than followed.
//!
//! **Order in the class list does not decide the cascade** — which rule wins is
//! decided by specificity and position in the stylesheet, exactly as always. So
//! `composes` is for combining things that do not overlap; two composed classes
//! setting the same property is a coin toss, and the same was true before.
//!
//! # The hash is over the path, not the contents
//!
//! So editing a component does not rename its classes. That keeps a rebuild
//! from invalidating every reference to a name, and — because the path is taken
//! relative to the project root — keeps two machines building the same commit
//! to the same class names.

use std::collections::{BTreeMap, HashSet};

use super::ast::*;
use super::print::value_text;
use super::token::{Kind, Token};

/// How a `composes: … from "./other.module.css"` reaches the other module.
///
/// A trait rather than a path, so this module does no I/O and its tests need no
/// files. [`crate::cssmodules`] implements it by parsing and scoping the target,
/// with a memo so a module imported twice is scoped once.
pub trait Resolve {
    /// The scoped names of the module `specifier` names, relative to the file
    /// currently being scoped.
    fn names(&mut self, specifier: &str) -> Result<BTreeMap<String, String>, String>;
}

/// One name a class composes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Composed {
    /// A class in this same file, resolved once the whole file has been walked.
    Local(String),
    /// A name that is already final: from another module, or `from global`.
    Ready(String),
}

/// A [`Resolve`] that refuses every specifier — for a caller with no module
/// graph, and for the tests that do not exercise cross-file composition.
#[allow(dead_code, reason = "the no-graph entry point; used by tests")]
pub struct NoImports;

impl Resolve for NoImports {
    fn names(&mut self, specifier: &str) -> Result<BTreeMap<String, String>, String> {
        Err(format!("cannot resolve {specifier} from here"))
    }
}

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
#[allow(dead_code, reason = "the no-graph entry point; used by tests")]
pub fn scope(sheet: Stylesheet, ident: &str) -> Result<Scoped, String> {
    scope_with(sheet, ident, &mut NoImports)
}

/// Scopes `sheet`, resolving `composes … from` through `resolve`.
pub fn scope_with<R: Resolve>(
    mut sheet: Stylesheet,
    ident: &str,
    resolve: &mut R,
) -> Result<Scoped, String> {
    let suffix = hash(ident);
    let mut names = BTreeMap::new();

    // Keyframes first: a rule can refer to an animation defined below it, so
    // the set has to be complete before any value is rewritten.
    let mut keyframes = HashSet::new();
    collect_keyframes(&sheet.items, &mut keyframes);

    // What each class composes, collected while walking and applied after — so
    // a class may compose one declared further down the same file.
    let mut composed: Vec<(String, Vec<Composed>)> = Vec::new();

    for item in &mut sheet.items {
        if let Item::Rule(rule) = item {
            rewrite_rule(
                rule,
                &suffix,
                &keyframes,
                &mut names,
                &mut composed,
                resolve,
            )?;
        }
    }

    // Composition is **transitive**. If `.big` composes `.button` and `.button`
    // composes `.rounded`, an element with only `big button` gets none of
    // `.rounded`'s rules — the class has to be on the element, and nothing else
    // will put it there. So each list is expanded through the whole chain.
    let chains: BTreeMap<String, Vec<Composed>> = composed.into_iter().collect();
    let locals: Vec<String> = chains.keys().cloned().collect();
    for local in locals {
        let mut out = Vec::new();
        expand(&local, &chains, &names, &suffix, &mut out, &mut Vec::new())?;
        if let Some(own) = names.get(&local).cloned() {
            let mut value = own;
            for name in out {
                value.push(' ');
                value.push_str(&name);
            }
            names.insert(local, value);
        }
    }

    Ok(Scoped { sheet, names })
}

/// Appends everything `local` composes, following the chain.
///
/// `visiting` is the path taken to get here, and is what makes a cycle
/// terminate. Two classes composing each other has no finite answer, and a
/// build that recursed until the stack went would report it as a crash rather
/// than as the mistake it is.
fn expand(
    local: &str,
    chains: &BTreeMap<String, Vec<Composed>>,
    names: &BTreeMap<String, String>,
    suffix: &str,
    out: &mut Vec<String>,
    visiting: &mut Vec<String>,
) -> Result<(), String> {
    if visiting.iter().any(|seen| seen == local) {
        return Err(format!(
            "`composes` cycles through `{local}`.\n\n             A class composing one that composes it back has no answer."
        ));
    }
    visiting.push(local.to_string());

    for name in chains.get(local).map(Vec::as_slice).unwrap_or_default() {
        match name {
            Composed::Ready(name) => push_once(out, name.clone()),
            Composed::Local(name) => {
                if !names.contains_key(name) {
                    return Err(format!(
                        "`composes: {name}` names a class this file does not declare."
                    ));
                }
                push_once(out, scoped(name, suffix));
                expand(name, chains, names, suffix, out, visiting)?;
            }
        }
    }

    visiting.pop();
    Ok(())
}

/// Adds a class name unless it is already in the list — two chains reaching the
/// same class is ordinary, and the attribute should say it once.
fn push_once(out: &mut Vec<String>, name: String) {
    if !out.contains(&name) {
        out.push(name);
    }
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

fn rewrite_rule<R: Resolve>(
    rule: &mut Rule,
    suffix: &str,
    keyframes: &HashSet<String>,
    names: &mut BTreeMap<String, String>,
    composed: &mut Vec<(String, Vec<Composed>)>,
    resolve: &mut R,
) -> Result<(), String> {
    match rule {
        Rule::Qualified(qualified) => {
            // Read before the selector is rewritten, so the error message and
            // the mapping key both name the class the author wrote.
            let sole = sole_class(&qualified.prelude);
            rewrite_selector(&mut qualified.prelude, suffix, names);
            take_composes(&mut qualified.block, sole, composed, resolve)?;
            rewrite_block(
                &mut qualified.block,
                suffix,
                keyframes,
                names,
                composed,
                resolve,
            )?;
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
                rewrite_block(block, suffix, keyframes, names, composed, resolve)?;
            }
        }
    }
    Ok(())
}

fn rewrite_block<R: Resolve>(
    block: &mut Block,
    suffix: &str,
    keyframes: &HashSet<String>,
    names: &mut BTreeMap<String, String>,
    composed: &mut Vec<(String, Vec<Composed>)>,
    resolve: &mut R,
) -> Result<(), String> {
    for item in &mut block.items {
        match item {
            BlockItem::Declaration(declaration) => {
                // `animation: fade 1s` and `animation-name: fade` name a
                // `@keyframes` this file scoped, so the reference moves with it.
                let property = declaration.name.text.to_ascii_lowercase();
                if property == "animation" || property == "animation-name" {
                    rewrite_animation(&mut declaration.value, suffix, keyframes);
                }
            }
            BlockItem::Rule(rule) => {
                rewrite_rule(rule, suffix, keyframes, names, composed, resolve)?;
            }
            BlockItem::Trivia(_) | BlockItem::Semicolon | BlockItem::Dangling(_) => {}
        }
    }
    Ok(())
}

/// Removes every `composes` declaration from `block` and records what it named.
///
/// Removed rather than rewritten: `composes` is a convention of this build, and
/// a browser handed one skips it as an unknown property — which would look like
/// it worked while doing nothing.
fn take_composes<R: Resolve>(
    block: &mut Block,
    sole: Option<String>,
    composed: &mut Vec<(String, Vec<Composed>)>,
    resolve: &mut R,
) -> Result<(), String> {
    let mut found: Vec<Composed> = Vec::new();
    let mut error = None;
    // A declaration is followed by the `;` that ended it. Dropping the
    // declaration alone would leave that behind as an empty one — harmless to a
    // browser, and visible in the output as a stray `;`.
    let mut drop_next_semicolon = false;

    block.items.retain(|item| {
        if let BlockItem::Semicolon = item
            && std::mem::take(&mut drop_next_semicolon)
        {
            return false;
        }
        let BlockItem::Declaration(declaration) = item else {
            return true;
        };
        if !declaration.name.text.eq_ignore_ascii_case("composes") {
            return true;
        }
        drop_next_semicolon = true;
        match sole.as_deref() {
            // `composes` attaches a name to *one* class. On `.a .b` or `div`
            // there is nothing to attach it to.
            None => {
                error.get_or_insert_with(|| {
                    "`composes` needs a rule that is a single class selector.\n\n                     It adds a name to that class, and a rule matching anything                      else has no one name to add it to. Move the `composes` to                      its own `.class { … }` rule."
                        .to_string()
                });
            }
            Some(_) => match compose_names(&declaration.value, resolve) {
                Ok(names) => found.extend(names),
                Err(message) => {
                    error.get_or_insert(message);
                }
            },
        }
        false
    });

    if let Some(message) = error {
        return Err(message);
    }
    if let (Some(class), false) = (sole, found.is_empty()) {
        composed.push((class, found));
    }
    Ok(())
}

/// The scoped names one `composes` declaration resolves to.
fn compose_names<R: Resolve>(
    value: &[ComponentValue],
    resolve: &mut R,
) -> Result<Vec<Composed>, String> {
    let words: Vec<String> = value
        .iter()
        .filter(|value| !value.is_trivia())
        .map(value_text)
        .collect();

    // `composes: a b from "./x.module.css"` — everything before `from` is a
    // name, and what follows says where to look it up.
    let (wanted, source) = match words.iter().position(|word| word == "from") {
        Some(at) => (&words[..at], words.get(at + 1).map(String::as_str)),
        None => (&words[..], None),
    };

    if wanted.is_empty() {
        return Err("`composes` names nothing.".to_string());
    }

    match source {
        // `composes: a b` — this file's own classes. Deferred, so a class may
        // compose one declared further down.
        None => Ok(wanted.iter().cloned().map(Composed::Local).collect()),
        // `composes: a from global` — a name nothing scopes.
        Some("global") => Ok(wanted.iter().cloned().map(Composed::Ready).collect()),
        Some(specifier) => {
            let path = specifier.trim_matches(['"', '\'']);
            let names = resolve.names(path)?;
            wanted
                .iter()
                .map(|name| {
                    names
                        .get(name)
                        .cloned()
                        .map(Composed::Ready)
                        .ok_or_else(|| format!("{path} has no class `{name}` to compose."))
                })
                .collect()
        }
    }
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

/// The single class a rule selects, if that is all it selects.
///
/// `composes` attaches a name to one class, so `.a`, and not `.a .b`, `div.a`
/// or `.a:hover`. Anything with more than one significant token is refused.
fn sole_class(prelude: &[ComponentValue]) -> Option<String> {
    let significant: Vec<&ComponentValue> =
        prelude.iter().filter(|value| !value.is_trivia()).collect();
    let [dot, name] = significant.as_slice() else {
        return None;
    };
    let ComponentValue::Token(dot) = dot else {
        return None;
    };
    let ComponentValue::Token(name) = name else {
        return None;
    };
    (dot.kind == Kind::Delim && dot.text == "." && name.kind == Kind::Ident)
        .then(|| name.text.clone())
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

    fn scoped_css_err(source: &str) -> String {
        scope(parse(source), "src/Button.module.css").expect_err("should not scope")
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

    /// A stub module graph, so these tests need no files on disk.
    struct Stub(BTreeMap<String, BTreeMap<String, String>>);

    impl Resolve for Stub {
        fn names(&mut self, specifier: &str) -> Result<BTreeMap<String, String>, String> {
            self.0
                .get(specifier)
                .cloned()
                .ok_or_else(|| format!("no such module {specifier}"))
        }
    }

    fn stub(module: &str, names: &[(&str, &str)]) -> Stub {
        Stub(
            [(
                module.to_string(),
                names
                    .iter()
                    .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
                    .collect(),
            )]
            .into_iter()
            .collect(),
        )
    }

    /// The mechanic: the mapping's value becomes two names, and the element
    /// carries both classes.
    #[test]
    fn composing_from_another_module_yields_both_names() {
        let out = scope_with(
            parse(".button { composes: rounded from \"./base.module.css\"; color: white }"),
            "src/Button.module.css",
            &mut stub("./base.module.css", &[("rounded", "rounded_e5f6a7b8")]),
        )
        .expect("composes");

        let value = &out.names["button"];
        assert!(value.ends_with(" rounded_e5f6a7b8"), "{value}");
        assert_eq!(value.split(' ').count(), 2, "{value}");

        // The declaration is gone: a browser handed `composes:` skips it as an
        // unknown property, which would look like it worked.
        let css = print_minified(&out.sheet);
        assert!(!css.contains("composes"), "{css}");
        assert!(css.contains("color:white"), "{css}");
    }

    #[test]
    fn composing_within_one_file_needs_no_module_graph() {
        let (_, names) = scoped_css(".base { padding: 1rem }\n.card { composes: base }");
        let card = &names["card"];
        assert_eq!(card.split(' ').count(), 2, "{card}");
        assert!(
            card.ends_with(&names["base"]),
            "{card} vs {}",
            names["base"]
        );
    }

    /// A class may compose one declared further down the file, which is why the
    /// lookup is deferred until the whole file has been walked.
    #[test]
    fn composing_a_class_declared_later_works() {
        let (_, names) = scoped_css(".card { composes: base }\n.base { padding: 1rem }");
        assert!(names["card"].ends_with(&names["base"]), "{names:?}");
    }

    /// A class only styles an element that has it, so a chain has to be
    /// followed all the way down or the middle link's styling is lost.
    #[test]
    fn composition_is_transitive() {
        let (_, names) =
            scoped_css(".rounded{}\n.button { composes: rounded }\n.big { composes: button }");
        let big: Vec<&str> = names["big"].split(' ').collect();
        assert_eq!(big.len(), 3, "{:?}", names["big"]);
        assert!(big.contains(&names["rounded"].as_str()), "{big:?}");
    }

    /// Two chains reaching the same class should say it once.
    #[test]
    fn a_name_reached_twice_appears_once() {
        let (_, names) =
            scoped_css(".a{}\n.b { composes: a }\n.c { composes: a }\n.d { composes: b c }");
        let parts: Vec<&str> = names["d"].split(' ').collect();
        let unique: std::collections::BTreeSet<&str> = parts.iter().copied().collect();
        assert_eq!(parts.len(), unique.len(), "{:?}", names["d"]);
    }

    /// Two classes composing each other has no finite answer.
    #[test]
    fn a_composition_cycle_is_refused_rather_than_followed() {
        let message = scoped_css_err(".a { composes: b }\n.b { composes: a }");
        assert!(message.contains("cycles"), "{message}");
    }

    #[test]
    fn several_names_compose_in_the_order_written() {
        let (_, names) = scoped_css(".a{}\n.b{}\n.c { composes: a b }");
        let parts: Vec<&str> = names["c"].split(' ').collect();
        assert_eq!(parts.len(), 3, "{:?}", names["c"]);
        assert_eq!(parts[1], names["a"]);
        assert_eq!(parts[2], names["b"]);
    }

    /// `from global` reaches a name nothing scopes.
    #[test]
    fn composing_from_global_keeps_the_name_as_written() {
        let (_, names) = scoped_css(".a { composes: sr-only from global }");
        assert!(names["a"].ends_with(" sr-only"), "{:?}", names["a"]);
    }

    /// `composes` adds a name to *one* class, so a rule matching anything else
    /// has nothing to add it to.
    #[test]
    fn composes_on_a_rule_that_is_not_one_class_is_refused() {
        for selector in [".a .b", "div", ".a:hover", ".a, .b"] {
            let source = format!("{selector} {{ composes: x from global }}");
            let message =
                scope(parse(&source), "src/x.module.css").expect_err("should refuse {selector}");
            assert!(message.contains("single class"), "{selector}: {message}");
        }
    }

    /// A name that is not there is the author's mistake to see, not an element
    /// quietly missing half its styling.
    #[test]
    fn composing_a_name_that_does_not_exist_is_an_error() {
        let local = scoped_css_err(".a { composes: nope }");
        assert!(local.contains("nope"), "{local}");

        let remote = scope_with(
            parse(".a { composes: nope from \"./base.module.css\" }"),
            "src/x.module.css",
            &mut stub("./base.module.css", &[("rounded", "rounded_1")]),
        )
        .expect_err("no such class");
        assert!(remote.contains("nope"), "{remote}");
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
