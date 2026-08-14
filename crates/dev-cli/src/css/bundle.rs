//! Two passes over the tree: resolve `@import`, and point `url()` at where the
//! file it names landed.
//!
//! Both run while the file that wrote them is still the file being read, and
//! they have to: a `url()` means something different depending on which
//! stylesheet it appeared in, and once its tokens sit inside another file there
//! is nothing left in the text to say which. `theme/dark.css` naming
//! `url(./bg.png)` means the `bg.png` beside *it*.
//!
//! # What is inlined, and what is left for the browser
//!
//! `@import "a.css";` and `@import url(a.css);` are inlined. So is a
//! conditional one — `@import "a.css" screen;` becomes `@media screen { … }`,
//! which is what the condition meant.
//!
//! Left alone deliberately: an `@import` naming a URL this build does not
//! control (`https:`, `//`, rooted), and one carrying `layer()` or
//! `supports()`. The first is the documented escape hatch. The second is
//! narrower than it looks — `layer()` takes part in cascade-layer ordering that
//! depends on where the `@import` sits — and wrapping it wrongly would change
//! which rules win. An `@import` left in place still works; it costs a request,
//! not correctness.

use std::path::{Path, PathBuf};

use super::ast::*;
use super::parse::parse;
use super::token::{Kind, Token};

/// A file a stylesheet referenced with `url()`, waiting to be given a name.
///
/// The stylesheet cannot name it: the name a file gets in `assets/` is its
/// content hash, and [`crate::html`] is what computes those. So the tree
/// carries a placeholder where the URL goes and the caller swaps in the real
/// one once it has written the file.
#[derive(Debug)]
pub struct Referenced {
    /// The file on disk, resolved against the stylesheet that named it.
    pub path: PathBuf,
    /// The opaque string standing in for its URL.
    pub placeholder: String,
}

/// A stylesheet with its imports resolved.
#[derive(Debug)]
pub struct Bundled {
    pub sheet: Stylesheet,
    pub referenced: Vec<Referenced>,
    /// How many files were merged into it, the entry included.
    pub sources: usize,
}

/// Reads `entry` and everything it imports.
pub fn bundle(entry: &Path) -> Result<Bundled, String> {
    let mut out = Bundled {
        sheet: Stylesheet::default(),
        referenced: Vec::new(),
        sources: 0,
    };
    let mut stack = Vec::new();
    let items = read(entry, &mut out, &mut stack)?;
    out.sheet.items = items;
    Ok(out)
}

/// Parses `file`, resolves what it references, and returns its top-level items.
///
/// `stack` is the chain of files currently being inlined, and is what makes a
/// cycle terminate. Two stylesheets importing each other is a mistake, but it
/// is the author's mistake to see reported rather than a build that never
/// returns.
fn read(file: &Path, out: &mut Bundled, stack: &mut Vec<PathBuf>) -> Result<Vec<Item>, String> {
    let canonical = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    if stack.contains(&canonical) {
        return Err(format!(
            "{} imports itself, through:\n  {}\n\n\
             An @import cycle has no bundled form — the file would be inlined \
             into itself for ever.",
            display(file),
            stack
                .iter()
                .map(|path| display(path))
                .collect::<Vec<_>>()
                .join("\n  "),
        ));
    }

    let source =
        std::fs::read_to_string(file).map_err(|e| format!("cannot read {}: {e}", display(file)))?;
    let dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();

    stack.push(canonical);
    out.sources += 1;

    let mut sheet = parse(&source);
    // `url()` first, while `dir` still means this file's directory.
    rewrite_urls_in_items(&mut sheet.items, &dir, out)?;

    let mut items = Vec::new();
    for item in sheet.items {
        match item {
            Item::Rule(Rule::At(at)) if at.name() == "import" && at.block.is_none() => {
                inline_import(&at, &dir, &mut items, out, stack)?;
            }
            other => items.push(other),
        }
    }

    stack.pop();
    Ok(items)
}

