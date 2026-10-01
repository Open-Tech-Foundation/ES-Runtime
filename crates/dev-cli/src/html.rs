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
//! Everything else in the file is untouched — *byte for byte*, because that is
//! literally what happens: the tokenizer reports the byte span of each
//! attribute it found, and what is written out is the original text with those
//! spans spliced. Nothing is re-serialised, so the title, the meta tags, the
//! Open Graph block, the inline analytics snippet, the author's choice of
//! quoting and their trailing whitespace all survive. A tool that parsed this to
//! a tree and printed it back would own every one of those.
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
//! become one bundle, under the browser conditions ([`crate::build`]). A
//! `<link rel="stylesheet">` is an entry too: it and everything it `@import`s
//! become one stylesheet ([`crate::css`]). Anything else a relative reference
//! names — a favicon, an image, a classic script — is **copied** and hashed,
//! because a classic script has no imports to follow and an image has nothing
//! to resolve.
//!
//! A stylesheet's own `url()` references are copied as well, and it is pointed
//! at where they landed. That is not a nicety: bundling moves the file to
//! `assets/`, and a relative `url()` is resolved by the browser against
//! wherever the file *is*, so a stylesheet that moved without them would arrive
//! pointing at nothing.

use std::ops::Range;
use std::path::Path;

use html5gum::emitters::default::DefaultEmitter;
use html5gum::{Token, Tokenizer};

use crate::config::Target;

/// Where a bundled entry, a copied stylesheet and a copied image all land,
/// under the output directory.
///
/// One directory, so a deployment can cache the whole of it immutably —
/// everything in it is content-hashed, which is the only thing that makes that
/// safe. The HTML itself is *not* in it, because the HTML is the one file whose
/// URL cannot change.
pub const ASSET_DIR: &str = "assets";

/// What an HTML file references, and what is to be done with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A `<script type="module">`: an entry to bundle.
    Module,
    /// A stylesheet, an image, a favicon, a classic script: a file to copy.
    Asset,
}

/// Where a reference points, which decides whether this build owns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Where {
    /// A relative path: a file in this project, and an input to this build.
    Project,
    /// A rooted path: a URL the deployment already serves. Left exactly as
    /// written — but checked, because it is also how a page comes to build
    /// nothing at all ([`misrooted`]).
    Rooted,
}

/// One reference found in the document.
struct Reference {
    /// The attribute value, entity references already decoded — so
    /// `href="./a&amp;b.css"` names the file `a&b.css`, which is the one on
    /// disk.
    url: String,
    kind: Kind,
    at: Where,
    /// The attribute this came from, to write back: `src` or `href`.
    attribute: &'static str,
    /// Where the whole attribute sits in the source, so the rewrite is a splice
    /// rather than a re-serialisation.
    span: Range<usize>,
}

/// Whether an attribute value names a file this build has anything to do with,
/// and on what terms.
///
/// The two "no"s are the ones that matter: a scheme-ful or protocol-relative
/// URL is somebody else's host, and a fragment or query-only value is not a
/// file at all. A rooted path is neither — it is a URL of this deployment's,
/// and this build leaves it alone.
fn origin(url: &str) -> Option<Where> {
    let url = url.trim();
    if url.is_empty() || url.starts_with('#') || url.starts_with('?') {
        return None;
    }
    if url.starts_with("//") {
        return None;
    }
    if url.starts_with('/') {
        return Some(Where::Rooted);
    }
    // `data:`, `https:`, `mailto:` — anything with a scheme. A Windows path
    // (`C:\…`) is not something an HTML attribute should carry either.
    let scheme_ful = url.split_once(':').is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.')
    });
    (!scheme_ful).then_some(Where::Project)
}

/// A rooted reference that names a **source file of this project** — which is
/// almost always the wrong spelling, and is silent in the worst way.
///
/// `src="/src/main.jsx"` is what Vite's own React template ships, so it is what
/// people arrive with. Here a rooted path is a URL the deployment serves, so
/// the reference is left alone and the entry it names is never built: the
/// document is written out pointing at a file that was not produced, the target
/// reports zero scripts, and the build succeeds. An empty page that exits 0 is
/// the outcome that costs an afternoon.
///
/// What makes it decidable rather than a guess is the `assets` list. A rooted
/// URL the assets copy will satisfy — `/styles.css` with `"assets":
/// ["styles.css"]`, or with a `public/` directory holding it — is a correct
/// spelling and stays one. What is left is a rooted URL naming a file that
/// nothing copies to the output, which is a 404 in production whatever this
/// build thinks of it.
fn misrooted(url: &str, root: &Path, assets: &[String]) -> Option<String> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let relative = path.trim_start_matches('/');
    if relative.is_empty() || !root.join(relative).is_file() {
        return None;
    }
    // A file asset lands at the output root under its own *name*; a directory
    // asset lands as its contents, so a path inside it is a path from the root.
    let copied = assets.iter().any(|asset| {
        let from = root.join(asset);
        if from.is_dir() {
            from.join(relative).exists()
        } else {
            Path::new(asset)
                .file_name()
                .is_some_and(|name| name == std::ffi::OsStr::new(relative))
                && from.is_file()
        }
    });
    (!copied).then(|| relative.to_string())
}

/// The attribute a tag carries a reference in, and what that reference is.
///
/// `<script type="module">` is the only entry: a classic script has no imports
/// to follow, so it is a file like any other.
fn interesting(tag: &str, is_module: bool) -> Option<(&'static str, Kind)> {
    match tag {
        "script" => Some(("src", if is_module { Kind::Module } else { Kind::Asset })),
        "link" => Some(("href", Kind::Asset)),
        "img" | "source" | "video" | "audio" => Some(("src", Kind::Asset)),
        _ => None,
    }
}

