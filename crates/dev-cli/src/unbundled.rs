//! A module `esdev` runs unbundled, put through the passes a build installs.
//!
//! `esdev test`, `--dom`, `esdev <file>` and global setup load modules one at
//! a time through the runtime's loader rather than through a bundler. What a
//! module *is* has to be the same either way (D148): a component whose `.jsx`
//! is compiled by a framework's plugin, a route map a plugin generates, a
//! stylesheet a component imports, an image. So the loader asks the same passes
//! the bundler would, in the same order:
//!
//! ```text
//!   import "./page.jsx"
//!     resolve   esdev's passes, then the project's plugins — pre, normal, post
//!               first answer wins; none → the runtime's own resolution
//!     load      the same, first answer wins; none → the file is read
//!     transform every pass whose filter admits the module, in order
//!     compile   types stripped, JSX the project's way (crate::transform)
//! ```
//!
//! # What does not cross
//!
//! `start`, `end` and `bundle` describe a bundle, and there is none. A hook's
//! `ctx.emit()` fails, because nothing is written. `ctx.resolve()` finds a
//! relative or absolute path the way a bundler would and answers `null` for a
//! package: the loader's own resolution is not reachable from the plugin
//! isolate's side.
//!
//! # Why it blocks
//!
//! The runtime asks for a module's resolution and source synchronously
//! (`import.meta.resolve` has nowhere to await), and a hook's answer comes from
//! another thread — the plugin isolate. Waiting on it here is what every module
//! load already does for the file read; the isolate never waits on this thread,
//! so there is nothing for the two to deadlock over.
//!
//! # A module with no file
//!
//! A hook may answer `resolve` with an id that names no file (`virtual: true`,
//! or anything that is not an absolute path). It is loaded under
//! [`VIRTUAL`], a scheme of esdev's own, so it cannot be mistaken for a file
//! and the runtime's file read never sees it. What it imports resolves against
//! the project root, since it has no directory of its own.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use es_runtime_cli_common::run::{ModuleHooks, SourceTransform};

use crate::contract::{self, Hook};
use crate::plugins::wait;

/// The scheme a module with no file behind it is loaded under.
pub const VIRTUAL: &str = "esdev-virtual:";

/// The passes for a module run unbundled, and the compiler after them.
pub struct Pipeline {
    /// esdev's own passes first, then the project's plugins, as a build
    /// installs them. Each hook orders them by its own `order`.
    passes: Vec<Arc<dyn contract::Pass>>,
    /// What compiles the module once the passes are done with it.
    then: Arc<dyn SourceTransform>,
    ctx: Arc<dyn contract::Context>,
    root: PathBuf,
    /// What a `load` answered, for the `transform` of the same module: the
    /// type it said the code is, and the map that starts the chain.
    loaded: Mutex<HashMap<String, Loaded>>,
}

struct Loaded {
    module_type: Option<String>,
    map: Option<String>,
}

/// The pipeline every unbundled run in this project loads through: the
/// project's plugins behind esdev's own passes, and `then` after them.
///
/// `site` is what the plugins read as `ctx.command`, `ctx.platform`,
/// `ctx.target` and `ctx.hot`.
pub async fn pipeline(
    source: &crate::settings::Source,
    site: &contract::Site,
    then: Arc<dyn SourceTransform>,
) -> Result<Arc<Pipeline>, String> {
    let mut passes: Vec<Arc<dyn contract::Pass>> = vec![
        // What they collect is dropped: nothing is written by a run, and a
        // name or a URL is all an importing module sees.
        Arc::new(crate::cssmodules::CssModules::new(
            &source.root,
            crate::cssmodules::Collected::new(),
            false,
        )),
        Arc::new(crate::assets::Assets::new(crate::assets::Emitted::new())),
    ];
    if let Some(host) = crate::plugins::host(source).await? {
        passes.extend(host.passes(site));
    }
    Ok(Arc::new(Pipeline::new(passes, &source.root, then)))
}

impl Pipeline {
    pub fn new(
        passes: Vec<Arc<dyn contract::Pass>>,
        root: &Path,
        then: Arc<dyn SourceTransform>,
    ) -> Pipeline {
        Pipeline {
            passes,
            then,
            ctx: Arc::new(RunContext {
                dir: root.to_path_buf(),
            }),
            root: root.to_path_buf(),
            loaded: Mutex::new(HashMap::new()),
        }
    }

