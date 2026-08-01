//! A module-import allowlist — the policy behind `esrun --allow-imports=<list>`
//! (DECISIONS D38).
//!
//! **Entries are read the way specifiers are.** An entry beginning with `.` or
//! `/` is a **path** and covers its subtree, exactly as `--allow-read` does;
//! anything else is a **package name** (`lodash`, `@scope/pkg`) and covers that
//! package and its subpaths. That is not a new grammar to learn — it is the
//! same split the loader already makes between a relative/absolute specifier
//! and a bare one.
//!
//! **The check runs on the resolved, canonicalized module**, after the sandbox
//! root has been enforced, for the same reason the filesystem lists do: a
//! symlink is a name for a file elsewhere, and judging the specifier would let
//! `./src/link-to-node_modules/evil/index.js` through. Resolving first also
//! makes package matching work under pnpm, whose `node_modules/lodash` is a
//! symlink into a content store — the real path still ends in
//! `…/node_modules/lodash/…`, so the package it belongs to is recoverable from
//! the path itself.

use std::path::{Component, Path};

use es_runtime_common::ErrorCode;
use es_runtime_providers::ProviderError;

use crate::path_allowlist::PathAllowlist;

/// Packages and paths a run may import from. An empty list permits nothing.
#[derive(Clone, Debug, Default)]
pub struct ImportAllowlist {
    /// Package names as written in a bare specifier (`lodash`, `@scope/pkg`).
    packages: Vec<String>,
    /// Path entries, canonicalized — the same matching `--allow-read` uses.
    paths: PathAllowlist,
}

impl ImportAllowlist {
    /// Parses `entries` — package names, or paths (absolute, or relative to
    /// `base`: the directory the user typed them in).
    pub fn parse<I, S>(entries: I, base: &Path) -> Result<ImportAllowlist, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut packages = Vec::new();
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.as_ref();
            if entry.is_empty() {
                return Err("an empty entry is neither a package nor a path".to_string());
            }
            if entry.starts_with('.') || entry.starts_with('/') || entry.starts_with("file:") {
                paths.push(entry.to_string());
            } else {
                packages.push(entry.trim_end_matches('/').to_string());
            }
        }
        Ok(ImportAllowlist {
            packages,
            paths: PathAllowlist::parse(paths, base)?,
        })
    }

    /// Whether the module at `real` (resolved and canonicalized) may be
    /// imported.
    ///
    /// A module inside a `node_modules` tree is judged as the **package** it
    /// belongs to; anything else is judged as a path. So `--allow-imports=lodash`
    /// covers `lodash`'s own internal files — a package whose parts could not
    /// load would be no grant at all — while saying nothing about the packages
    /// *it* imports, which are named in their own right.
    pub(crate) fn permits(&self, real: &Path) -> bool {
        match package_of(real) {
            Some(name) => self.packages.contains(&name),
            None => self.paths.permits(real),
        }
    }

    /// [`permits`](Self::permits), as a provider error naming what was refused.
    pub(crate) fn check(&self, real: &Path) -> Result<(), ProviderError> {
        if self.permits(real) {
            return Ok(());
        }
        let what = match package_of(real) {
            Some(name) => format!("package {name}"),
            None => real.display().to_string(),
        };
        Err(ProviderError::Coded {
            code: ErrorCode::PermissionDenied,
            message: format!("{what} is not an allowed import"),
        })
    }
}

/// The package a resolved module belongs to, if it is inside a `node_modules`
/// tree: the one or two components after the **last** `node_modules`, so a
/// scoped package (`@scope/pkg`) keeps its scope and a nested dependency is
/// named as itself rather than as its parent.
fn package_of(real: &Path) -> Option<String> {
    let components: Vec<&std::ffi::OsStr> = real
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    let last = components
        .iter()
        .rposition(|name| *name == std::ffi::OsStr::new("node_modules"))?;
    let first = components.get(last + 1)?.to_str()?;
    if first.starts_with('@') {
        let second = components.get(last + 2)?.to_str()?;
        return Some(format!("{first}/{second}"));
    }
    Some(first.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("esrt-importallow-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::path::canonicalize(&dir).unwrap()
    }

    #[test]
    fn a_package_entry_covers_that_packages_own_files() {
        let root = temp_dir("package");
        let allow = ImportAllowlist::parse(["lodash"], &root).unwrap();
        assert!(allow.permits(Path::new("/proj/node_modules/lodash/index.js")));
        assert!(allow.permits(Path::new("/proj/node_modules/lodash/fp/map.js")));
        assert!(!allow.permits(Path::new("/proj/node_modules/left-pad/index.js")));
    }

    #[test]
    fn a_nested_dependency_is_named_as_itself() {
        // Allowing a package is not allowing what it imports: each package is
        // named in its own right, whoever pulled it in.
        let root = temp_dir("nested");
        let allow = ImportAllowlist::parse(["lodash"], &root).unwrap();
        assert!(!allow.permits(Path::new(
            "/proj/node_modules/lodash/node_modules/inner/index.js"
        )));
    }

    #[test]
    fn a_scoped_package_keeps_its_scope() {
        let root = temp_dir("scoped");
        let allow = ImportAllowlist::parse(["@acme/ui"], &root).unwrap();
        assert!(allow.permits(Path::new("/proj/node_modules/@acme/ui/index.js")));
        assert!(!allow.permits(Path::new("/proj/node_modules/@acme/other/index.js")));
        // A same-named package outside the scope is a different package.
        assert!(!allow.permits(Path::new("/proj/node_modules/ui/index.js")));
    }

    #[test]
    fn a_pnpm_store_path_still_names_its_package() {
        // pnpm's node_modules/lodash is a symlink into .pnpm; the real path is
        // what the loader canonicalizes to, and it still ends in the package.
        let root = temp_dir("pnpm");
        let allow = ImportAllowlist::parse(["lodash"], &root).unwrap();
        assert!(allow.permits(Path::new(
            "/proj/node_modules/.pnpm/lodash@4.17.21/node_modules/lodash/index.js"
        )));
    }

    #[test]
    fn a_path_entry_covers_project_files() {
        let root = temp_dir("paths");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("vendor")).unwrap();
        let allow = ImportAllowlist::parse(["./src"], &root).unwrap();
        assert!(allow.permits(&root.join("src/app.mjs")));
        assert!(allow.permits(&root.join("src/lib/util.mjs")));
        assert!(!allow.permits(&root.join("vendor/thing.mjs")));
    }

    #[test]
    fn packages_and_paths_can_be_mixed() {
        let root = temp_dir("mixed");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let allow = ImportAllowlist::parse(["./src", "lodash"], &root).unwrap();
        assert!(allow.permits(&root.join("src/app.mjs")));
        assert!(allow.permits(Path::new("/proj/node_modules/lodash/index.js")));
    }

    #[test]
    fn an_empty_list_permits_nothing() {
        assert!(!ImportAllowlist::default().permits(Path::new("/proj/src/app.mjs")));
        assert!(!ImportAllowlist::default().permits(Path::new("/proj/node_modules/x/index.js")));
    }

    #[test]
    fn check_names_a_package_as_a_package() {
        let root = temp_dir("message");
        let allow = ImportAllowlist::parse(["lodash"], &root).unwrap();
        let err = allow
            .check(Path::new("/proj/node_modules/evil/index.js"))
            .unwrap_err();
        assert!(err.to_string().contains("package evil"), "{err}");
        assert_eq!(err.code(), Some(ErrorCode::PermissionDenied));
    }
}