/// Everything `html` references that this build is responsible for, with where
/// each reference sits in the text.
///
/// **The tokenizer switches states for raw-text elements**, which is what keeps
/// this from finding references that are not references: a URL inside a
/// `<script>` string, inside a CSS comment in a `<style>`, or inside a
/// `<textarea>` is text, and a scan that treated it as markup would try to build
/// a file the page never asked for. Comments are skipped for the same reason —
/// commented-out markup is not markup.
///
/// Parse errors are ignored rather than fatal. They are the spec's own
/// recoverable ones (an unescaped `&`, a stray `<`), every browser renders such
/// a page, and a build tool that refused it would be stricter than the thing the
/// page is written for.
fn discover(html: &str) -> Vec<Reference> {
    let mut emitter: DefaultEmitter<usize> = DefaultEmitter::new_with_span();
    emitter.naively_switch_states(true);
    let mut found = Vec::new();

    for token in Tokenizer::new_with_emitter(html, emitter).flatten() {
        let Token::StartTag(tag) = token else {
            continue;
        };
        let name = String::from_utf8_lossy(&tag.name).into_owned();
        let is_module = tag
            .attributes
            .get(b"type".as_slice())
            .is_some_and(|value| value.value.as_slice() == b"module");
        let Some((attribute, kind)) = interesting(&name, is_module) else {
            continue;
        };
        let Some(value) = tag.attributes.get(attribute.as_bytes()) else {
            continue;
        };
        let url = String::from_utf8_lossy(&value.value).into_owned();
        let Some(at) = origin(&url) else {
            continue;
        };
        found.push(Reference {
            url,
            kind,
            at,
            attribute,
            // An unquoted value's span runs to whatever ended it — the `>` or
            // the space before the next attribute — because that is the byte
            // the tokenizer learned it was over on. Trimmed here so the splice
            // replaces the attribute and not the character after it.
            span: value.span.start..trim_terminator(html, value.span.start..value.span.end),
        });
    }
    found
}

/// The end of an attribute, given a span that may run one character past it.
fn trim_terminator(html: &str, span: Range<usize>) -> usize {
    let text = &html[span.clone()];
    span.start
        + text
            .trim_end_matches(['>', '/', ' ', '\t', '\n', '\r'])
            .len()
}

/// Writes `html` back with each reference's attribute replaced.
///
/// Applied last-first so that every span still describes the text it was found
/// in: a replacement changes the offsets of everything after it and nothing
/// before it.
fn splice(html: &str, replacements: &[(&Reference, String)]) -> String {
    let mut ordered: Vec<&(&Reference, String)> = replacements.iter().collect();
    ordered.sort_by_key(|(reference, _)| std::cmp::Reverse(reference.span.start));

    let mut document = html.to_string();
    for (reference, url) in ordered {
        document.replace_range(
            reference.span.clone(),
            &format!("{}=\"{}\"", reference.attribute, escape(url)),
        );
    }
    document
}

/// An attribute value, safe to write between double quotes.
///
/// The URLs this writes are built from a filename and a hash, so in practice
/// there is nothing to escape — but "in practice" is not a reason to emit a
/// document that a stylesheet called `a"b.css` would break.
fn escape(url: &str) -> String {
    url.replace('&', "&amp;").replace('"', "&quot;")
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

/// The few lines that make a document reload itself, injected only by the dev
/// loop.
///
/// Three things live in here, and all three are worth the bytes.
///
/// **A WebSocket**, where server-sent events used to be: the channel carries
/// hot updates now rather than the word "reload". What that costs is the
/// reconnect loop below — `EventSource` provided it for free, a WebSocket
/// does not, and a dev server being restarted is the ordinary case rather
/// than a failure. A page that gave up after the first drop would look broken
/// for the rest of the session. Backoff, because the socket also fails while
/// esdev is down and a tight retry would spin a core; capped, because somebody
/// who fixes the thing should not then wait a minute for the page to notice.
///
/// **The stylesheet swap**, which is the whole point of a `css` update: the
/// replacement is inserted and only then is the old one removed, on its
/// `load`. Removing first would leave the document unstyled for a frame,
/// which is a flash of white on every save — worse than the reload this is
/// avoiding. The old link is also removed on `error`, or a stylesheet that
/// 404s would leave two of them behind on every save until the page had a
/// hundred.
///
/// **The error overlay**: a failed build leaves the page on its last good
/// output, which from the browser looks exactly like a save that did nothing.
/// The `error` message shows what the terminal shows instead, full-screen,
/// until the next patch, swap or reload clears it. Plain inline styles under
/// an `__esdev_` id — no classes for the application's CSS to collide with —
/// and `textContent`, never `innerHTML`, because the message holds code.
///
/// It is `esdev`'s endpoint rather than the application's, so no template ships
/// dev-only code and nothing has to be stripped from it later.
fn reload_client(port: u16) -> String {
    // Two things live in here, and both are worth the bytes.
    //
    // **Reconnection**, because `EventSource` used to provide it and a
    // WebSocket does not, and a dev server being restarted is the ordinary case
    // rather than a failure — a page that gave up after the first drop would
    // look broken for the rest of the session. Backoff, because the socket also
    // fails while esdev is down and a tight retry would spin a core; capped,
    // because somebody who fixes the thing should not then wait a minute for the
    // page to notice.
    //
    // **The stylesheet swap**, which is the whole point of a `css` update: the
    // replacement is inserted and only then is the old one removed, on its
    // `load`. Removing first would leave the document unstyled for a frame,
    // which is a flash of white on every save — worse than the reload this is
    // avoiding. The old link is also removed on `error`, or a stylesheet that
    // 404s would leave two of them behind on every save until the page had a
    // hundred.
    format!(
        "\n<script>\
         (function(){{\
         var wait=250;\
         function css(){{\
         var links=document.querySelectorAll('link[rel=\"stylesheet\"]');\
         for(var i=0;i<links.length;i++){{(function(old){{\
         var next=old.cloneNode();\
         next.href=old.href.split('?')[0]+'?t='+Date.now();\
         var drop=function(){{if(old.parentNode)old.parentNode.removeChild(old);}};\
         next.onload=drop;next.onerror=drop;\
         old.parentNode.insertBefore(next,old.nextSibling);\
         }})(links[i]);}}\
         }}\
         function patch(m,tries){{\
         var hot=globalThis.__esdev_hot;\
         if(!hot){{location.reload();return;}}\
         var el=document.createElement('script');\
         el.type='module';el.src=m.url;\
         el.onerror=function(){{\
         el.remove();\
         if((tries||0)<10)setTimeout(function(){{patch(m,(tries||0)+1);}},100);\
         else location.reload();\
         }};\
          el.onload=function(){{if(!hot.apply(m.changedIds))location.reload();}};\
          document.head.appendChild(el);\
          }}\
          {overlay}\
          function open(){{\
          var s=new WebSocket(\"ws://127.0.0.1:{port}{path}\");\
          s.onopen=function(){{wait=250;}};\
          s.onmessage=function(e){{\
          var m;try{{m=JSON.parse(e.data);}}catch(_){{return;}}\
          if(m.type===\"error\"){{err(m);return;}}\
          clearErr();\
          if(m.type===\"css\")css();\
          else if(m.type===\"patch\")patch(m,0);\
          else if(m.type===\"reload\")location.reload();\
          }};\
          s.onclose=function(){{setTimeout(open,wait);wait=Math.min(wait*2,5000);}};\
          s.onerror=function(){{s.close();}};\
          }}\
          open();\
          }})();\
          </script>\n",
        path = crate::devserver::HMR_PATH,
        overlay = overlay_script(),
    )
}

/// Shows a build failure full-screen, and clears it on the next update.
///
/// Plain text, not a format string: it is interpolated into the client as
/// data, so its braces ship as written. Built once and reused: a second
/// failure while one is showing replaces the text rather than stacking
/// overlays. `textContent`, so code in the message cannot become markup;
/// inline styles under an `__esdev_` id, so the application's CSS cannot
/// reach it. No close button — like Vite's and Next's overlays, the fix
/// dismisses it, which is the next message.
fn overlay_script() -> &'static str {
    "\
    function clearErr(){\
    var d=document.getElementById('__esdev_err');\
    if(d&&d.parentNode)d.parentNode.removeChild(d);\
    }\
    function err(m){\
    var d=document.getElementById('__esdev_err');\
    if(!d){\
    d=document.createElement('div');d.id='__esdev_err';\
    d.style.cssText='position:fixed;inset:0;z-index:99999;background:rgba(24,4,4,.94);color:#ffd9d9;padding:24px;overflow:auto;font:14px/1.5 monospace;';\
    var h=document.createElement('div');\
    h.textContent='Build failed \\u2014 fix and save';\
    h.style.cssText='font-weight:bold;margin-bottom:12px;';\
    var p=document.createElement('pre');p.id='__esdev_err_pre';\
    p.style.cssText='margin:0;white-space:pre-wrap;word-break:break-word;';\
    d.appendChild(h);d.appendChild(p);\
    (document.body||document.documentElement).appendChild(d);\
    }\
    document.getElementById('__esdev_err_pre').textContent=m.message||'Build failed';\
    }"
}

