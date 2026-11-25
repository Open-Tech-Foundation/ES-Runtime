//! The documentation names things the code defines — error codes, decisions,
//! pages, files, modules, flags — and each is a fact with one home in the code
//! (AGENTS.md: "restating it elsewhere is how documentation starts lying").
//! These tests hold every page to those homes, so a renamed code, a removed
//! flag or a moved page fails here instead of in the next review.
//!
//! `docs/DECISIONS.md` and the changelogs are history: they name what was
//! rejected, renamed or removed on purpose, so the checks that ask "does this
//! still exist" skip them. Only `std`: this is a scanner, not a parser, and the
//! project counts its dependencies.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    // The one spelling of a real path (D165).
    es_runtime_default_providers::path::canonicalize(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
    )
    .expect("repo root")
}

/// Every file under `dir` (relative to the repo) whose name passes `keep`,
/// skipping dependencies and build output.
fn files(dir: &str, keep: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![repo().join(dir)];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if p.is_dir() {
                if !matches!(
                    name.as_ref(),
                    "node_modules" | "target" | "vendor" | ".next" | "out"
                ) {
                    stack.push(p);
                }
            } else if keep(&p) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn ext(p: &Path, wanted: &[&str]) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| wanted.contains(&e))
}

/// The documentation: the site's pages, the repo's reference docs and READMEs.
fn docs() -> Vec<PathBuf> {
    let mut out = files("website/app", &|p| ext(p, &["mdx"]));
    out.extend(files("docs", &|p| ext(p, &["md"])));
    for top in ["README.md", "SECURITY.md"] {
        out.push(repo().join(top));
    }
    out
}

fn is_history(p: &Path) -> bool {
    p.ends_with("docs/DECISIONS.md")
}

fn rel(p: &Path) -> String {
    p.strip_prefix(repo()).unwrap_or(p).display().to_string()
}

/// The source the facts live in.
fn source_text() -> String {
    let mut text = String::new();
    for p in files("crates", &|p| ext(p, &["rs", "js", "mjs"]))
        .into_iter()
        .chain(files("packages", &|p| ext(p, &["js", "mjs", "ts"])))
    {
        text.push_str(&std::fs::read_to_string(&p).unwrap_or_default());
        text.push('\n');
    }
    text
}

/// Each maximal run of `[A-Z0-9_]` following `ERR_`.
fn error_codes(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while let Some(at) = text[i..].find("ERR_") {
        let start = i + at;
        let mut end = start + 4;
        while end < bytes.len()
            && (bytes[end].is_ascii_uppercase()
                || bytes[end].is_ascii_digit()
                || bytes[end] == b'_')
        {
            end += 1;
        }
        // `ERR_ARCHIVE_*` names a family, not a code.
        let code = text[start..end].trim_end_matches('_');
        let before = start.checked_sub(1).map(|j| bytes[j]);
        if code.len() > 4 && !before.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_') {
            out.insert(code.to_string());
        }
        i = end.max(start + 1);
    }
    out
}

fn fail_if_any(what: &str, found: Vec<String>) {
    assert!(found.is_empty(), "{what}:\n  {}", found.join("\n  "));
}

#[test]
fn every_error_code_the_docs_name_is_one_the_code_has() {
    let known = error_codes(&source_text());
    let mut found = Vec::new();
    for p in docs().iter().filter(|p| !is_history(p)) {
        let text = std::fs::read_to_string(p).unwrap();
        for code in error_codes(&text) {
            // Node's codes, named in comparisons with it.
            if !known.contains(&code)
                && !text.contains(&format!("Node | `{code}`"))
                && code != "ERR_MODULE_NOT_FOUND"
            {
                found.push(format!("{}: {code}", rel(p)));
            }
        }
    }
    fail_if_any("error codes no source defines", found);
}

#[test]
fn every_decision_the_docs_cite_exists() {
    let decisions = std::fs::read_to_string(repo().join("docs/DECISIONS.md")).unwrap();
    let known: BTreeSet<u32> = decisions
        .lines()
        .filter_map(|l| l.strip_prefix("### D"))
        .filter_map(|l| l.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok())
        .collect();
    let mut found = Vec::new();
    for p in docs() {
        let text = std::fs::read_to_string(&p).unwrap();
        let bytes = text.as_bytes();
        for (at, _) in text.match_indices('D') {
            let before = at.checked_sub(1).map(|j| bytes[j]);
            if before.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
                continue;
            }
            let digits: String = text[at + 1..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            let after = bytes.get(at + 1 + digits.len()).copied();
            if digits.is_empty()
                || digits.len() > 3
                || after.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                continue;
            }
            let n: u32 = digits.parse().unwrap();
            if !known.contains(&n) {
                found.push(format!("{}: D{n}", rel(&p)));
            }
        }
    }
    found.dedup();
    fail_if_any("decisions cited that DECISIONS.md does not have", found);
}

