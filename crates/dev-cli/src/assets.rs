//! `import logo from "./logo.png"` — an asset in the module graph.
//!
//! # Why the copy was not enough
//!
//! A target's `assets` list copies files into the output, and for a favicon or
//! a `robots.txt` that is the whole answer: nothing imports them, they are
//! referenced by name from a document, and their names must not change. What it
//! cannot do is the case where a *module* is the thing that knows about the
//! file:
//!
//! * **The name cannot be hashed**, so a changed image keeps its URL and a
//!   cache serves last week's one. Every other output of this build — the
//!   bundle, its chunks, the stylesheet — is content-hashed for exactly that
//!   reason ([`crate::html`]).
//! * **The build does not know the file exists.** A component that references
//!   an image only in its own source has to be remembered about in `esdev.json`
//!   as well, and the failure for forgetting is a 404 in production rather than
//!   anything the build says. Delete the component and the file ships for ever.
//! * **CSS already disagreed with it.** `background: url(./logo.png)` in a
//!   stylesheet *is* followed, hashed and emitted ([`crate::css::bundle`]), so
//!   one file referenced two ways got two different treatments.
//!
//! # What this does
//!
//! A `load` hook, filtered to the extensions below, which turns the file into a
//! one-line module:
//!
//! ```js
//! export default "/assets/logo-1a2b3c4d.png";
//! ```
//!
//! …and records the file so the caller can write it. The bytes are **not**
//! carried through the bundler: what is recorded is the source path and the name
//! it takes, and the copy happens once the build has succeeded — a video in a
//! `Vec<u8>` for the length of a bundle is a cost with no purpose.
//!
//! # One URL, for both halves of an app
//!
//! The URL is **rooted** — `/assets/…` — in every build, browser and server
//! alike, and that is the point rather than an oversight. A component that
//! renders on the server and hydrates in the browser is in both bundles, and the
//! markup the server sends has to name the file the browser fetches. A
//! module-relative URL would be correct for one of them and wrong for the other.
//!
//! It is also what [`crate::html`] already writes for the bundle it emits, so
//! the document and the modules agree about where the output is served from.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::contract::{self, Answer, Filter, HookSpec, Hooks, ModuleResult, Pattern};

/// What this pass is called wherever a diagnostic names it.
pub const PASS_NAME: &str = "esdev:assets";

/// Why a `--lib` build claims an asset import and then refuses it.
///
/// Printed by [`crate::build`] rather than returned from the hook, because the
/// bundler renders a plugin failure as "plugin `x` threw an error" and keeps
/// only that line — what the pass itself said is the cause underneath, and the
/// cause is dropped.
pub const WHY_A_LIBRARY_REFUSES: &str = "\
An asset import needs a URL, and --lib cannot know one: a library is an input to \
somebody else's build, and where that build serves a file from is its own \
decision. An application build emits /assets/<name>-<hash>.<ext> because it knows \
where its output goes.

Ship the file beside your package and let the consumer reference it, or inline it \
in the source as a data: URL.";

/// The directory an emitted asset lands in, under the build's output.
///
/// [`crate::html::ASSET_DIR`]'s, deliberately: a document's hashed bundle and a
/// module's hashed image are the same kind of thing to whatever serves them.
pub const ASSET_DIR: &str = crate::html::ASSET_DIR;

/// What an import has to end in for this pass to claim it.
///
/// A list rather than "anything that is not JavaScript", because claiming an
/// unknown extension would swallow the diagnostic for a typo: `./util.tsx` with
/// a missing `x` should be an unresolved import, not a module exporting a URL
/// to a file that is not there.
///
/// Images, fonts, media and documents — the files a component references.
/// `.json` is not among them: the bundler parses JSON into a module already, and
/// a JSON file that became a URL would break every import of a config.
const ASSET_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "avif", "ico", "bmp", "svg", "woff", "woff2", "ttf",
    "otf", "eot", "mp4", "webm", "ogv", "mp3", "wav", "ogg", "flac", "pdf", "zip", "wasm",
];

/// The assets one build referenced, and the names they take in its output.
///
/// Shared with the pass rather than returned from it, for the reason
/// [`crate::cssmodules::Collected`] is: a hook has nowhere to return something
/// the bundle does not contain.
#[derive(Debug, Default, Clone)]
pub struct Emitted(Arc<Mutex<Vec<Asset>>>);

/// One file a module imported.
#[derive(Debug, Clone)]
pub struct Asset {
    /// Where it is now.
    pub source: PathBuf,
    /// What it is called in the output — its own name with a content hash.
    pub name: String,
}

impl Emitted {
    pub fn new() -> Self {
        Self::default()
    }

    fn record(&self, asset: Asset) {
        if let Ok(mut held) = self.0.lock()
            && !held.iter().any(|existing| existing.name == asset.name)
        {
            held.push(asset);
        }
    }

    /// Everything recorded so far.
    pub fn take(&self) -> Vec<Asset> {
        self.0.lock().map(|held| held.clone()).unwrap_or_default()
    }