    /// The passes that declared `hook`, in the order a build calls them: `pre`,
    /// then unordered, then `post`, each in the order they were installed.
    fn ordered(&self, hook: Hook) -> Vec<&Arc<dyn contract::Pass>> {
        let mut passes: Vec<_> = self
            .passes
            .iter()
            .filter(|pass| pass.hooks().get(hook).is_some())
            .collect();
        passes.sort_by_key(|pass| match pass.hooks().get(hook).map(|h| h.order) {
            Some(contract::Order::Pre) => 0,
            Some(contract::Order::Post) => 2,
            _ => 1,
        });
        passes
    }

    fn admits(pass: &Arc<dyn contract::Pass>, hook: Hook, id: &str, code: Option<&str>) -> bool {
        pass.hooks()
            .get(hook)
            .is_some_and(|spec| spec.filter.admits(id, code))
    }

    /// `source` after every pass whose filter admits `id`, and the maps of the
    /// passes that changed it, in the order they ran — starting with the map a
    /// `load` returned.
    ///
    /// The maps are `None` when a pass changed the module without one: the
    /// chain back to the file is broken, and a stack frame is better left on
    /// the compiled line than moved to a wrong one.
    fn through_passes(
        &self,
        id: &str,
        mut source: String,
    ) -> Result<(String, Option<Vec<String>>), String> {
        let loaded = self.loaded.lock().ok().and_then(|mut held| held.remove(id));
        let mut module_type = loaded
            .as_ref()
            .and_then(|loaded| loaded.module_type.clone())
            .unwrap_or_else(|| module_type(id));
        let mut maps: Option<Vec<String>> =
            Some(loaded.and_then(|loaded| loaded.map).into_iter().collect());
        for pass in self.ordered(Hook::Transform) {
            if !Self::admits(pass, Hook::Transform, id, Some(&source)) {
                continue;
            }
            let answer = wait(pass.transform(&source, id, &module_type, &self.ctx))
                .map_err(|e| format!("[plugin {}] {id}\n{e}", pass.name()))?;
            if let Some(result) = answer {
                if result.code != source {
                    match (&mut maps, result.map) {
                        (Some(maps), Some(map)) => maps.push(map),
                        _ => maps = None,
                    }
                }
                source = result.code;
                if let Some(changed) = result.module_type {
                    module_type = changed;
                }
            }
        }
        Ok((source, maps))
    }

    /// The id a pass names a module by: a path for a file, the id a `resolve`
    /// invented for a virtual module. `None` for what is this binary's own, or
    /// anything else no pass has a say in.
    fn pass_id(specifier: &str) -> Option<String> {
        if let Some(id) = specifier.strip_prefix(VIRTUAL) {
            return Some(id.to_string());
        }
        url::Url::parse(specifier)
            .ok()
            .filter(|url| url.scheme() == "file")
            .and_then(|mut url| {
                url.set_query(None);
                url.set_fragment(None);
                url.to_file_path().ok()
            })
            .map(|path| path.to_string_lossy().into_owned())
    }
}

impl ModuleHooks for Pipeline {
    fn resolve(&self, specifier: &str, referrer: &str) -> Result<Option<String>, String> {
        // `runtime:` is this binary's own namespace; nothing is resolved there
        // but the modules it ships.
        if specifier.starts_with("runtime:") {
            return Ok(None);
        }
        let importer = Self::pass_id(referrer);
        for pass in self.ordered(Hook::Resolve) {
            if !Self::admits(pass, Hook::Resolve, specifier, None) {
                continue;
            }
            let answer = wait(pass.resolve(
                specifier,
                importer.as_deref(),
                referrer.is_empty(),
                &self.ctx,
            ))
            .map_err(|e| format!("[plugin {}] {specifier}\n{e}", pass.name()))?;
            let contract::Resolved::To {
                id,
                external,
                virtual_module,
            } = answer
            else {
                continue;
            };
            // External means "not part of the bundle"; unbundled, that is
            // every import, and the runtime resolves it as written.
            if external != contract::External::No {
                return Ok(None);
            }
            if virtual_module || !Path::new(&id).is_absolute() {
                return Ok(Some(format!("{VIRTUAL}{id}")));
            }
            return url::Url::from_file_path(&id)
                .map(|url| Some(url.to_string()))
                .map_err(|()| {
                    format!(
                        "[plugin {}] resolved {specifier} to {id}, which is not a path",
                        pass.name()
                    )
                });
        }
        Ok(None)
    }

