//! The packages a project links from outside itself (D234).
//!
//! Developing a package against the app that uses it is `"my-lib":
//! "link:../my-lib"` (pnpm, Yarn), `"file:../my-lib"` (npm) or
//! `"workspace:*"` (a workspace member): an install puts a symbolic link in
//! `node_modules` that leads out of the project. The module loader confines
//! every module to the project root (D25), so the link was followed, found to
//! lead outside, and refused — `esdev build` bundled the app and `esdev test`
//! could not load it.
//!
//! A package the project's own `package.json` declares with one of those
//! protocols is a root of its own: what it holds loads, its own `node_modules`
//! included. Nothing else outside the project does. A link a dependency's
//! install made, or one nobody declared, is not the project saying "this is
//! mine too".

use std::path::{Path, PathBuf};

/// The protocols that name a package by its place on disk rather than by a
/// version — what a project writes to develop against a local copy.
const LOCAL_PROTOCOLS: &[&str] = &["link:", "file:", "workspace:", "portal:"];

/// The sections of `package.json` a dependency is declared in.
const SECTIONS: &[&str] = &[
    "dependencies",
    "devDependencies",
    "optionalDependencies",
    "peerDependencies",
];

/// The real directories of the packages `root`'s `package.json` declares with
/// a local protocol, that are installed in its `node_modules` and lie outside
/// it. A missing or unreadable manifest, or a package not installed, is
/// nothing to admit rather than an error: the loader's own refusal says what
/// is wrong when something imports it.
pub fn linked_packages(root: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string(root.join("package.json")) else {
        return Vec::new();
    };
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let real_root = es_runtime_default_providers::path::canonicalize(root)
        .unwrap_or_else(|_| root.to_path_buf());
    let mut linked = Vec::new();
    for section in SECTIONS {
        let Some(deps) = manifest
            .get(*section)
            .and_then(serde_json::Value::as_object)
        else {
            continue;
        };
        for (name, spec) in deps {
            let Some(spec) = spec.as_str() else { continue };
            if !LOCAL_PROTOCOLS
                .iter()
                .any(|protocol| spec.starts_with(protocol))
            {
                continue;
            }
            // `@scope/name` is two components; joined one at a time so neither
            // carries a separator.
            let installed = name
                .split('/')
                .fold(root.join("node_modules"), |dir, part| dir.join(part));
            let Ok(real) = es_runtime_default_providers::path::canonicalize(&installed) else {
                continue;
            };
            if real.is_dir() && !real.starts_with(&real_root) && !linked.contains(&real) {
                linked.push(real);
            }
        }
    }
    linked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_declared_local_package_outside_the_project_is_linked() {
        use std::os::unix::fs::symlink;
        let base = std::env::temp_dir().join(format!("esdev-linked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let app = base.join("app");
        for dir in [
            "lib",
            "scoped",
            "undeclared",
            "app/node_modules/@me",
            "app/vendored",
        ] {
            std::fs::create_dir_all(base.join(dir)).expect("mkdir");
        }
        symlink(base.join("lib"), app.join("node_modules/lib")).expect("link lib");
        symlink(base.join("scoped"), app.join("node_modules/@me/ui")).expect("link scoped");
        symlink(base.join("undeclared"), app.join("node_modules/undeclared")).expect("link");
        // A `file:` dependency inside the project is the project already.
        symlink(app.join("vendored"), app.join("node_modules/vendored")).expect("link");
        std::fs::write(
            app.join("package.json"),
            r#"{
              "dependencies": { "lib": "link:../lib", "vendored": "file:./vendored", "react": "^19" },
              "devDependencies": { "@me/ui": "workspace:*", "undeclared": "^1.0.0", "missing": "link:../missing" }
            }"#,
        )
        .expect("manifest");
        let real =
            |p: &str| es_runtime_default_providers::path::canonicalize(base.join(p)).unwrap();
        let mut found = linked_packages(&app);
        found.sort();
        let mut wanted = vec![real("lib"), real("scoped")];
        wanted.sort();
        assert_eq!(found, wanted);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn no_manifest_links_nothing() {
        assert!(linked_packages(Path::new("/nonexistent/esdev-project")).is_empty());
    }
}
