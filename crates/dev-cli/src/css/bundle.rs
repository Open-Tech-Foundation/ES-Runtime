//! Resolving `@import`, and pointing `url()` at where the file it names landed.
//!
//! One pass produces both, and it has to: a `url()` means something different
//! depending on which file wrote it, and after inlining there is no longer
//! anything in the text to say. `theme/dark.css` naming `url(./bg.png)` means
//! the `bg.png` beside *it*, and once its bytes sit inside `app.css` that is
//! unrecoverable. So each is resolved while the file it came from is still the
//! file being read.
//!
//! # It splices; it does not re-print
//!
//! Output is built by copying the input and replacing the spans
//! [`super::token`] identified. Nothing is parsed into a structure and printed
//! back, which is what keeps a declaration this module has never heard of
//! byte-identical on the way through. The failure mode of a printer is silently
//! emitting *different* CSS; the failure mode of a splicer is leaving something
//! alone.
//!
//! # What is inlined, and what is left for the browser
//!
//! `@import "a.css";` and `@import url(a.css);` are inlined. So is a
//! conditional one — `@import "a.css" screen;` becomes
//! `@media screen { … }`, which preserves what the condition meant.
//!
//! Left alone deliberately: an `@import` naming a URL this build does not
//! control (`https:`, `//`, rooted), and one carrying `layer()` or
//! `supports()`. The first is the documented escape hatch. The second is a
//! narrower thing than it looks — `layer()` participates in cascade-layer
//! ordering that depends on where the `@import` sits — and wrapping it wrongly
//! would change which rules win. An `@import` left in place still works; it
//! costs a request, not correctness.

use std::path::{Path, PathBuf};

use super::token::{Kind, Token, tokenize};

/// A file a stylesheet referenced with `url()`, waiting to be given a name.
///
/// The stylesheet cannot name it: the name a file gets in `assets/` is its
/// content hash, and [`crate::html`] is what computes those. So the output
/// carries a placeholder where the URL goes and the caller swaps in the real
/// one once it has written the file.
#[derive(Debug)]
pub struct Referenced {
    /// The file on disk, resolved against the stylesheet that named it.
    pub path: PathBuf,
    /// The opaque string standing in for its URL in the output.
    pub placeholder: String,
}

/// One stylesheet, with its imports resolved.
#[derive(Debug)]
pub struct Stylesheet {
    /// The CSS, with a placeholder at every local `url()`.
    pub code: String,
    /// The files those placeholders stand for.
    pub referenced: Vec<Referenced>,
    /// How many files were merged into it, the entry included.
    pub sources: usize,
}

/// Reads `entry` and everything it imports.
pub fn bundle(entry: &Path) -> Result<Stylesheet, String> {
    let mut out = Stylesheet {
        code: String::new(),
        referenced: Vec::new(),
        sources: 0,
    };
    let mut stack = Vec::new();
    inline(entry, &mut out, &mut stack)?;
    Ok(out)
}

/// Appends `file` to `out.code`, resolving what it references.
///
/// `stack` is the chain of files currently being inlined, and is what makes a
/// cycle terminate. Two stylesheets importing each other is a mistake, but it
/// is the author's mistake to see reported rather than a build that never
/// returns.
fn inline(file: &Path, out: &mut Stylesheet, stack: &mut Vec<PathBuf>) -> Result<(), String> {
    let canonical = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    if stack.contains(&canonical) {
        return Err(format!(
            "{} imports itself, through:\n  {}\n\n\
             An @import cycle has no bundled form — the file would be inlined \
             into itself for ever.",
            display(file),
            stack
                .iter()
                .map(|p| display(p))
                .collect::<Vec<_>>()
                .join("\n  "),
        ));
    }

    let source =
        std::fs::read_to_string(file).map_err(|e| format!("cannot read {}: {e}", display(file)))?;
    let dir = file.parent().unwrap_or(Path::new("."));
    let tokens = tokenize(&source);

    stack.push(canonical);
    out.sources += 1;

    let mut at = 0usize; // How much of `source` has been copied out.
    let mut i = 0usize;
    while i < tokens.len() {
        let token = tokens[i];

        // `@import` — the only at-rule this pass acts on.
        if token.kind == Kind::AtKeyword
            && token.text(&source).eq_ignore_ascii_case("@import")
            && let Some(statement) = read_import(&tokens, i, &source)
        {
            out.code.push_str(&source[at..token.start]);
            emit_import(&statement, dir, &source, out, stack)?;
            at = statement.end;
            i = statement.next;
            continue;
        }

        // A local `url()`, which has to be resolved against *this* file.
        if token.kind == Kind::Url
            && let Some((body_start, body_end)) = token.url_body(&source)
        {
            let url = &source[body_start..body_end];
            if let Some(placeholder) = reference(url, dir, out)? {
                out.code.push_str(&source[at..body_start]);
                out.code.push_str(&placeholder);
                at = body_end;
            }
        }

        // `url("…")` — a function and a string, so the quotes are part of the
        // span to replace and the URL is what is inside them.
        if token.kind == Kind::Function && token.text(&source).eq_ignore_ascii_case("url(") {
            let mut j = i + 1;
            while tokens.get(j).is_some_and(|t| t.kind == Kind::Whitespace) {
                j += 1;
            }
            if let Some(string) = tokens.get(j).filter(|t| t.kind == Kind::String) {
                let quoted = string.text(&source);
                let url = quoted
                    .strip_prefix(['"', '\''])
                    .and_then(|s| s.strip_suffix(['"', '\'']))
                    .unwrap_or(quoted);
                if let Some(placeholder) = reference(url, dir, out)? {
                    out.code.push_str(&source[at..string.start + 1]);
                    out.code.push_str(&placeholder);
                    at = string.end - 1;
                }
            }
        }

        i += 1;
    }
    out.code.push_str(&source[at..]);
    stack.pop();
    Ok(())
}