/// Where the reload script goes: just before `</body>`, or `</html>`, or at the
/// end.
///
/// Found by tokenizing rather than by searching for the text, because
/// `"</body>"` inside a `<script>` string is not the end of the body — and a
/// document with no `</body>` at all is valid, which is why there are three
/// answers.
/// Where a `<link>` for the JavaScript-imported stylesheets goes: the end of
/// `<head>`, so it is fetched with the bundle rather than after it.
///
/// After any stylesheet the document already links, because a CSS Module is
/// scoped to one component and a global sheet is the baseline it sits on —
/// which is the order that makes a tie between them resolve the way an author
/// expects.
fn head_end(html: &str) -> Option<usize> {
    let mut emitter: DefaultEmitter<usize> = DefaultEmitter::new_with_span();
    emitter.naively_switch_states(true);
    for token in Tokenizer::new_with_emitter(html, emitter).flatten() {
        if let Token::EndTag(tag) = token
            && tag.name.as_slice() == b"head"
        {
            return Some(tag.span.start);
        }
    }
    None
}

/// The document after the plugins' `html` hooks for one stage (D151): `pre`
/// when `output` is `None`, the others — unordered, then `post` — when it is
/// what the build produced. Each sees what the one before it returned.
async fn through_html_hooks(
    passes: &[std::sync::Arc<dyn crate::contract::Pass>],
    mut document: String,
    id: &str,
    output: Option<&[crate::contract::Output]>,
    ctx: &std::sync::Arc<dyn crate::contract::Context>,
) -> Result<String, String> {
    use crate::contract::{Hook, Order};
    let mut stage: Vec<_> = passes
        .iter()
        .filter(|pass| {
            pass.hooks().get(Hook::Html).is_some_and(|spec| {
                (spec.order == Order::Pre) == output.is_none() && spec.filter.admits(id, None)
            })
        })
        .collect();
    stage.sort_by_key(|pass| {
        pass.hooks()
            .get(Hook::Html)
            .is_some_and(|spec| spec.order == Order::Post)
    });
    for pass in stage {
        let Some(answer) = pass
            .html(&document, id, output, ctx)
            .await
            .map_err(|e| format!("[plugin {}] {id}\n{e}", pass.name()))?
        else {
            continue;
        };
        if let Some(html) = answer.html {
            document = html;
        }
        for place in [
            crate::contract::InjectTo::HeadPrepend,
            crate::contract::InjectTo::Head,
            crate::contract::InjectTo::BodyPrepend,
            crate::contract::InjectTo::Body,
        ] {
            let markup: String = answer
                .tags
                .iter()
                .filter(|tag| tag.inject_to == place)
                .map(crate::contract::HtmlTag::render)
                .collect();
            if !markup.is_empty() {
                document.insert_str(tag_point(&document, place), &markup);
            }
        }
    }
    Ok(document)
}

/// Where a tag a plugin asked for goes. A document missing the element it
/// names gets the tag where the browser would have put that element: a head's
/// before everything else, a body's at the end.
fn tag_point(html: &str, place: crate::contract::InjectTo) -> usize {
    use crate::contract::InjectTo;
    let mut emitter: DefaultEmitter<usize> = DefaultEmitter::new_with_span();
    emitter.naively_switch_states(true);
    let (mut head_open, mut head_close, mut body_open) = (None, None, None);
    for token in Tokenizer::new_with_emitter(html, emitter).flatten() {
        match token {
            Token::StartTag(tag) if tag.name.as_slice() == b"head" => {
                head_open = head_open.or(Some(tag.span.end));
            }
            Token::StartTag(tag) if tag.name.as_slice() == b"body" => {
                body_open = body_open.or(Some(tag.span.end));
            }
            Token::EndTag(tag) if tag.name.as_slice() == b"head" => {
                head_close = head_close.or(Some(tag.span.start));
            }
            _ => {}
        }
    }
    match place {
        InjectTo::HeadPrepend => head_open.or(head_close).unwrap_or(0),
        InjectTo::Head => head_close.or(head_open).unwrap_or(0),
        InjectTo::BodyPrepend => body_open.unwrap_or_else(|| injection_point(html)),
        InjectTo::Body => injection_point(html),
    }
}

