//! An `index.html` target — the file that names the browser's half of a build.
//!
//! A server bundle's entry is a module, because the runtime starts at one. The
//! browser does not: it starts at a **document**, and the module is something
//! that document happens to reference. Which is why every other build tool
//! treats HTML as an entry point and why this one now does too — the
//! alternative is naming the client entry in a config file *and* naming its
//! built URL in the HTML, where the two are one rename apart from disagreeing.
//!
//! ```html
//! <script type="module" src="./src/entry.client.tsx"></script>
//! <link rel="stylesheet" href="./styles.css" />
//! ```
//!
//! Those two lines are the build's inputs. What is written out is the same
//! document with those references pointing at what was built, content-hashed:
//!
//! ```html
//! <script type="module" src="/assets/entry.client-B2v9Kq1x.js"></script>
//! <link rel="stylesheet" href="/assets/styles-9dfa03c1.css" />
//! ```
//!
//! Everything else in the file is untouched, and that is the point of it: the
//! title, the meta tags, the Open Graph block, the inline analytics snippet and
//! the favicon are the author's, and a tool that generated the document instead
//! would own all of them.
//!
//! # A relative path is an input; anything else is a URL
//!
//! `./src/entry.client.tsx` and `src/entry.client.tsx` name files in the
//! project. `/assets/vendor.js`, `https://…`, `//cdn…` and `data:` name things
//! the browser fetches from somewhere this build does not control, and are left
//! exactly as written. The rule is one line and it is the escape hatch: write a
//! rooted path for anything esdev should keep its hands off.
//!
//! # What is bundled, and what is copied
//!
//! A `<script type="module">` is an **entry**: it and everything it imports
//! become one bundle, under the browser conditions ([`crate::build`]). Anything
//! else a relative reference names — a stylesheet, a favicon, an image, a
//! classic script — is **copied** and hashed. A classic script cannot be
//! bundled, because it has no imports to follow; a stylesheet is not bundled
//! because there is no CSS pipeline here yet, and pretending otherwise would
//! produce a file that silently lost its `@import`s.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;

use lol_html::html_content::Element;
use lol_html::{RewriteStrSettings, element, rewrite_str};

use crate::config::Target;

/// Where a bundled entry, a copied stylesheet and a copied image all land,
/// under the output directory.
///
/// One directory, so a deployment can cache the whole of it immutably —
/// everything in it is content-hashed, which is the only thing that makes that
/// safe. The HTML itself is *not* in it, because the HTML is the one file whose
/// URL cannot change.
const ASSET_DIR: &str = "assets";

/// What an HTML file references, and what is to be done with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A `<script type="module">`: an entry to bundle.
    Module,
    /// A stylesheet, an image, a favicon, a classic script: a file to copy.
    Asset,
}

/// One reference found in the document.
struct Reference {
    /// The attribute value exactly as written — the key the rewrite pass looks
    /// itself up by, so a document that names the same file two ways gets both
    /// rewritten.
    url: String,
    kind: Kind,
}

/// Whether an attribute value names a file in this project.
///
/// The three "no"s are the ones that matter: a rooted path is a URL the
/// deployment already serves, a scheme-ful or protocol-relative URL is somebody
/// else's host, and a fragment or query-only value is not a file at all.
fn is_local(url: &str) -> bool {
    let url = url.trim();
    if url.is_empty() || url.starts_with('/') || url.starts_with('#') || url.starts_with('?') {
        return false;
    }
    // `data:`, `https:`, `mailto:` — anything with a scheme. A Windows path
    // (`C:\…`) is not something an HTML attribute should carry either.
    !url.split_once(':').is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.')
    })
}

/// Reads an element's reference, if it has one worth following.
fn reference(element: &Element<'_, '_>) -> Option<Reference> {
    let (attribute, kind) = match element.tag_name().as_str() {
        "script" => (
            "src",
            // The `type` is what decides whether this is a graph or a file.
            // Anything but `module` — a classic script, an import map, a JSON
            // block — is copied as it stands.
            match element.get_attribute("type").as_deref() {
                Some("module") => Kind::Module,
                _ => Kind::Asset,
            },
        ),
        "link" => ("href", Kind::Asset),
        "img" | "source" | "video" | "audio" => ("src", Kind::Asset),
        _ => return None,
    };
    let url = element.get_attribute(attribute)?;
    is_local(&url).then_some(Reference { url, kind })
}

/// The selector every handler is registered under.
///
/// One list, used by both passes, so what is *found* and what is *rewritten*
/// cannot drift apart.
const SELECTOR: &str = "script[src], link[href], img[src], source[src], video[src], audio[src]";