/// The site's routes: every directory under `website/app` with a page.
fn routes() -> BTreeSet<String> {
    files("website/app", &|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("page."))
    })
    .iter()
    .map(|p| {
        let dir = p.parent().unwrap();
        let route = dir
            .strip_prefix(repo().join("website/app"))
            .unwrap()
            .display()
            .to_string();
        format!("/{route}").trim_end_matches('/').to_string()
    })
    .map(|r| if r.is_empty() { "/".to_string() } else { r })
    .collect()
}

/// The site-relative targets of Markdown links and `href`s.
fn internal_links(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (open, close) in [("](/", ')'), ("href=\"/", '"')] {
        for (at, _) in text.match_indices(open) {
            let start = at + open.len() - 1;
            let target: String = text[start..]
                .chars()
                .take_while(|c| *c != close && !c.is_whitespace())
                .collect();
            let path = target
                .split(['#', '?'])
                .next()
                .unwrap_or("")
                .trim_end_matches('/');
            out.push(if path.is_empty() {
                "/".to_string()
            } else {
                path.to_string()
            });
        }
    }
    out
}

#[test]
fn every_internal_link_reaches_a_page() {
    let routes = routes();
    let mut found = Vec::new();
    for p in files("website/app", &|p| ext(p, &["mdx"])) {
        let text = std::fs::read_to_string(&p).unwrap();
        for link in internal_links(&text) {
            // Files the site serves rather than pages it renders.
            if link.starts_with("/install") || link.contains('.') {
                continue;
            }
            if !routes.contains(&link) {
                found.push(format!("{}: {link}", rel(&p)));
            }
        }
    }
    found.dedup();
    fail_if_any("links to pages the site does not have", found);
}

#[test]
fn every_repo_path_the_docs_name_exists() {
    const ROOTS: [&str; 8] = [
        "crates/",
        "bench/",
        "docs/",
        "packages/",
        "scripts/",
        "website/",
        "fuzz/",
        "examples/",
    ];
    let mut found = Vec::new();
    for p in docs().iter().filter(|p| !is_history(p)) {
        let text = std::fs::read_to_string(p).unwrap();
        for span in text.split('`').skip(1).step_by(2) {
            if !ROOTS.iter().any(|r| span.starts_with(r))
                || span.contains(' ')
                || span.contains('*')
            {
                continue;
            }
            let path = span.split(':').next().unwrap().trim_end_matches(['.', ',']);
            if !repo().join(path).exists() {
                found.push(format!("{}: {path}", rel(p)));
            }
        }
    }
    fail_if_any("repo paths that do not exist", found);
}

#[test]
fn every_runtime_module_the_docs_name_exists() {
    let mut known: BTreeSet<String> =
        std::fs::read_dir(repo().join("crates/runtime/src/runtime_modules"))
            .unwrap()
            .flatten()
            .filter_map(|e| {
                e.file_name()
                    .to_str()?
                    .strip_suffix(".js")
                    .map(str::to_string)
            })
            .collect();
    // esdev's own (D71).
    known.extend(["build", "watch", "test"].map(String::from));
    let mut found = Vec::new();
    for p in docs().iter().filter(|p| !is_history(p)) {
        let text = std::fs::read_to_string(p).unwrap();
        for (at, _) in text.match_indices("runtime:") {
            let name: String = text[at + 8..]
                .chars()
                .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
                .collect();
            if !name.is_empty() && !known.contains(&name) {
                found.push(format!("{}: runtime:{name}", rel(p)));
            }
        }
    }
    found.dedup();
    fail_if_any("runtime: modules that do not exist", found);
}