/// Replaces one `@import` with the contents of what it names.
fn inline_import(
    at: &AtRule,
    dir: &Path,
    items: &mut Vec<Item>,
    out: &mut Bundled,
    stack: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let Some((url, conditions)) = import_target(at) else {
        items.push(Item::Rule(Rule::At(at.clone())));
        return Ok(());
    };

    let lowered = conditions.to_ascii_lowercase();
    if !is_local(&url) || lowered.starts_with("layer") || lowered.starts_with("supports(") {
        items.push(Item::Rule(Rule::At(at.clone())));
        return Ok(());
    }

    let target = dir.join(&url);
    if !target.is_file() {
        return Err(format!(
            "{} imports {url}, which is not there.",
            display(dir)
        ));
    }

    let inlined = read(&target, out, stack)?;

    if conditions.is_empty() {
        items.extend(inlined);
        return Ok(());
    }

    // The condition applied to the whole imported sheet, so it has to apply to
    // the whole of what replaces it. The prelude is reused verbatim — it is
    // already a parsed media query list, minus the URL.
    let prelude: Vec<ComponentValue> = at
        .prelude
        .iter()
        .skip_while(|value| !is_url_value(value))
        .skip(1)
        .cloned()
        .collect();

    items.push(Item::Rule(Rule::At(AtRule {
        at: Token::new(Kind::AtKeyword, "@media"),
        prelude,
        semicolon: false,
        block: Some(Block {
            closed: true,
            items: inlined
                .into_iter()
                .map(|item| match item {
                    Item::Rule(rule) => BlockItem::Rule(rule),
                    Item::Trivia(token) => BlockItem::Trivia(token),
                    Item::Dangling(values) => BlockItem::Dangling(values),
                })
                .collect(),
        }),
    })));
    Ok(())
}

/// The URL an `@import` names, and whatever conditions follow it.
fn import_target(at: &AtRule) -> Option<(String, String)> {
    let mut values = at.prelude.iter().skip_while(|value| value.is_trivia());
    let first = values.next()?;
    let url = url_of(first)?;

    let conditions: String = at
        .prelude
        .iter()
        .skip_while(|value| !std::ptr::eq(*value, first))
        .skip(1)
        .filter(|value| !value.is_trivia())
        .map(super::print::value_text)
        .collect::<Vec<_>>()
        .join(" ");

    Some((url, conditions))
}

/// Whether a component value is the URL an `@import` names.
fn is_url_value(value: &ComponentValue) -> bool {
    url_of(value).is_some()
}

/// The URL a component value carries, in any of the three spellings.
fn url_of(value: &ComponentValue) -> Option<String> {
    match value {
        ComponentValue::Token(token) if token.kind == Kind::String => Some(token.unescape()),
        ComponentValue::Token(token) if token.kind == Kind::Url => token.url(),
        ComponentValue::Function(function) if function.name() == "url" => function
            .arguments
            .iter()
            .find(|value| !value.is_trivia())
            .and_then(|value| value.token())
            .filter(|token| token.kind == Kind::String)
            .map(Token::unescape),
        _ => None,
    }
}

// --- url() rewriting ---------------------------------------------------------

fn rewrite_urls_in_items(items: &mut [Item], dir: &Path, out: &mut Bundled) -> Result<(), String> {
    for item in items {
        match item {
            Item::Rule(rule) => rewrite_urls_in_rule(rule, dir, out)?,
            Item::Dangling(values) => rewrite_urls(values, dir, out)?,
            Item::Trivia(_) => {}
        }
    }
    Ok(())
}

fn rewrite_urls_in_rule(rule: &mut Rule, dir: &Path, out: &mut Bundled) -> Result<(), String> {
    match rule {
        Rule::At(at) => {
            // An `@import`'s own URL is not an asset — it is handled by the
            // import pass, which needs to see it unrewritten.
            if at.name() != "import" {
                rewrite_urls(&mut at.prelude, dir, out)?;
            }
            if let Some(block) = &mut at.block {
                rewrite_urls_in_block(block, dir, out)?;
            }
        }
        Rule::Qualified(qualified) => {
            rewrite_urls(&mut qualified.prelude, dir, out)?;
            rewrite_urls_in_block(&mut qualified.block, dir, out)?;
        }
    }
    Ok(())
}