/// The attribute a tag carries its reference in.
fn attribute_of(tag: &str) -> &'static str {
    if tag == "link" { "href" } else { "src" }
}

/// Everything `html` references that this build is responsible for.
fn discover(html: &str) -> Result<Vec<Reference>, String> {
    let found = RefCell::new(Vec::new());
    rewrite_str(
        html,
        RewriteStrSettings::new().append_element_content_handler(element!(SELECTOR, |element| {
            if let Some(reference) = reference(element) {
                found.borrow_mut().push(reference);
            }
            Ok(())
        })),
    )
    .map_err(|e| format!("cannot parse the HTML: {e}"))?;
    Ok(found.into_inner())
}

/// Writes `html` back with every reference in `rewritten` replaced.
fn rewrite(html: &str, rewritten: &BTreeMap<String, String>) -> Result<String, String> {
    rewrite_str(
        html,
        RewriteStrSettings::new().append_element_content_handler(element!(SELECTOR, |element| {
            let attribute = attribute_of(element.tag_name().as_str());
            if let Some(url) = element.get_attribute(attribute)
                && let Some(replacement) = rewritten.get(&url)
            {
                element.set_attribute(attribute, replacement)?;
            }
            Ok(())
        })),
    )
    .map_err(|e| format!("cannot rewrite the HTML: {e}"))
}

/// A short content hash, for cache-busting a filename.
///
/// FNV-1a, and deliberately not a cryptographic digest: what this answers is
/// "has this file changed since the last deployment", which decides whether a
/// browser reuses a cached copy. Nothing trusts it, and nothing is protected by
/// it — a collision costs a stale stylesheet, not an exploit.
fn content_hash(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")[..8].to_string()
}

/// The name a copied file takes: its own, with a content hash before the
/// extension.
pub fn hashed_name(path: &Path, bytes: &[u8]) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("asset");
    let hash = content_hash(bytes);
    match path.extension().and_then(|e| e.to_str()) {
        Some(extension) => format!("{stem}-{hash}.{extension}"),
        None => format!("{stem}-{hash}"),
    }
}