    fn referrer(&self, referrer: &str) -> Option<String> {
        // A file in the project root that is not there: relative specifiers
        // resolve against the root, and packages from the project's own
        // `node_modules`.
        referrer.starts_with(VIRTUAL).then(|| {
            url::Url::from_file_path(self.root.join("[virtual]"))
                .map(|url| url.to_string())
                .unwrap_or_default()
        })
    }

    fn load(&self, specifier: &str) -> Result<Option<String>, String> {
        let virtual_module = specifier.starts_with(VIRTUAL);
        let Some(id) = Self::pass_id(specifier) else {
            return Ok(None);
        };
        for pass in self.ordered(Hook::Load) {
            if !Self::admits(pass, Hook::Load, &id, None) {
                continue;
            }
            let answer = wait(pass.load(&id, &self.ctx))
                .map_err(|e| format!("[plugin {}] {id}\n{e}", pass.name()))?;
            if let Some(result) = answer {
                if let Ok(mut held) = self.loaded.lock() {
                    held.insert(
                        id,
                        Loaded {
                            module_type: result.module_type,
                            map: result.map,
                        },
                    );
                }
                return Ok(Some(result.code));
            }
        }
        if virtual_module {
            return Err(format!(
                "{id} was resolved by a plugin as a module with no file, and no plugin's \
                 `load` answered for it"
            ));
        }
        Ok(None)
    }
}

impl SourceTransform for Pipeline {
    fn reserved_query(&self) -> Option<&'static str> {
        self.then.reserved_query()
    }

    fn transform(&self, specifier: &str, source: String) -> Result<String, String> {
        // A mocked module is not the file's source at all, and the compiler
        // after this replaces it whole; a pass has nothing to say about it.
        if crate::module_mocks::synthetic(specifier).is_some() {
            return self.then.transform(specifier, source);
        }
        let Some(id) = Self::pass_id(specifier) else {
            return self.then.transform(specifier, source);
        };
        let (source, maps) = self.through_passes(&id, source)?;
        let path = Path::new(&id);
        let Some(mut chain) = maps
            .filter(|maps| !maps.is_empty())
            .filter(|_| !specifier.starts_with(VIRTUAL))
        else {
            return self.then.transform(specifier, source);
        };
        // What the compiler registers maps its output to the passes' output,
        // not to the file. Forgotten first, so a map registered by an earlier
        // load of this file is not mistaken for this one's.
        es_runtime_cli_common::sourcemap::forget(path);
        let compiled = self.then.transform(specifier, source)?;
        chain.extend(es_runtime_cli_common::sourcemap::registered(path));
        if let Some(whole) = collapse(&chain) {
            es_runtime_cli_common::sourcemap::register(path, &whole);
        }
        Ok(compiled)
    }
}

/// One map from the end of `chain` back to its start, as JSON: each map names
/// positions in the text the one before it produced.
fn collapse(chain: &[String]) -> Option<String> {
    let parsed = chain
        .iter()
        .map(|json| {
            rolldown_sourcemap::OwnedSourceMap::from_json_string(json)
                .ok()
                .map(rolldown_sourcemap::SourceMap::from)
        })
        .collect::<Option<Vec<_>>>()?;
    match parsed.as_slice() {
        [] => None,
        [one] => Some(one.to_json_string()),
        many => {
            let refs: Vec<_> = many.iter().collect();
            Some(rolldown_sourcemap::collapse_sourcemaps(&refs).to_json_string())
        }
    }
}

/// What a module is before any pass has changed it, spelled the way the
/// bundler would tell a hook: its extension, with the JavaScript and
/// TypeScript family names folded to the four the compiler knows.
fn module_type(id: &str) -> String {
    match Path::new(id).extension().and_then(|e| e.to_str()) {
        Some("js" | "mjs" | "cjs") => "js".to_string(),
        Some("ts" | "mts" | "cts") => "ts".to_string(),
        Some(other) => other.to_string(),
        None => "js".to_string(),
    }
}

/// What a hook's `ctx` can do in a run.
///
/// A run is not a build: nothing is bundled, so there is no graph to add an
/// entry to and no output to put an asset beside.
struct RunContext {
    dir: PathBuf,
}