fn rewrite_urls_in_block(block: &mut Block, dir: &Path, out: &mut Bundled) -> Result<(), String> {
    for item in &mut block.items {
        match item {
            BlockItem::Declaration(declaration) => {
                rewrite_urls(&mut declaration.value, dir, out)?;
            }
            BlockItem::Rule(rule) => rewrite_urls_in_rule(rule, dir, out)?,
            BlockItem::Dangling(values) => rewrite_urls(values, dir, out)?,
            BlockItem::Trivia(_) | BlockItem::Semicolon => {}
        }
    }
    Ok(())
}

/// Rewrites every local `url()` in a component-value list, recursively.
fn rewrite_urls(
    values: &mut [ComponentValue],
    dir: &Path,
    out: &mut Bundled,
) -> Result<(), String> {
    for value in values {
        match value {
            ComponentValue::Token(token) if token.kind == Kind::Url => {
                if let Some(url) = token.url()
                    && let Some(replacement) = reference(&url, dir, out)?
                {
                    // Quoted, because a placeholder is opaque text and quoting
                    // is the form that cannot be a bad-url-token whatever the
                    // substituted name turns out to contain.
                    *token = Token::new(Kind::Url, format!("url(\"{replacement}\")"));
                }
            }
            ComponentValue::Function(function) => {
                if function.name() == "url" {
                    let quoted = function
                        .arguments
                        .iter_mut()
                        .find(|value| !value.is_trivia())
                        .and_then(|value| match value {
                            ComponentValue::Token(token) if token.kind == Kind::String => {
                                Some(token)
                            }
                            _ => None,
                        });
                    if let Some(token) = quoted
                        && let Some(replacement) = reference(&token.unescape(), dir, out)?
                    {
                        *token = Token::new(Kind::String, format!("\"{replacement}\""));
                    }
                } else {
                    rewrite_urls(&mut function.arguments, dir, out)?;
                }
            }
            ComponentValue::Block(block) => rewrite_urls(&mut block.items, dir, out)?,
            ComponentValue::Token(_) => {}
        }
    }
    Ok(())
}

/// Records a local URL and returns the text to write in its place, or `None` if
/// it names something this build does not control.
fn reference(url: &str, dir: &Path, out: &mut Bundled) -> Result<Option<String>, String> {
    if !is_local(url) {
        return Ok(None);
    }
    // The query and the fragment are the browser's business — `?v=2` on a font,
    // `#icon` on a sprite. They are only in the way when resolving a path, and
    // they are kept on the URL that is written out.
    let end = url.find(['?', '#']).unwrap_or(url.len());
    let (name, suffix) = url.split_at(end);
    let path = dir.join(name);
    if !path.is_file() {
        return Err(format!(
            "{} references {url}, which is not there.\n\n\
             A relative url() in a stylesheet names a file in the project. For a \
             URL this build should leave alone, write it rooted (/{}) or absolute.",
            display(dir),
            url.trim_start_matches("./"),
        ));
    }
    // Opaque and unique. The trailing `__` is load-bearing: without it,
    // substituting `…url_1__` would also match inside `…url_11__`.
    let placeholder = format!("__esdev_url_{}__", out.referenced.len());
    out.referenced.push(Referenced {
        path,
        placeholder: placeholder.clone(),
    });
    Ok(Some(format!("{placeholder}{suffix}")))
}

/// Whether a URL names a file in the project rather than something the browser
/// fetches from elsewhere.
fn is_local(url: &str) -> bool {
    !url.is_empty()
        && !url.starts_with('/')
        && !url.starts_with('#')
        && !url.contains("://")
        && !url.starts_with("data:")
        && !url.starts_with("//")
}

/// A path as it is worth showing in an error.
fn display(path: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok().map(Path::to_path_buf))
        .unwrap_or_else(|| path.to_path_buf())
        .display()
        .to_string()
}
