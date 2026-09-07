//! Filesystem-backed [`ModuleLoader`] for local `file:` ES modules, plus a
//! deny-all loader (the default when no module capability is granted).
//!
//! Canonical module ids are `file://` URLs, so `import.meta.url` is a real URL
//! and relative imports resolve with WHATWG URL semantics (`.`/`..`
//! normalization, percent-encoding) via the `url` crate. Only relative (`./`,
//! `../`), absolute-path, and `file:` specifiers are accepted — bare specifiers
//! (`"lodash"`) and remote schemes (`http:`) are rejected (SPEC non-goals: no
//! npm/node resolution, no remote imports yet).
//!
//! This strict loader is **not** root-confined: a path may escape via
//! `..`/symlinks. That is by design — it is an embedder-only alternative for
//! callers wanting no package resolution, which must add their own confinement.
//! The CLI default (`NodeModuleLoader`) and `runtime:fs` are root-jailed (D25);
//! `esrun` does not use this loader.

use std::path::PathBuf;

use es_runtime_providers::{BoxFuture, ModuleLoader, ModuleSource, ProviderError};
use url::Url;

/// A [`ModuleLoader`] that resolves and reads ES modules from the local
/// filesystem.
pub struct FsModuleLoader {
    /// Base for resolving an entry point's relative specifier (`referrer == ""`):
    /// a `file://` directory URL.
    base: Url,
}

impl FsModuleLoader {
    /// Builds a loader whose entry-point base is the process working directory.
    pub fn new() -> Result<Self, ProviderError> {
        let cwd = std::env::current_dir()
            .map_err(|e| ProviderError::Other(format!("cannot read working directory: {e}")))?;
        Self::with_base_dir(cwd)
    }

    /// Builds a loader whose entry-point base is `dir` (must be absolute).
    pub fn with_base_dir(dir: impl AsRef<std::path::Path>) -> Result<Self, ProviderError> {
        let base = Url::from_directory_path(dir.as_ref()).map_err(|()| {
            ProviderError::Other(format!(
                "module base directory is not absolute: {}",
                dir.as_ref().display()
            ))
        })?;
        Ok(FsModuleLoader { base })
    }
}

/// Strict path/URL resolution — no I/O, so it is the same work either way the
/// loader is asked (D41).
fn resolve_core(specifier: &str, referrer: &str, base: &Url) -> Result<String, ProviderError> {
    // ESM requires a relative path, absolute path, or URL. Reject bare
    // names up front — they would otherwise resolve as relative paths,
    // masking the "no bare-specifier resolution" non-goal with surprising
    // behaviour.
    let relative = specifier.starts_with("./") || specifier.starts_with("../");
    let absolute_path = specifier.starts_with('/');
    let url_like = !relative && !absolute_path && Url::parse(specifier).is_ok();
    if !(relative || absolute_path || url_like) {
        return Err(ProviderError::Other(format!(
            "bare module specifier not supported: {specifier:?} \
             (use a relative path, an absolute path, or a file: URL)"
        )));
    }

    let base = if referrer.is_empty() {
        base.clone()
    } else {
        Url::parse(referrer)
            .map_err(|e| ProviderError::Other(format!("invalid referrer {referrer:?}: {e}")))?
    };
    let resolved = base
        .join(specifier)
        .map_err(|e| ProviderError::Other(format!("cannot resolve {specifier:?}: {e}")))?;

    if resolved.scheme() != "file" {
        return Err(ProviderError::Other(format!(
            "unsupported module scheme {:?}: only file: modules are supported",
            resolved.scheme()
        )));
    }
    Ok(resolved.into())
}

impl ModuleLoader for FsModuleLoader {
    fn resolve(&self, specifier: &str, referrer: &str) -> BoxFuture<Result<String, ProviderError>> {
        Box::pin(std::future::ready(resolve_core(
            specifier, referrer, &self.base,
        )))
    }

    fn resolve_sync(
        &self,
        specifier: &str,
        referrer: &str,
    ) -> Option<Result<String, ProviderError>> {
        Some(resolve_core(specifier, referrer, &self.base))
    }

    fn load(&self, specifier: &str) -> BoxFuture<Result<ModuleSource, ProviderError>> {
        let specifier = specifier.to_string();
        Box::pin(async move {
            let url = Url::parse(&specifier).map_err(|e| {
                ProviderError::Other(format!("invalid module id {specifier:?}: {e}"))
            })?;
            let path: PathBuf = url.to_file_path().map_err(|()| {
                ProviderError::Other(format!("module id is not a file path: {specifier}"))
            })?;
            // `.wasm` is binary and joins the graph through the WebAssembly ESM
            // integration; everything else is read as UTF-8 source.
            let is_wasm = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("wasm"));
            let read = |e: std::io::Error| {
                ProviderError::Other(format!("cannot read {}: {e}", path.display()))
            };
            if is_wasm {
                Ok(ModuleSource::Wasm(
                    tokio::fs::read(&path).await.map_err(read)?,
                ))
            } else {
                Ok(ModuleSource::Text(
                    tokio::fs::read_to_string(&path).await.map_err(read)?,
                ))
            }
        })
    }
}

/// A [`ModuleLoader`] that refuses everything — the default when an embedder
/// grants no module-loading capability, so any `import` fails cleanly rather
/// than reaching the filesystem.
pub struct DenyModuleLoader;