/// Builds an HTML target: bundle what it imports, copy what it references,
/// write it back pointing at both.
///
/// `hash` is what an `esdev start` will turn off — a stable filename is what
/// lets a browser reload keep its cache warm and a developer read a stack
/// trace, and neither matters to a deployment, where an unhashed name is
/// instead the thing that serves last week's bundle to half your users.
pub async fn build(
    target: &Target,
    root: &Path,
    out_dir: &Path,
    hash: bool,
    minify: bool,
    defines: Vec<(String, String)>,
    conditions: Vec<String>,
) -> Result<String, String> {
    let entry = root.join(&target.entry);
    let html = std::fs::read_to_string(&entry)
        .map_err(|e| format!("cannot read {}: {e}", target.entry))?;
    // References are relative to the *document*, the way a browser reads them —
    // not to the project root, which is only the same directory by convention.
    let base = entry
        .parent()
        .map_or_else(|| root.to_path_buf(), Path::to_path_buf);

    let references = discover(&html)?;
    let assets = out_dir.join(ASSET_DIR);
    let mut rewritten: BTreeMap<String, String> = BTreeMap::new();
    let mut modules: Vec<(String, String)> = Vec::new();

    for reference in &references {
        let path = base.join(&reference.url);
        if !path.is_file() {
            return Err(format!(
                "{} references {}, which is not there.\n\n\
                 A relative path in an HTML file names a file in the project. For a \
                 URL this build should leave alone, write it rooted \
                 (/{}) or absolute.",
                target.entry,
                reference.url,
                reference.url.trim_start_matches("./")
            ));
        }
        match reference.kind {
            Kind::Module => {
                let name = Path::new(&reference.url)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| format!("{} does not name a module", reference.url))?
                    .to_string();
                if modules.iter().any(|(existing, _)| existing == &name) {
                    return Err(format!(
                        "{} has two module scripts whose files are both called {name}.\n\n\
                         They would be built to one file. Rename one — the output is \
                         named after the entry, so a collision is silent.",
                        target.entry
                    ));
                }
                modules.push((name, path.to_string_lossy().into_owned()));
            }
            Kind::Asset => {
                let bytes = std::fs::read(&path)
                    .map_err(|e| format!("cannot read {}: {e}", reference.url))?;
                let name = if hash {
                    hashed_name(&path, &bytes)
                } else {
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("asset")
                        .to_string()
                };
                std::fs::create_dir_all(&assets)
                    .map_err(|e| format!("cannot create {}: {e}", assets.display()))?;
                std::fs::write(assets.join(&name), &bytes)
                    .map_err(|e| format!("cannot write {name}: {e}"))?;
                rewritten.insert(reference.url.clone(), format!("/{ASSET_DIR}/{name}"));
            }
        }
    }

    let bundled = if modules.is_empty() {
        Vec::new()
    } else {
        crate::build::bundle_browser_entries(
            modules, root, &assets, hash, minify, defines, conditions,
        )
        .await?
    };
    for (name, filename) in &bundled {
        // Back to the URL that named it. The reference is found again by its
        // entry name rather than kept alongside, because the bundler is what
        // decides the output filename and it only speaks in entry names.
        if let Some(reference) = references.iter().find(|reference| {
            reference.kind == Kind::Module
                && Path::new(&reference.url)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    == Some(name)
        }) {
            rewritten.insert(reference.url.clone(), format!("/{ASSET_DIR}/{filename}"));
        }
    }

    let document = rewrite(&html, &rewritten)?;
    std::fs::create_dir_all(out_dir)
        .map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;
    let name = entry
        .file_name()
        .ok_or_else(|| format!("{} does not name a file", target.entry))?;
    let written = out_dir.join(name);
    std::fs::write(&written, document)
        .map_err(|e| format!("cannot write {}: {e}", written.display()))?;

    let scripts = bundled.len();
    let copied = rewritten.len() - scripts;
    Ok(format!(
        "{} ({scripts} script{}, {copied} asset{})",
        written
            .strip_prefix(root)
            .unwrap_or(&written)
            .to_string_lossy(),
        if scripts == 1 { "" } else { "s" },
        if copied == 1 { "" } else { "s" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_path_is_an_input_and_everything_else_is_a_url() {
        assert!(is_local("./src/entry.client.tsx"));
        assert!(is_local("src/entry.client.tsx"));
        assert!(is_local("../shared/styles.css"));

        // Already a URL the deployment serves, or somebody else's host.
        assert!(!is_local("/assets/vendor.js"));
        assert!(!is_local("https://cdn.example.com/a.js"));
        assert!(!is_local("//cdn.example.com/a.js"));
        assert!(!is_local("data:text/css,body{}"));
        assert!(!is_local("#main"));
        assert!(!is_local(""));
    }

    #[test]
    fn a_module_script_is_an_entry_and_the_rest_are_files() {
        let found = discover(
            r#"<html><head>
                 <link rel="stylesheet" href="./styles.css">
                 <link rel="icon" href="/favicon.ico">
                 <script type="module" src="./src/main.tsx"></script>
                 <script src="./legacy.js"></script>
                 <script src="https://cdn.example.com/a.js"></script>
                 <script>console.log("inline")</script>
               </head><body><img src="./logo.png"></body></html>"#,
        )
        .expect("parsed");

        let urls: Vec<&str> = found.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "./styles.css",
                "./src/main.tsx",
                "./legacy.js",
                "./logo.png"
            ]
        );
        assert_eq!(found[1].kind, Kind::Module);
        // A classic script has no imports to follow, so it is a file.
        assert_eq!(found[2].kind, Kind::Asset);
    }

    /// Everything that is not a reference is the author's, and survives byte
    /// for byte — the title, the meta tags, the inline script.
    #[test]
    fn rewriting_touches_only_the_references() {
        let html = r#"<!doctype html>
<html lang="en"><head>
<meta charset="utf-8">
<title>My App</title>
<script>window.__EARLY__ = 1;</script>
<link rel="stylesheet" href="./styles.css">
</head><body><div id="root"></div>
<script type="module" src="./src/main.tsx"></script>
</body></html>"#;
        let mut rewritten = BTreeMap::new();
        rewritten.insert("./styles.css".to_string(), "/assets/styles-abc.css".into());
        rewritten.insert("./src/main.tsx".to_string(), "/assets/main-def.js".into());

        let out = rewrite(html, &rewritten).expect("rewritten");
        assert!(out.contains(r#"href="/assets/styles-abc.css""#), "{out}");
        assert!(out.contains(r#"src="/assets/main-def.js""#), "{out}");
        assert!(out.contains("<title>My App</title>"), "{out}");
        assert!(out.contains("window.__EARLY__ = 1;"), "{out}");
        assert!(out.contains(r#"<html lang="en">"#), "{out}");
    }

    #[test]
    fn a_hash_follows_the_content_and_keeps_the_name() {
        let name = hashed_name(Path::new("/p/styles.css"), b"body{}");
        assert!(name.starts_with("styles-"), "{name}");
        assert!(name.ends_with(".css"), "{name}");
        assert_ne!(
            name,
            hashed_name(Path::new("/p/styles.css"), b"body{color:red}")
        );
        assert_eq!(name, hashed_name(Path::new("/p/styles.css"), b"body{}"));
    }
}