fn injection_point(html: &str) -> usize {
    let mut emitter: DefaultEmitter<usize> = DefaultEmitter::new_with_span();
    emitter.naively_switch_states(true);
    let mut body = None;
    let mut end = None;
    for token in Tokenizer::new_with_emitter(html, emitter).flatten() {
        if let Token::EndTag(tag) = token {
            match tag.name.as_slice() {
                b"body" => body = body.or(Some(tag.span.start)),
                b"html" => end = end.or(Some(tag.span.start)),
                _ => {}
            }
        }
    }
    body.or(end).unwrap_or(html.len())
}

/// Whether a referenced file is a stylesheet, and so an entry rather than a
/// file to copy.
///
/// By extension rather than by the `rel="stylesheet"` on the tag: a `<link>`
/// can carry several `rel` values, `rel` is optional on the ones that matter
/// least, and a `.css` file is a stylesheet however it was linked. The
/// extension is also what decides this in every other build tool, which makes
/// it the answer somebody will guess correctly.
fn is_stylesheet(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "css")
}

/// Bundles a stylesheet and writes out the files it references.
///
/// Returns the CSS to write, which is deliberately *not* written here: its
/// content hash has to be computed from these bytes, after every `url()` has
/// been pointed at a real name. Hashing the source instead would leave a
/// stylesheet whose name never changed when the file it `@import`s did — a
/// stale-cache bug that only appears in production, and only for the people who
/// visited before the change.
fn stylesheet(
    path: &Path,
    source: Option<String>,
    assets: &Path,
    hash: bool,
    minify: bool,
    sources: &mut usize,
    written: &mut usize,
) -> Result<Vec<u8>, String> {
    let bundled = crate::css::build_source(path, source, minify)?;
    // Read here rather than by the bundler, so no bundler knows to watch it:
    // the linked file itself (a `.scss` included) and every `@import` behind it.
    crate::build::record_stylesheet_files(
        std::iter::once(path.to_path_buf()).chain(bundled.read_files.iter().cloned()),
    );
    let mut code = bundled.code;
    // Every stylesheet that went in, not every `<link>` that named one: an
    // `@import` is a file this build read, and counting tags instead would
    // report the same "1 stylesheet" whether it resolved three of them or none.
    *sources += bundled.sources;
    // The files it pulled in are files this build wrote, and the summary is a
    // count of those. Left out, a stylesheet that dragged in a font and three
    // images would report as one asset.
    *written += bundled.referenced.len();

    for referenced in &bundled.referenced {
        let bytes = std::fs::read(&referenced.path)
            .map_err(|e| format!("cannot read {}: {e}", referenced.path.display()))?;
        let name = if hash {
            hashed_name(&referenced.path, &bytes)
        } else {
            referenced
                .path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("asset")
                .to_string()
        };
        std::fs::create_dir_all(assets)
            .map_err(|e| format!("cannot create {}: {e}", assets.display()))?;
        std::fs::write(assets.join(&name), &bytes)
            .map_err(|e| format!("cannot write {name}: {e}"))?;
        // Rooted, like every other URL this build writes, so it resolves the
        // same from a page at `/` and a page at `/posts/1`.
        code = code.replace(&referenced.placeholder, &format!("/{ASSET_DIR}/{name}"));
    }

    Ok(code.into_bytes())
}