    /// Writes each recorded asset into `dir`, and reports how many.
    ///
    /// After the bundle rather than during it: a build that fails writes
    /// nothing, and an asset copied by a build that then failed is a file in the
    /// output directory that no build put a reference to (D78).
    ///
    /// A name that is already there is already this file: the name carries a
    /// hash of the bytes. So the copy is skipped, which is what keeps a dev
    /// loop's rebuild from re-copying every image on every keystroke.
    pub fn write(&self, dir: &Path) -> Result<usize, String> {
        let assets = self.take();
        if assets.is_empty() {
            return Ok(0);
        }
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        for asset in &assets {
            let target = dir.join(&asset.name);
            if target.is_file() {
                continue;
            }
            std::fs::copy(&asset.source, &target).map_err(|e| {
                format!(
                    "cannot copy {} to {}: {e}",
                    asset.source.display(),
                    target.display()
                )
            })?;
        }
        Ok(assets.len())
    }
}

/// The pass itself.
#[derive(Debug)]
pub struct Assets {
    /// Where to record what was imported, or `None` for a build that may not
    /// answer the question at all — see [`Assets::refusing`].
    emitted: Option<Emitted>,
    hooks: Hooks,
}

impl Assets {
    pub fn new(emitted: Emitted) -> Self {
        Self::with(Some(emitted))
    }

    /// The pass a `--lib` build installs: it claims the same files and refuses
    /// them.
    ///
    /// **Because the URL is the consuming build's to decide.** An application
    /// build knows where its output is served from and can emit
    /// `/assets/logo-1a2b3c4d.png`; a library is an input to a build that has
    /// not run yet, whose asset handling, output directory and public path are
    /// all its own. Emitting a rooted URL here would publish a package that
    /// works only where esdev's own layout happens to be served.
    ///
    /// Refused rather than left alone, because leaving it alone is what
    /// produced the failure this whole pass exists to fix: the bundler reads
    /// the image as source and reports that it is not valid UTF-8.
    pub fn refusing() -> Self {
        Self::with(None)
    }

    fn with(emitted: Option<Emitted>) -> Self {
        let pattern = format!(r"\.({})$", ASSET_EXTENSIONS.join("|"));
        Self {
            emitted,
            hooks: Hooks {
                load: Some(HookSpec {
                    filter: Filter {
                        id: vec![Pattern::Regex(
                            regex::Regex::new(&pattern).expect("a generated pattern"),
                        )],
                        code: Vec::new(),
                    },
                    ..HookSpec::default()
                }),
                ..Hooks::default()
            },
        }
    }
}

impl contract::Pass for Assets {
    fn name(&self) -> &str {
        PASS_NAME
    }

    fn hooks(&self) -> &Hooks {
        &self.hooks
    }

    fn load<'a>(
        &'a self,
        id: &'a str,
        _ctx: &'a Arc<dyn contract::Context>,
    ) -> Answer<'a, Option<ModuleResult>> {
        Box::pin(async move {
            let path = Path::new(id);
            let Some(emitted) = &self.emitted else {
                // Short, because the backend keeps only the first line of it —
                // [`WHY_A_LIBRARY_REFUSES`] is what the reader actually gets.
                return Err(format!("{id} is an asset, and --lib emits none"));
            };
            let bytes = std::fs::read(path).map_err(|e| format!("cannot read {id}: {e}"))?;
            let name = crate::html::hashed_name(path, &bytes);
            emitted.record(Asset {
                source: path.to_path_buf(),
                name: name.clone(),
            });
            Ok(Some(ModuleResult {
                // JSON-encoded, so a filename with a quote or a backslash in it
                // produces a string rather than a syntax error.
                code: format!(
                    "export default {};\n",
                    serde_json::Value::String(format!("/{ASSET_DIR}/{name}"))
                ),
                module_type: Some("js".to_string()),
                map: None,
                depends_on: Vec::new(),
            }))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_files_a_component_references_are_claimed_and_source_is_not() {
        let pass = Assets::new(Emitted::new());
        let claims = |id: &str| {
            contract::Pass::hooks(&pass)
                .load
                .as_ref()
                .expect("a load hook")
                .filter
                .id
                .iter()
                .any(|pattern| match pattern {
                    Pattern::Regex(regex) => regex.is_match(id),
                    Pattern::Exact(exact) => exact == id,
                })
        };

        assert!(claims("/p/src/logo.png"));
        assert!(claims("/p/src/fonts/Inter.woff2"));
        assert!(!claims("/p/src/util.ts"));
        // A stylesheet is a module the CSS Modules pass claims, not a file.
        assert!(!claims("/p/src/styles.css"));
        // The bundler makes a module out of JSON already, and a config that
        // became a URL would break every import of one.
        assert!(!claims("/p/src/data.json"));
    }

    /// One file imported by three modules is one file in the output.
    #[test]
    fn the_same_asset_is_recorded_once() {
        let emitted = Emitted::new();
        for _ in 0..3 {
            emitted.record(Asset {
                source: PathBuf::from("/p/logo.png"),
                name: "logo-1a2b3c4d.png".to_string(),
            });
        }
        assert_eq!(emitted.take().len(), 1);
    }

    /// Two files whose names collide are told apart by their content hash, so
    /// `a/icon.svg` and `b/icon.svg` do not overwrite one another.
    #[test]
    fn different_bytes_take_different_names() {
        let one = crate::html::hashed_name(Path::new("icon.svg"), b"<svg>a</svg>");
        let two = crate::html::hashed_name(Path::new("icon.svg"), b"<svg>b</svg>");
        assert_ne!(one, two);
        assert!(one.starts_with("icon-") && one.ends_with(".svg"), "{one}");
    }
}