/// Records a local `url()` and returns the placeholder to write in its place,
/// or `None` if it names something this build does not control.
fn reference(url: &str, dir: &Path, out: &mut Stylesheet) -> Result<Option<String>, String> {
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
    // Opaque and unique, so a later substitution cannot collide with real CSS
    // and two references to the same file still get separate slots. The
    // trailing `__` is load-bearing: without it, substituting `…url_1` would
    // also match inside `…url_10`.
    let placeholder = format!("__esdev_url_{}__", out.referenced.len());
    out.referenced.push(Referenced {
        path,
        placeholder: placeholder.clone(),
    });
    // The query and fragment ride along in the *output* but are not part of
    // what gets substituted, so `url(./f.woff2?v=2)` keeps its `?v=2`.
    Ok(Some(format!("{placeholder}{suffix}")))
}

/// An `@import` statement, located.
struct Import {
    /// The URL it names.
    url: String,
    /// The conditions after the URL — a media query list, or empty.
    conditions: String,
    /// Whether this one is inlinable at all.
    inlinable: bool,
    /// Byte offset of the `@`.
    start: usize,
    /// Byte offset one past the statement's `;`.
    end: usize,
    /// Index of the token after the statement.
    next: usize,
}

/// Reads the `@import` beginning at token `i`, if it is one.
fn read_import(tokens: &[Token], i: usize, source: &str) -> Option<Import> {
    let mut j = i + 1;
    let skip_trivia = |j: &mut usize| {
        while tokens
            .get(*j)
            .is_some_and(|t| matches!(t.kind, Kind::Whitespace | Kind::Comment))
        {
            *j += 1;
        }
    };
    skip_trivia(&mut j);

    // The URL, as either a string or an unquoted `url()`.
    let token = tokens.get(j)?;
    let url = match token.kind {
        Kind::String => {
            let text = token.text(source);
            text.strip_prefix(['"', '\''])
                .and_then(|s| s.strip_suffix(['"', '\'']))
                .unwrap_or(text)
                .to_string()
        }
        Kind::Url => {
            let (start, end) = token.url_body(source)?;
            source[start..end].trim_matches(['"', '\'']).to_string()
        }
        Kind::Function if token.text(source).eq_ignore_ascii_case("url(") => {
            let mut k = j + 1;
            skip_trivia(&mut k);
            let string = tokens.get(k).filter(|t| t.kind == Kind::String)?;
            let text = string.text(source);
            k += 1;
            skip_trivia(&mut k);
            tokens.get(k).filter(|t| t.kind == Kind::CloseParen)?;
            j = k;
            text.strip_prefix(['"', '\''])
                .and_then(|s| s.strip_suffix(['"', '\'']))
                .unwrap_or(text)
                .to_string()
        }
        _ => return None,
    };

    // Everything from here to the `;` is the condition. An `@import` may also
    // be terminated by end-of-file, which browsers accept.
    let condition_start = tokens.get(j + 1).map_or(source.len(), |t| t.start);
    let mut k = j + 1;
    while tokens
        .get(k)
        .is_some_and(|t| !matches!(t.kind, Kind::Semicolon | Kind::OpenBrace))
    {
        k += 1;
    }
    let condition_end = tokens.get(k).map_or(source.len(), |t| t.start);
    let conditions = source[condition_start.min(condition_end)..condition_end]
        .trim()
        .to_string();

    // `layer()` and `supports()` change what the import *means* in ways a
    // wrapper cannot reproduce, so they are recognised in order to be left
    // alone rather than mangled.
    let lowered = conditions.to_ascii_lowercase();
    let inlinable =
        is_local(&url) && !lowered.starts_with("layer") && !lowered.starts_with("supports(");

    Some(Import {
        url,
        conditions,
        inlinable,
        start: tokens[i].start,
        end: tokens.get(k).map_or(source.len(), |t| t.end),
        next: k + 1,
    })
}

/// Writes the statement out: inlined if it can be, unchanged if it cannot.
fn emit_import(
    statement: &Import,
    dir: &Path,
    source: &str,
    out: &mut Stylesheet,
    stack: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if !statement.inlinable {
        // Verbatim, including its condition and its `;`. The browser will fetch
        // it, which is what an `@import` means when it is not bundled.
        out.code.push_str(&source[statement.start..statement.end]);
        return Ok(());
    }

    let target = dir.join(&statement.url);
    if !target.is_file() {
        return Err(format!(
            "{} imports {}, which is not there.",
            display(dir),
            statement.url
        ));
    }

    if statement.conditions.is_empty() {
        inline(&target, out, stack)?;
    } else {
        // The condition applied to the whole imported sheet, so it has to apply
        // to the whole of what replaces it.
        out.code.push_str("@media ");
        out.code.push_str(&statement.conditions);
        out.code.push('{');
        inline(&target, out, stack)?;
        out.code.push('}');
    }
    Ok(())
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
    let shown = std::env::current_dir()
        .ok()
        .and_then(|cwd| path.strip_prefix(cwd).ok().map(Path::to_path_buf))
        .unwrap_or_else(|| path.to_path_buf());
    shown.display().to_string()
}