/// Builds an HTML target: bundle what it imports, copy what it references,
/// write it back pointing at both.
#[allow(
    clippy::too_many_arguments,
    reason = "every one is a distinct build setting; a struct would only move the list"
)]
pub async fn build(
    target: &Target,
    root: &Path,
    out_dir: &Path,
    dev: Option<&crate::build::Dev>,
    minify: bool,
    defines: Vec<(String, String)>,
    conditions: Vec<String>,
    alias: Vec<(String, String)>,
    sourcemap: Option<String>,
    plugins: &[std::sync::Arc<dyn crate::contract::Pass>],
    jsx: crate::contract::Jsx,
    jsx_settings: crate::transform::JsxSettings,
    tsconfig: Option<std::path::PathBuf>,
) -> Result<(String, Vec<crate::contract::Output>), String> {
    let hash = dev.is_none();
    let entry = root.join(&target.entry);
    let html = std::fs::read_to_string(&entry)
        .map_err(|e| format!("cannot read {}: {e}", target.entry))?;
    // What a plugin's hook is handed while a file is read here rather than by
    // the bundler.
    let ctx = crate::unbundled::run_context(root);
    let id = entry.to_string_lossy().into_owned();
    // The plugins' `pre` stage sees the source, so what they add to it is
    // built as though the author had written it (D151).
    let html = through_html_hooks(plugins, html, &id, None, &ctx).await?;
    // References are relative to the *document*, the way a browser reads them —
    // not to the project root, which is only the same directory by convention.
    let base = entry
        .parent()
        .map_or_else(|| root.to_path_buf(), Path::to_path_buf);

    let references = discover(&html);
    let assets = out_dir.join(ASSET_DIR);
    // Keyed by where the reference sits, so a document naming the same file
    // twice gets both occurrences rewritten and neither is looked up by a URL
    // string that two tags might spell differently.
    let mut rewritten: Vec<(&Reference, String)> = Vec::new();
    let mut modules: Vec<(String, String)> = Vec::new();
    // Counted for the summary, and counted directly rather than derived from
    // `rewritten`: what a `<link>` becomes is no longer one file, so the tags in
    // the document have stopped being a count of anything.
    let mut styled = 0usize;
    let mut pulled_in = 0usize;
    let mut copied = 0usize;

    for reference in &references {
        if reference.at == Where::Rooted {
            if let Some(named) = misrooted(&reference.url, root, &target.assets) {
                return Err(format!(
                    "{} references {}, and {named} is a file in this project.\n\n\
                     A rooted path is a URL the deployment already serves, so this build \
                     leaves it alone — nothing is built from it, and the page written out \
                     points at a file that was never produced. Write ./{named} to build \
                     it, or add it to this target's `assets` if the deployment really \
                     serves it as it is.",
                    target.entry, reference.url,
                ));
            }
            continue;
        }
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
                // Offered to the plugins first, as an imported stylesheet is
                // (D149): a linked `.scss` a plugin compiles to CSS is a
                // stylesheet, and a `.css` a plugin rewrites is bundled from
                // what it wrote. What comes out as anything else is copied as
                // the file it is.
                let compiled = crate::unbundled::through(plugins, &path, &ctx)
                    .await?
                    .filter(|(_, kind)| kind == "css")
                    .map(|(code, _)| code);
                let (bytes, named) = if compiled.is_some() || is_stylesheet(&path) {
                    (
                        stylesheet(
                            &path,
                            compiled,
                            &assets,
                            hash,
                            minify,
                            &mut styled,
                            &mut pulled_in,
                        )?,
                        path.with_extension("css"),
                    )
                } else {
                    copied += 1;
                    (
                        std::fs::read(&path)
                            .map_err(|e| format!("cannot read {}: {e}", reference.url))?,
                        path.clone(),
                    )
                };
                let name = if hash {
                    hashed_name(&named, &bytes)
                } else {
                    named
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("asset")
                        .to_string()
                };
                std::fs::create_dir_all(&assets)
                    .map_err(|e| format!("cannot create {}: {e}", assets.display()))?;
                std::fs::write(assets.join(&name), &bytes)
                    .map_err(|e| format!("cannot write {name}: {e}"))?;
                rewritten.push((reference, format!("/{ASSET_DIR}/{name}")));
            }
        }
    }

    // Collected by the bundler's CSS Modules plugin: what the JavaScript
    // imported has no place in a JavaScript bundle, so it comes back here to be
    // written as a stylesheet and linked below. Returned rather than filled into
    // a handle passed down, because in the dev loop the plugin outlives the
    // build and keeps whichever handle it was constructed with.
    let crate::build::BrowserBundle {
        entries: bundled,
        sheets,
        assets: imported,
        produced,
    } = if modules.is_empty() {
        crate::build::BrowserBundle {
            entries: Vec::new(),
            sheets: Vec::new(),
            assets: 0,
            produced: Vec::new(),
        }
    } else {
        crate::build::bundle_browser_entries(
            modules,
            root,
            &assets,
            dev.is_some(),
            minify,
            defines,
            conditions,
            alias,
            sourcemap,
            dev.filter(|dev| dev.hot).map(|_| hot_runtime()),
            // Only in a hot dev loop, and only when a plugin asked for them.
            // The registrations the compiler inserts call globals that only a
            // hot loop installs, so emitting them anywhere else would ship
            // calls to something undefined — and a build that replaces nothing
            // has no component state to keep anyway.
            if dev.is_some_and(|dev| dev.hot) {
                jsx
            } else {
                crate::contract::Jsx::default()
            },
            jsx_settings.clone(),
            tsconfig.clone(),
            plugins,
        )
        .await?
    };
    for (name, filename) in &bundled {
        // Back to the tag that named it. The reference is found again by its
        // entry name rather than kept alongside, because the bundler is what
        // decides the output filename and it only speaks in entry names.
        for reference in references.iter().filter(|reference| {
            reference.kind == Kind::Module
                && Path::new(&reference.url)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    == Some(name.as_str())
        }) {
            rewritten.push((reference, format!("/{ASSET_DIR}/{filename}")));
        }
    }

    let mut document = splice(&html, &rewritten);

    // The stylesheet the imported `.module.css` files became. Linked rather
    // than injected from script: a `<style>` written at runtime costs a flash
    // of unstyled content and needs `style-src 'unsafe-inline'`, which the
    // template's own policy does not grant.
    if !sheets.is_empty() {
        // Each sheet's `url()`s are still placeholders. They are substituted
        // here, for the same reason a `<link>`ed stylesheet's are: the CSS is
        // about to move into `assets/`, and a relative `url()` moves with it.
        let mut parts = Vec::new();
        for sheet in sheets {
            let mut code = sheet.code;
            for referenced in &sheet.referenced {
                let bytes = std::fs::read(&referenced.path)
                    .map_err(|e| format!("cannot read {}: {e}", referenced.path.display()))?;
                let name = if hash {
                    hashed_name(&referenced.path, &bytes)
                } else {
                    referenced
                        .path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("asset")
                        .to_string()
                };
                std::fs::create_dir_all(&assets)
                    .map_err(|e| format!("cannot create {}: {e}", assets.display()))?;
                std::fs::write(assets.join(&name), &bytes)
                    .map_err(|e| format!("cannot write {name}: {e}"))?;
                code = code.replace(&referenced.placeholder, &format!("/{ASSET_DIR}/{name}"));
                pulled_in += 1;
            }
            parts.push(code);
        }
        let bytes = parts.join("\n").into_bytes();
        let name = if hash {
            hashed_name(Path::new("modules.css"), &bytes)
        } else {
            "modules.css".to_string()
        };
        std::fs::create_dir_all(&assets)
            .map_err(|e| format!("cannot create {}: {e}", assets.display()))?;
        std::fs::write(assets.join(&name), &bytes)
            .map_err(|e| format!("cannot write {name}: {e}"))?;
        styled += 1;

        let link = format!("<link rel=\"stylesheet\" href=\"/{ASSET_DIR}/{name}\">");
        match head_end(&document) {
            Some(at) => document.insert_str(at, &link),
            // A document with no `</head>` is one the browser will still give a
            // head to; putting the link first is the closest this can get.
            None => document.insert_str(0, &link),
        }
    }

    // The rest of the plugins see the written document, and what the build
    // produced, named from the output directory: a `modulepreload` for a chunk
    // is `/` + its name.
    let listing: Vec<crate::contract::Output> = produced
        .into_iter()
        .map(|output| match output {
            crate::contract::Output::Chunk {
                file_name,
                name,
                is_entry,
                is_dynamic_entry,
                facade_module_id,
                module_ids,
                imports,
                dynamic_imports,
            } => crate::contract::Output::Chunk {
                file_name: format!("{ASSET_DIR}/{file_name}"),
                name,
                is_entry,
                is_dynamic_entry,
                facade_module_id,
                module_ids,
                imports: imports
                    .into_iter()
                    .map(|import| format!("{ASSET_DIR}/{import}"))
                    .collect(),
                dynamic_imports: dynamic_imports
                    .into_iter()
                    .map(|import| format!("{ASSET_DIR}/{import}"))
                    .collect(),
            },
            crate::contract::Output::Asset { file_name } => crate::contract::Output::Asset {
                file_name: format!("{ASSET_DIR}/{file_name}"),
            },
        })
        .collect();
    let mut document = through_html_hooks(plugins, document, &id, Some(&listing), &ctx).await?;

    if let Some(dev) = dev {
        // Into the *output*, never the source. The file the developer edits is
        // never written to by a build. After the plugins, so one replacing the
        // document cannot drop it.
        document.insert_str(injection_point(&document), &reload_client(dev.reload_port));
    }
    std::fs::create_dir_all(out_dir)
        .map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;
    let name = entry
        .file_name()
        .ok_or_else(|| format!("{} does not name a file", target.entry))?;
    let written = out_dir.join(name);
    std::fs::write(&written, document)
        .map_err(|e| format!("cannot write {}: {e}", written.display()))?;

    let scripts = bundled.len();
    // Three ways a file lands in the output and one count for them: the
    // target's `assets` list, the files a stylesheet referenced, and the files
    // a *module* imported.
    let copied = copied + pulled_in + imported;
    let report = format!(
        "{} ({scripts} script{}, {styled} stylesheet{}, {copied} asset{})",
        written
            .strip_prefix(root)
            .unwrap_or(&written)
            .to_string_lossy(),
        if scripts == 1 { "" } else { "s" },
        if styled == 1 { "" } else { "s" },
        if copied == 1 { "" } else { "s" }
    );
    // What a `finish` hook is shown (D153): the bundle, and the document.
    let mut files = listing;
    files.push(crate::contract::Output::Asset {
        file_name: name.to_string_lossy().into_owned(),
    });
    Ok((report, files))
}