#[test]
fn every_flag_on_an_esrun_or_esdev_command_is_one_a_parser_takes() {
    // Every flag literal the three parsers match on, plus the permission
    // vocabulary, which they build from `Capability`.
    let mut known = BTreeSet::new();
    for dir in [
        "crates/runtime-cli/src",
        "crates/cli-common/src",
        "crates/dev-cli/src",
    ] {
        for p in files(dir, &|p| ext(p, &["rs"])) {
            let text = std::fs::read_to_string(&p).unwrap();
            for piece in text.split('"').skip(1).step_by(2) {
                if piece.starts_with('-')
                    && piece.len() > 1
                    && piece[1..]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-')
                {
                    known.insert(piece.to_string());
                }
            }
        }
    }
    for cap in es_runtime_common::Capability::HOST_FACING {
        if let Some(name) = cap.flag_name() {
            known.insert(format!("--allow-{name}"));
            known.insert(format!("--deny-{name}"));
        }
    }
    // Template options `esdev create` reads from each template (`--mode`,
    // `--styling`), and other tools' flags passed through (`tsc`'s).
    for passed in ["--mode", "--styling", "--noEmit", "--pretty", "--owner"] {
        known.insert(passed.to_string());
    }
    let mut found = Vec::new();
    for p in docs().iter().filter(|p| !is_history(p)) {
        let text = std::fs::read_to_string(p).unwrap();
        for (n, line) in text.lines().enumerate() {
            for bin in ["esrun ", "esdev "] {
                for (at, _) in line.match_indices(bin) {
                    let command = line[at..].split(['`', '|', '#']).next().unwrap_or("");
                    // A script's own arguments start after the file it runs.
                    let mut past_script = false;
                    for word in command.split_whitespace().skip(1) {
                        let flag = word
                            .split('=')
                            .next()
                            .unwrap()
                            .trim_end_matches([',', ';', ')', '.', ':']);
                        if !flag.starts_with('-') {
                            if flag.ends_with(".js")
                                || flag.ends_with(".mjs")
                                || flag.ends_with(".ts")
                            {
                                past_script = true;
                            }
                            continue;
                        }
                        if past_script || flag == "-" || flag == "--" || flag.len() < 2 {
                            continue;
                        }
                        // A placeholder (`--allow-<name>`), or not a flag at
                        // all (`-->`).
                        if flag.contains('<')
                            || !flag[1..]
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || c == '-')
                        {
                            continue;
                        }
                        if !known.contains(flag) && !flag.starts_with("--allow-ne") {
                            found.push(format!("{}:{}: {flag}", rel(p), n + 1));
                        }
                    }
                }
            }
        }
    }
    found.dedup();
    fail_if_any("flags no esrun or esdev parser takes", found);
}

/// A heading's id as the site generates it — checked against the built site:
/// lowercase; letters, digits, `-` and `_` kept; each space a `-`; anything
/// else dropped (`mongodb+srv://` is `mongodbsrv`). A repeated id gets `-1`,
/// `-2`, ….
fn slug(heading: &str) -> String {
    // Link text, not the link: `[text](url)` → `text`.
    let mut text = String::new();
    let mut rest = heading;
    while let Some(open) = rest.find('[') {
        text.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match (after.find("]("), after.find(')')) {
            (Some(close), Some(end)) if close < end => {
                text.push_str(&after[..close]);
                rest = &after[end + 1..];
            }
            _ => {
                text.push('[');
                rest = after;
            }
        }
    }
    text.push_str(rest);
    text.trim()
        .chars()
        .filter_map(|c| {
            if c.is_alphanumeric() {
                Some(c.to_lowercase().next().unwrap_or(c))
            } else if c == ' ' {
                Some('-')
            } else if c == '-' || c == '_' {
                Some(c)
            } else {
                None
            }
        })
        .collect()
}

/// The fragment ids a page has: its headings' slugs, and any `id="…"` it sets.
fn anchors(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut seen = std::collections::BTreeMap::<String, usize>::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let hashes = line.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
            let base = slug(&line[hashes + 1..]);
            let n = seen.entry(base.clone()).or_insert(0);
            out.insert(if *n == 0 {
                base.clone()
            } else {
                format!("{base}-{n}")
            });
            *n += 1;
        }
    }
    for (at, _) in text.match_indices("id=\"") {
        let id: String = text[at + 4..].chars().take_while(|c| *c != '"').collect();
        out.insert(id);
    }
    out
}

#[test]
fn every_section_link_reaches_a_heading() {
    let site = repo().join("website/app");
    let page_of = |route: &str| site.join(route.trim_start_matches('/')).join("page.mdx");
    let mut found = Vec::new();
    for p in files("website/app", &|p| ext(p, &["mdx"])) {
        let text = std::fs::read_to_string(&p).unwrap();
        let here = anchors(&text);
        for (open, close) in [("](", ')'), ("href=\"", '"')] {
            for (at, _) in text.match_indices(open) {
                let target: String = text[at + open.len()..]
                    .chars()
                    .take_while(|c| *c != close && !c.is_whitespace())
                    .collect();
                let Some((path, fragment)) = target.split_once('#') else {
                    continue;
                };
                if fragment.is_empty() || path.starts_with("http") || path.contains('.') {
                    continue;
                }
                let ok = if path.is_empty() {
                    here.contains(fragment)
                } else if path.starts_with('/') {
                    let target_page = page_of(path.trim_end_matches('/'));
                    match std::fs::read_to_string(&target_page) {
                        Ok(t) => anchors(&t).contains(fragment),
                        // A route with no page.mdx is the other test's concern.
                        Err(_) => true,
                    }
                } else {
                    true
                };
                if !ok {
                    found.push(format!("{}: {target}", rel(&p)));
                }
            }
        }
    }
    found.dedup();
    fail_if_any("links to a section the page does not have", found);
}