impl contract::Context for RunContext {
    fn resolve<'a>(
        &'a self,
        specifier: &'a str,
        importer: Option<&'a str>,
        _skip_self: bool,
    ) -> contract::Answer<'a, Option<contract::ResolvedId>> {
        Box::pin(async move { Ok(resolve_file(specifier, importer)) })
    }

    fn emit(&self, _emit: contract::Emit) -> Result<String, String> {
        Err(
            "ctx.emit() adds to a build's output, and a run unbundled writes none — \
             this hook cannot emit under esdev test or esdev <file>"
                .to_string(),
        )
    }

    fn log(&self, level: &str, message: String) {
        eprintln!("esdev: plugin {level}: {message}");
    }

    fn depends_on(&self, _file: &str) {}

    fn cwd(&self) -> PathBuf {
        self.dir.clone()
    }
}

/// A relative or absolute specifier, as the file it names — with the
/// extensions and `index` files a bundler would try. A package is left
/// unresolved: `None` is an answer a plugin already has to handle.
fn resolve_file(specifier: &str, importer: Option<&str>) -> Option<contract::ResolvedId> {
    const EXTENSIONS: [&str; 6] = ["ts", "tsx", "mts", "js", "jsx", "mjs"];
    let base = if specifier.starts_with('/') {
        PathBuf::from(specifier)
    } else if specifier.starts_with("./") || specifier.starts_with("../") {
        Path::new(importer?).parent()?.join(specifier)
    } else {
        return None;
    };
    let candidates = std::iter::once(base.clone())
        .chain(EXTENSIONS.iter().map(|ext| {
            let mut name = base.clone().into_os_string();
            name.push(format!(".{ext}"));
            PathBuf::from(name)
        }))
        .chain(
            EXTENSIONS
                .iter()
                .map(|ext| base.join(format!("index.{ext}"))),
        );
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .and_then(|path| dunce::canonicalize(path).ok())
        .map(|path| contract::ResolvedId {
            id: path.to_string_lossy().into_owned(),
            external: false,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hook is told what a module is in the bundler's vocabulary, so a
    /// plugin written against a build reads the same `ctx.type` in a test.
    #[test]
    fn a_modules_type_is_what_the_bundler_would_call_it() {
        assert_eq!(module_type("/a/b.mjs"), "js");
        assert_eq!(module_type("/a/b.mts"), "ts");
        assert_eq!(module_type("/a/b.jsx"), "jsx");
        assert_eq!(module_type("/a/b.mdx"), "mdx");
    }

    /// `ctx.resolve()` in a run finds what a bundler would, and leaves a
    /// package for the plugin's own fallback.
    #[test]
    fn a_run_resolves_files_the_way_a_bundler_would() {
        let dir =
            std::env::temp_dir().join(format!("esdev-unbundled-resolve-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(dir.join("util.ts"), "").unwrap();
        std::fs::write(dir.join("lib/index.js"), "").unwrap();
        let importer = dir.join("main.ts").to_string_lossy().into_owned();
        let found = |specifier| resolve_file(specifier, Some(&importer)).map(|r| r.id);
        assert!(found("./util").unwrap().ends_with("util.ts"));
        assert!(found("./lib").unwrap().ends_with("index.js"));
        assert!(found("./missing").is_none());
        assert!(found("some-package").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A pass names a file by its path and a virtual module by the id it was
    /// given; the binary's own modules are nobody's.
    #[test]
    fn a_pass_names_modules_by_path_or_by_their_own_id() {
        let file = url::Url::from_file_path(std::env::temp_dir().join("a.jsx"))
            .unwrap()
            .to_string();
        assert!(
            Pipeline::pass_id(&format!("{file}?actual"))
                .unwrap()
                .ends_with("a.jsx")
        );
        assert_eq!(
            Pipeline::pass_id(&format!("{VIRTUAL}@app/routes")).as_deref(),
            Some("@app/routes")
        );
        assert_eq!(Pipeline::pass_id("runtime:test"), None);
    }

    /// A chain of one map is that map; a broken map breaks the chain.
    #[test]
    fn a_chain_collapses_to_one_map() {
        let map = r#"{"version":3,"sources":["a.js"],"names":[],"mappings":";;;AAAA"}"#;
        let one = collapse(&[map.to_string()]).unwrap();
        assert!(one.contains("a.js"), "{one}");
        assert!(collapse(&[map.to_string(), "not json".to_string()]).is_none());
        assert!(collapse(&[]).is_none());
    }
}