/// The transport half of the client's hot-update runtime, compiled into the
/// browser bundle.
///
/// rolldown injects the other half — the module graph, the factory registry and
/// the module cache — and stops there. Its patch assembler says why, and it is
/// worth quoting because it is this function's whole reason to exist: *"no
/// driver tail: the client walks its own graph, removes from its cache, and
/// re-runs from the factory map."* Loading a patch registers new factories and
/// changes nothing else. **Deciding what to re-run is ours, and so is the API a
/// framework hooks into.**
///
/// # The walk
///
/// From each changed module, climb the importer graph looking for a module
/// willing to be re-run — one that called `accept()` for itself, or
/// `accept(dep)` naming the importee the change came through. That module is a
/// *boundary*: everything from the change up to it is dropped and the boundary
/// re-runs.
///
/// Reaching a module with no importers means the climb hit an entry with nobody
/// willing, and the answer is a reload. That is not a failure — it is what
/// should happen when nothing says how to replace itself.
///
/// Re-running is `initModule`, not `loadExports`. The runtime is explicit that
/// `initModule` is *"the one re-execution gate"*; `loadExports` only reads the
/// cache and returns `{}` when it is empty. Using the latter after dropping the
/// cache is a walk that finds its boundary, drops everything, then runs none of
/// it — the page keeps its state, calls the callback, and shows the old code.
///
/// # Two things here that the ecosystem does not have
///
/// **`import.meta.hot.signal` is an `AbortSignal`,** aborted immediately before
/// the module is replaced. The commonest hot-reload bug in any framework is a
/// listener or a timer registered on every re-run and torn down on none, so the
/// twentieth save has twenty of them; the usual cure is remembering to write a
/// `dispose` callback that undoes by hand what the module did. The platform
/// already solved this, generally, and the whole web platform already takes the
/// solution as an argument:
///
/// ```js
/// addEventListener("resize", onResize, { signal: import.meta.hot.signal });
/// ```
///
/// That listener is now correct under replacement with **no HMR-specific code
/// at all** — the same line works in a plain build, where the signal is simply
/// never aborted. `fetch`, `addEventListener`, observers and any well-written
/// library accept a signal, so the fix generalises without this runtime knowing
/// what any of them are. It is the same instinct as the rest of this project:
/// reach for the standard name rather than invent a branded one.
///
/// **`import.meta.hot.keep(key, make)` is one call site, not two.** Carrying
/// state across a replacement conventionally means writing into a bag in
/// `dispose` and reading it back at the top of the module — two places that
/// have to agree, and the failure when they do not is silent state loss. Here
/// the value is made once and returned every time after:
///
/// ```js
/// const cache = import.meta.hot.keep("cache", () => new Map());
/// ```
///
/// `dispose(cb)` and `data` are both still here, because a framework porting an
/// integration from elsewhere expects them and there is no reason to make it
/// rewrite what already works.
fn hot_runtime() -> String {
    String::from(
        r#"
class EsdevHot {
  constructor(id, runtime, store) {
    this.moduleId = id;
    this._runtime = runtime;
    this._store = store;
    this.acceptCallbacks = [];
    this.disposeCallbacks = [];
    this.declined = false;
    this._aborter = new AbortController();
    // The bag `dispose` writes into and the next instance reads. Kept on the
    // store rather than on this context, because this context is what a
    // replacement throws away.
    this.data = store.data;
  }
  /** Aborted just before this module instance is replaced. */
  get signal() { return this._aborter.signal; }
  /**
   * `accept()` / `accept(cb)` accept this module. `accept(dep, cb)` and
   * `accept([deps], cb)` accept a change arriving through those importees --
   * rolldown rewrites the specifiers to stable ids at build time, so what
   * arrives here is already what the graph is keyed by.
   */
  accept(first, second) {
    if (first === undefined || typeof first === "function") {
      this.acceptCallbacks.push({ deps: [this.moduleId], fn: first || function () {} });
      return;
    }
    var deps = Array.isArray(first) ? first : [first];
    this.acceptCallbacks.push({ deps: deps, fn: second || function () {} });
  }
  /** Run before this instance is dropped. `signal` covers most of what this is for. */
  dispose(fn) { this.disposeCallbacks.push(fn); }
  /** Refuse replacement outright: any change reaching this module reloads. */
  decline() { this.declined = true; }
  /** Made once, returned on every replacement after. */
  keep(key, make) {
    if (!this._store.kept.has(key)) this._store.kept.set(key, make());
    return this._store.kept.get(key);
  }
  /** "I cannot handle this after all" -- try again from this module's importers. */
  invalidate() { this._runtime.esdevInvalidate(this.moduleId); }
  /** Called by the runtime, immediately before this instance is dropped. */
  _retire() {
    for (var i = 0; i < this.disposeCallbacks.length; i++) {
      try { this.disposeCallbacks[i](this.data); }
      catch (e) { console.error("[esdev] a dispose callback threw", e); }
    }
    this._aborter.abort();
  }
}

class EsdevRuntime extends DevRuntime {
  constructor(id) {
    super(id);
    this.moduleHotContexts = new Map();
    // Per module and outliving every instance of it: what `keep` holds and what
    // `data` is. A replacement makes a new context, never a new store.
    this._stores = new Map();
    this._invalidated = null;
  }
  _storeFor(id) {
    if (!this._stores.has(id)) this._stores.set(id, { data: {}, kept: new Map() });
    return this._stores.get(id);
  }
  createModuleHotContext(id) {
    var context = new EsdevHot(id, this, this._storeFor(id));
    this.moduleHotContexts.set(id, context);
    return context;
  }
  esdevInvalidate(id) {
    // Outside an update there is nothing to re-walk, so the honest answer is a
    // reload; during one it is a second attempt from this module's importers.
    if (this._invalidated) this._invalidated.push(id);
    else location.reload();
  }
  /**
   * The modules to re-run for `changedIds`, or `null` when nobody accepts and
   * the page has to reload.
   */
  _boundaries(changedIds, skip) {
    var seen = new Set(), drop = new Set(), plan = [], queue = [], i;
    for (i = 0; i < changedIds.length; i++) queue.push([changedIds[i], null]);
    while (queue.length) {
      var step = queue.shift(), id = step[0], via = step[1];
      if (seen.has(id)) continue;
      seen.add(id);
      var context = this.moduleHotContexts.get(id);
      if (context && context.declined) return null;
      if (context && !skip.has(id)) {
        var selfFns = [], depFns = [], entry;
        for (i = 0; i < context.acceptCallbacks.length; i++) {
          entry = context.acceptCallbacks[i];
          if (entry.deps.indexOf(id) > -1) selfFns.push(entry.fn);
          else if (via && entry.deps.indexOf(via) > -1) depFns.push(entry.fn);
        }
        // Accepting *itself*: this module re-runs, and its callbacks are told
        // with its own new exports.
        if (selfFns.length) {
          drop.add(id);
          plan.push({ rerun: id, fns: selfFns });
          continue;
        }
        // Accepting a *dependency*: the dependency re-runs and this module is
        // told, with the dependency's new exports. It does not re-run itself --
        // which is not a shortcut but the contract, and rolldown builds to the
        // same one: a patch for `accept(dep)` ships the dep's factory and not
        // this module's, so re-running this module is not merely wrong, it is
        // impossible.
        if (depFns.length) {
          drop.add(via);
          plan.push({ rerun: via, fns: depFns });
          continue;
        }
      }
      // Nothing here accepts, so the change is someone else's to handle. What
      // this module holds is stale either way, so it is dropped and re-made by
      // whoever above it does accept.
      drop.add(id);
      var importers = this.getImporters(id) || [];
      if (!importers.length) return null;
      for (i = 0; i < importers.length; i++) queue.push([importers[i], id]);
    }
    return plan.length ? { drop: drop, plan: plan } : null;
  }
  _rerun(found) {
    var self = this, i;
    // Captured before anything is dropped: re-running installs a new context,
    // and the callbacks that asked to be told belong to the old one.
    var pending = found.plan.map(function (step) {
      return { rerun: step.rerun, fns: step.fns.slice() };
    });
    found.drop.forEach(function (id) {
      var context = self.moduleHotContexts.get(id);
      if (context) context._retire();
      self.removeModuleCache(id);
    });
    for (i = 0; i < pending.length; i++) {
      var exports;
      try { exports = this.initModule(pending[i].rerun); }
      catch (e) { console.error("[esdev] re-running " + pending[i].rerun + " failed", e); return false; }
      for (var f = 0; f < pending[i].fns.length; f++) {
        try { pending[i].fns[f](exports); }
        catch (e) { console.error("[esdev] an accept callback threw", e); return false; }
      }
    }
    return true;
  }
  esdevApply(changedIds) {
    var ids = changedIds.slice(), skip = new Set(), rounds = 0;
    this._invalidated = [];
    try {
      // Bounded: a module that invalidates every time would otherwise walk for
      // ever, and a reload is a fine answer to a graph that cannot settle.
      while (rounds++ < 8) {
        var found = this._boundaries(ids, skip);
        if (!found || !this._rerun(found)) return false;
        if (!this._invalidated.length) return true;
        ids = this._invalidated.slice();
        for (var i = 0; i < ids.length; i++) skip.add(ids[i]);
        this._invalidated = [];
      }
      return false;
    } finally {
      this._invalidated = null;
    }
  }
}

globalThis.__rolldown_runtime__ ??= new EsdevRuntime("esdev");
(globalThis.__esdev_hot ||= {}).apply = function (changedIds) {
  try { return __rolldown_runtime__.esdevApply(changedIds); }
  catch (e) { console.error("[esdev] hot update failed, reloading", e); return false; }
};
"#,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plugin's tag lands where it said: after `<head …>`, before
    /// `</head>`, after `<body …>`, before `</body>`.
    #[test]
    fn a_tag_goes_where_it_was_asked_to() {
        use crate::contract::InjectTo;
        let html = "<!doctype html><html><head lang=\"en\"><title>t</title></head>\
                    <body class=\"x\"><p>hi</p></body></html>";
        let at = |place| &html[tag_point(html, place)..];
        assert!(at(InjectTo::HeadPrepend).starts_with("<title>"));
        assert!(at(InjectTo::Head).starts_with("</head>"));
        assert!(at(InjectTo::BodyPrepend).starts_with("<p>hi"));
        assert!(at(InjectTo::Body).starts_with("</body>"));
        // No head at all: the start, where the browser would put one.
        assert_eq!(tag_point("<p>x</p>", InjectTo::Head), 0);
    }

    /// The injected client shows build errors and clears them: the error
    /// branch, the dismissal on every other update, the overlay id — and no
    /// innerHTML (which would make code in the message markup). Brace balance
    /// is checked by hand with `node --check`, not here: the minified client
    /// legitimately nests closes (`}}`), so a substring assert cannot tell a
    /// balanced one from a broken one.
    #[test]
    fn the_client_shows_and_clears_build_errors() {
        let client = reload_client(5173);
        assert!(client.contains("m.type===\"error\""), "no error branch");
        assert!(client.contains("clearErr()"), "nothing dismisses it");
        assert!(client.contains("__esdev_err"), "no overlay");
        assert!(client.contains("textContent"), "unsafe rendering");
        assert!(!client.contains("innerHTML"), "markup from code");
        assert!(
            client.contains("ws://127.0.0.1:5173"),
            "the channel lost its port"
        );
    }

    #[test]
    fn a_relative_path_is_an_input_and_everything_else_is_a_url() {
        for input in [
            "./src/entry.client.tsx",
            "src/entry.client.tsx",
            "../shared/styles.css",
        ] {
            assert_eq!(origin(input), Some(Where::Project), "{input}");
        }
        // A URL of this deployment's: left alone, and checked rather than
        // dropped, because it is also how a page comes to build nothing.
        assert_eq!(origin("/assets/vendor.js"), Some(Where::Rooted));
        // Somebody else's host, or not a file at all.
        for other in [
            "https://cdn.example.com/a.js",
            "//cdn.example.com/a.js",
            "data:text/css,body{}",
            "#main",
            "",
        ] {
            assert_eq!(origin(other), None, "{other}");
        }
    }

    /// The Vite spelling. `src="/src/main.jsx"` builds nothing and exits 0,
    /// which is the outcome that costs an afternoon — so it is refused, and
    /// only when the rooted path names a file nothing copies to the output.
    #[test]
    fn a_rooted_path_naming_a_source_file_is_refused() {
        let root = std::env::temp_dir().join(format!("esdev-misrooted-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("public")).unwrap();
        std::fs::write(root.join("src/main.jsx"), "export {};").unwrap();
        std::fs::write(root.join("styles.css"), "body{}").unwrap();
        std::fs::write(root.join("public/logo.svg"), "<svg/>").unwrap();

        let assets = vec!["styles.css".to_string(), "public".to_string()];
        assert_eq!(
            misrooted("/src/main.jsx", &root, &assets),
            Some("src/main.jsx".to_string())
        );
        // A query or a fragment does not make it a different file.
        assert_eq!(
            misrooted("/src/main.jsx?v=2", &root, &assets),
            Some("src/main.jsx".to_string())
        );
        // What the assets copy will put at the output root is the correct
        // spelling and stays one — named as a file, or found inside a directory.
        assert_eq!(misrooted("/styles.css", &root, &assets), None);
        assert_eq!(misrooted("/logo.svg", &root, &assets), None);
        // Nothing of ours: a URL the deployment serves from somewhere else.
        assert_eq!(misrooted("/api/session", &root, &assets), None);
        assert_eq!(misrooted("/", &root, &assets), None);

        let _ = std::fs::remove_dir_all(&root);
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
        );

        // A rooted reference is discovered along with the rest — it is left as
        // written, but a build that dropped it here could not tell the person
        // who meant `./` about it. Somebody else's host is not discovered at
        // all, because nothing about it is ours to say.
        let urls: Vec<&str> = found.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "./styles.css",
                "/favicon.ico",
                "./src/main.tsx",
                "./legacy.js",
                "./logo.png"
            ]
        );
        assert_eq!(found[1].at, Where::Rooted);
        assert!(
            found
                .iter()
                .all(|r| r.url != "https://cdn.example.com/a.js")
        );
        assert_eq!(found[2].kind, Kind::Module);
        // A classic script has no imports to follow, so it is a file.
        assert_eq!(found[3].kind, Kind::Asset);
    }

    /// The reason this is a real tokenizer and not a search for `src=`. Each of
    /// these looks exactly like a reference and is text, and treating one as
    /// markup would fail the build on a page that is perfectly correct.
    #[test]
    fn a_url_that_is_text_rather_than_markup_is_not_a_reference() {
        let found = discover(
            r#"<script>var s = '<img src="./inside-a-string.png">';</script>
               <style>/* <link href="./inside-css.css"> */</style>
               <!-- <script src="./commented-out.js"></script> -->
               <textarea><img src="./inside-textarea.png"></textarea>
               <img src="./real.png" alt="a > b">"#,
        );
        let urls: Vec<&str> = found.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(urls, ["./real.png"], "{urls:?}");
    }

    /// Everything that is not a reference is the author's, and survives byte
    /// for byte — because the rewrite is a splice, not a re-serialisation.
    #[test]
    fn rewriting_touches_only_the_attributes_it_replaces() {
        let html = r#"<!DOCTYPE html>
<html lang="en"><head>
<meta charset='utf-8'>
<title>My App &mdash; home</title>
<script>window.__EARLY__ = 1 < 2;</script>
<link rel=stylesheet href=./styles.css>
</head><body><div id="root"   ></div>
<script type="module" src='./src/main.tsx'></script>
</body></html>"#;
        let found = discover(html);
        let replacements: Vec<(&Reference, String)> = found
            .iter()
            .map(|reference| {
                let url = if reference.kind == Kind::Module {
                    "/assets/main-def.js".to_string()
                } else {
                    "/assets/styles-abc.css".to_string()
                };
                (reference, url)
            })
            .collect();

        let out = splice(html, &replacements);
        assert!(
            out.contains(r#"<link rel=stylesheet href="/assets/styles-abc.css">"#),
            "{out}"
        );
        assert!(out.contains(r#"src="/assets/main-def.js">"#), "{out}");

        // The author's document, unchanged: their doctype casing, their single
        // quotes, their entity, their stray whitespace.
        assert!(out.contains("<!DOCTYPE html>"), "{out}");
        assert!(out.contains("<meta charset='utf-8'>"), "{out}");
        assert!(out.contains("<title>My App &mdash; home</title>"), "{out}");
        assert!(out.contains("window.__EARLY__ = 1 < 2;"), "{out}");
        assert!(out.contains(r#"<div id="root"   ></div>"#), "{out}");
    }

    /// An unquoted value's span runs to whatever ended it, so the splice would
    /// otherwise eat the `>` and weld two tags together.
    #[test]
    fn an_unquoted_attribute_keeps_what_followed_it() {
        for (html, expected) in [
            (
                r#"<link href=./a.css rel=stylesheet>"#,
                r#"<link href="/x.css" rel=stylesheet>"#,
            ),
            (r#"<link href=./a.css>"#, r#"<link href="/x.css">"#),
            (r#"<img src=./a.css/>"#, r#"<img src="/x.css"/>"#),
        ] {
            let found = discover(html);
            assert_eq!(found.len(), 1, "{html}");
            let out = splice(html, &[(&found[0], "/x.css".to_string())]);
            assert_eq!(out, expected);
        }
    }

    /// The URLs this writes come from a filename and a hash, so there is
    /// normally nothing to escape — which is not a reason to emit a document a
    /// file called `a"b.css` would break.
    #[test]
    fn a_replacement_is_safe_between_quotes() {
        assert_eq!(escape(r#"/assets/a"b&c.css"#), "/assets/a&quot;b&amp;c.css");
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