impl ModuleLoader for DenyModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        _referrer: &str,
    ) -> BoxFuture<Result<String, ProviderError>> {
        let specifier = specifier.to_string();
        Box::pin(async move {
            Err(ProviderError::Other(format!(
                "module loading is not permitted (cannot resolve {specifier:?})"
            )))
        })
    }

    fn load(&self, specifier: &str) -> BoxFuture<Result<ModuleSource, ProviderError>> {
        let specifier = specifier.to_string();
        Box::pin(async move {
            Err(ProviderError::Other(format!(
                "module loading is not permitted (cannot load {specifier})"
            )))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A loader rooted at the test process's working directory — absolute on
    /// every platform — with the matching base URL to build expectations from.
    /// A literal `"/app"` used to stand here, which is absolute on Unix and a
    /// drive-less relative path on Windows, so every test using the helper
    /// failed there in the helper rather than in anything it meant to check.
    fn loader() -> (FsModuleLoader, Url) {
        let dir = std::env::current_dir().expect("working directory");
        let base = Url::from_directory_path(&dir).expect("base dir");
        let loader = FsModuleLoader::with_base_dir(&dir).expect("base dir");
        (loader, base)
    }

    #[tokio::test]
    async fn resolves_relative_against_referrer() {
        let (l, base) = loader();
        let referrer = base.join("main.mjs").unwrap().to_string();
        assert_eq!(
            l.resolve("./util.mjs", &referrer).await.unwrap(),
            base.join("util.mjs").unwrap().to_string()
        );
        let referrer = base.join("sub/main.mjs").unwrap().to_string();
        assert_eq!(
            l.resolve("../lib/x.mjs", &referrer).await.unwrap(),
            base.join("lib/x.mjs").unwrap().to_string()
        );
    }

    #[tokio::test]
    async fn resolves_entry_relative_to_base() {
        // Empty referrer → resolve against the loader's base directory.
        let (l, base) = loader();
        assert_eq!(
            l.resolve("./main.mjs", "").await.unwrap(),
            base.join("main.mjs").unwrap().to_string()
        );
    }

    #[tokio::test]
    async fn resolves_absolute_path_and_file_url() {
        let (l, base) = loader();
        let referrer = base.join("main.mjs").unwrap().to_string();
        // An absolute-path reference replaces the base's whole path, on every
        // platform — so this is one fixed URL, not one per drive letter.
        assert_eq!(
            l.resolve("/abs/x.mjs", &referrer).await.unwrap(),
            "file:///abs/x.mjs"
        );
        assert_eq!(
            l.resolve("file:///elsewhere/y.mjs", &referrer)
                .await
                .unwrap(),
            "file:///elsewhere/y.mjs"
        );
    }

    #[tokio::test]
    async fn rejects_bare_specifier() {
        let (l, base) = loader();
        let referrer = base.join("main.mjs").unwrap().to_string();
        let err = l.resolve("lodash", &referrer).await.unwrap_err();
        assert!(format!("{err}").contains("bare module specifier"), "{err}");
    }

    #[tokio::test]
    async fn rejects_non_file_scheme() {
        let (l, base) = loader();
        let referrer = base.join("main.mjs").unwrap().to_string();
        let err = l
            .resolve("https://example.com/x.mjs", &referrer)
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("only file:"), "{err}");
    }

    #[tokio::test]
    async fn loads_a_real_file() {
        let dir = std::env::temp_dir().join(format!("esrt-mod-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.mjs");
        std::fs::write(&path, "export const v = 1;").unwrap();
        let id = Url::from_file_path(&path).unwrap().to_string();

        let source = loader().0.load(&id).await.unwrap();
        assert_eq!(source, ModuleSource::Text("export const v = 1;".into()));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn loads_a_wasm_file_as_bytes() {
        let dir = std::env::temp_dir().join(format!("esrt-mod-wasm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.wasm");
        // `\0asm` + version 1 — an empty but well-formed module.
        let bytes = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        std::fs::write(&path, bytes).unwrap();
        let id = Url::from_file_path(&path).unwrap().to_string();

        // Read as bytes, not decoded as UTF-8 (which these are not).
        let source = loader().0.load(&id).await.unwrap();
        assert_eq!(source, ModuleSource::Wasm(bytes.to_vec()));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn load_reports_a_missing_file() {
        // A well-formed file URL that names nothing: `to_file_path` must
        // succeed so the failure is the read, on every platform. A Unix-only
        // literal like `file:///no/such/module.mjs` is not convertible on
        // Windows, which would fail the test in URL parsing instead of in
        // the read it is about.
        let missing = std::env::temp_dir().join(format!("esrt-mod-missing-{}", std::process::id()));
        let id = Url::from_file_path(missing.join("module.mjs"))
            .unwrap()
            .to_string();
        let err = loader().0.load(&id).await.unwrap_err();
        assert!(format!("{err}").contains("cannot read"), "{err}");
    }

    #[tokio::test]
    async fn deny_loader_refuses() {
        assert!(DenyModuleLoader.resolve("./x.mjs", "").await.is_err());
        assert!(DenyModuleLoader.load("file:///x.mjs").await.is_err());
    }
}
