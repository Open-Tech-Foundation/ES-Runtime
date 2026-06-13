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
//! No root confinement yet: a path may escape via `..`/symlinks. Jailing module
//! loading to a root is a later hardening item (tracked with the FS provider).

use std::path::PathBuf;

use es_runtime_providers::{BoxFuture, ModuleLoader, ProviderError};
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

impl ModuleLoader for FsModuleLoader {
    fn resolve(&self, specifier: &str, referrer: &str) -> BoxFuture<Result<String, ProviderError>> {
        let base = self.base.clone();
        let specifier = specifier.to_string();
        let referrer = referrer.to_string();
        Box::pin(async move {
            // ESM requires a relative path, absolute path, or URL. Reject bare
            // names up front — they would otherwise resolve as relative paths,
            // masking the "no bare-specifier resolution" non-goal with
            // surprising behaviour.
            let relative = specifier.starts_with("./") || specifier.starts_with("../");
            let absolute_path = specifier.starts_with('/');
            let url_like = !relative && !absolute_path && Url::parse(&specifier).is_ok();
            if !(relative || absolute_path || url_like) {
                return Err(ProviderError::Other(format!(
                    "bare module specifier not supported: {specifier:?} \
                     (use a relative path, an absolute path, or a file: URL)"
                )));
            }

            let base = if referrer.is_empty() {
                base
            } else {
                Url::parse(&referrer).map_err(|e| {
                    ProviderError::Other(format!("invalid referrer {referrer:?}: {e}"))
                })?
            };
            let resolved = base
                .join(&specifier)
                .map_err(|e| ProviderError::Other(format!("cannot resolve {specifier:?}: {e}")))?;

            if resolved.scheme() != "file" {
                return Err(ProviderError::Other(format!(
                    "unsupported module scheme {:?}: only file: modules are supported",
                    resolved.scheme()
                )));
            }
            Ok(resolved.into())
        })
    }

    fn load(&self, specifier: &str) -> BoxFuture<Result<String, ProviderError>> {
        let specifier = specifier.to_string();
        Box::pin(async move {
            let url = Url::parse(&specifier).map_err(|e| {
                ProviderError::Other(format!("invalid module id {specifier:?}: {e}"))
            })?;
            let path: PathBuf = url.to_file_path().map_err(|()| {
                ProviderError::Other(format!("module id is not a file path: {specifier}"))
            })?;
            tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| ProviderError::Other(format!("cannot read {}: {e}", path.display())))
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

    fn load(&self, specifier: &str) -> BoxFuture<Result<String, ProviderError>> {
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

    fn loader() -> FsModuleLoader {
        FsModuleLoader::with_base_dir("/app").expect("base dir")
    }

    #[tokio::test]
    async fn resolves_relative_against_referrer() {
        let l = loader();
        assert_eq!(
            l.resolve("./util.mjs", "file:///app/main.mjs")
                .await
                .unwrap(),
            "file:///app/util.mjs"
        );
        assert_eq!(
            l.resolve("../lib/x.mjs", "file:///app/sub/main.mjs")
                .await
                .unwrap(),
            "file:///app/lib/x.mjs"
        );
    }

    #[tokio::test]
    async fn resolves_entry_relative_to_base() {
        // Empty referrer → resolve against the loader's base directory.
        assert_eq!(
            loader().resolve("./main.mjs", "").await.unwrap(),
            "file:///app/main.mjs"
        );
    }

    #[tokio::test]
    async fn resolves_absolute_path_and_file_url() {
        let l = loader();
        assert_eq!(
            l.resolve("/abs/x.mjs", "file:///app/main.mjs")
                .await
                .unwrap(),
            "file:///abs/x.mjs"
        );
        assert_eq!(
            l.resolve("file:///elsewhere/y.mjs", "file:///app/main.mjs")
                .await
                .unwrap(),
            "file:///elsewhere/y.mjs"
        );
    }

    #[tokio::test]
    async fn rejects_bare_specifier() {
        let err = loader()
            .resolve("lodash", "file:///app/main.mjs")
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("bare module specifier"), "{err}");
    }

    #[tokio::test]
    async fn rejects_non_file_scheme() {
        let err = loader()
            .resolve("https://example.com/x.mjs", "file:///app/main.mjs")
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

        let source = loader().load(&id).await.unwrap();
        assert_eq!(source, "export const v = 1;");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn load_reports_a_missing_file() {
        let err = loader()
            .load("file:///no/such/module.mjs")
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("cannot read"), "{err}");
    }

    #[tokio::test]
    async fn deny_loader_refuses() {
        assert!(DenyModuleLoader.resolve("./x.mjs", "").await.is_err());
        assert!(DenyModuleLoader.load("file:///x.mjs").await.is_err());
    }
}
