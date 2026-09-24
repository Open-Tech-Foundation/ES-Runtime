//! A project's import aliases, for a module `esdev` runs unbundled.
//!
//! Two places name them, and `esdev build` has always honoured both: the
//! project's own `alias` in `esdev.json`, and `compilerOptions.paths` (with
//! `baseUrl`) in `tsconfig.json`. A module run unbundled — a test file, `esdev
//! app.ts` — went to the runtime's resolver instead, which knows neither, so
//! `import { db } from "@/db"` built and failed under `esdev test`.
//!
//! This applies them the way the build's resolver does, in the same order:
//!
//! 1. **`esdev.json` `alias`**: the longest name that is the whole specifier or
//!    its start up to a `/`, replaced by a path or by another package's name.
//! 2. **`tsconfig.json` `paths`, then `baseUrl`**: from the tsconfig that owns
//!    the importing file, through the resolver `esdev build` uses — so
//!    `extends`, project references and the `*` patterns mean what they mean
//!    to the build. A mapping is taken only when it names a file, as
//!    TypeScript and the build take it; otherwise the specifier is resolved
//!    as written.
//!
//! A file inside `node_modules` is not the project's, so the project's
//! `tsconfig.json` does not apply to what it imports. `esdev.json`'s `alias`
//! does, as it does in a build: `"react": "preact/compat"` is for the
//! dependencies too.

use std::path::{Path, PathBuf};

use es_runtime_cli_common::run::SpecifierAlias;

/// The aliases a module run unbundled resolves through — from
/// [`crate::settings::Source`], which is where the file's and the flags' meet.
pub struct Aliases {
    /// `esdev.json`'s, with the flags', longest name first, a path already
    /// absolute.
    project: Vec<(String, String)>,
    /// Finds the tsconfig that owns a file, and reads it as the build does.
    resolver: oxc_resolver::Resolver,
}

impl Aliases {
    /// `tsconfig` names the one file to read, when looking would find none
    /// ([`crate::settings::Source::tsconfig`]); `None` finds the tsconfig that
    /// owns each importing file, as the build does.
    pub fn new(project: Vec<(String, String)>, tsconfig: Option<PathBuf>) -> Aliases {
        let discovery = match tsconfig {
            Some(config_file) => {
                oxc_resolver::TsconfigDiscovery::Manual(oxc_resolver::TsconfigOptions {
                    config_file,
                    references: oxc_resolver::TsconfigReferences::Auto,
                })
            }
            None => oxc_resolver::TsconfigDiscovery::Auto,
        };
        Aliases {
            project,
            resolver: oxc_resolver::Resolver::new(oxc_resolver::ResolveOptions {
                tsconfig: Some(discovery),
                ..oxc_resolver::ResolveOptions::default()
            }),
        }
    }

    /// `esdev.json`'s alias for `specifier`, if one names it.
    fn project_alias(&self, specifier: &str) -> Option<String> {
        self.project.iter().find_map(|(find, to)| {
            let rest = specifier.strip_prefix(find.as_str())?;
            if !(rest.is_empty() || rest.starts_with('/')) {
                return None;
            }
            let replaced = format!("{to}{rest}");
            if Path::new(to).is_absolute() {
                // A path: the file it names, found as the build finds it —
                // otherwise the path as written, so a miss is reported
                // against the file the alias pointed at.
                let path = PathBuf::from(&replaced);
                Some(file_url(&probe(&path).unwrap_or(path)))
            } else {
                Some(replaced)
            }
        })
    }

    /// `tsconfig.json`'s mapping for `specifier`, written in `importer`.
    fn tsconfig_alias(&self, specifier: &str, importer: &Path) -> Option<String> {
        if importer
            .components()
            .any(|part| part.as_os_str() == "node_modules")
        {
            return None;
        }
        let tsconfig = self.resolver.find_tsconfig(importer).ok()??;
        tsconfig
            .resolve_path_alias_or_base_url(specifier)
            .iter()
            .find_map(|candidate| probe(candidate))
            .map(|file| file_url(&file))
    }
}

impl SpecifierAlias for Aliases {
    fn alias(&self, specifier: &str, referrer: &str) -> Option<String> {
        if is_path(specifier) {
            return None;
        }
        // Any name the project wrote, `#private` ones included, as the build
        // applies it.
        if let Some(aliased) = self.project_alias(specifier) {
            return Some(aliased);
        }
        if specifier.starts_with('#') || specifier.contains(':') {
            return None;
        }
        self.tsconfig_alias(specifier, &importer(referrer))
    }
}

/// A path or a URL, which no alias names.
fn is_path(specifier: &str) -> bool {
    specifier.is_empty()
        || specifier.starts_with('.')
        || specifier.starts_with('/')
        || specifier.starts_with('\\')
        || url::Url::parse(specifier).is_ok_and(|url| url.scheme().len() > 1)
}

/// The file an import was written in. The entry's own imports have no referrer
/// yet, and resolve from the working directory, as the runtime resolves them.
fn importer(referrer: &str) -> PathBuf {
    url::Url::parse(referrer)
        .ok()
        .filter(|url| url.scheme() == "file")
        .and_then(|url| url.to_file_path().ok())
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|dir| dir.join("__esdev_entry__"))
        })
        .unwrap_or_default()
}

/// The file `path` names, the way `esdev`'s resolver finds one: as written,
/// with a source extension, as a directory's index, or `x.js` meaning `x.ts`.
fn probe(path: &Path) -> Option<PathBuf> {
    const SOURCE: [&str; 6] = ["ts", "tsx", "mts", "js", "jsx", "mjs"];
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    let with = |extension: &str| {
        let mut name = path.as_os_str().to_owned();
        name.push(format!(".{extension}"));
        PathBuf::from(name)
    };
    let written_as_js = matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("js" | "mjs" | "jsx")
    );
    let typescript = written_as_js
        .then(|| ["ts", "tsx", "mts"].map(|ext| path.with_extension(ext)))
        .into_iter()
        .flatten();
    typescript
        .chain(SOURCE.iter().map(|ext| with(ext)))
        .chain(SOURCE.iter().map(|ext| path.join(format!("index.{ext}"))))
        .find(|candidate| candidate.is_file())
}

fn file_url(path: &Path) -> String {
    url::Url::from_file_path(path)
        .map(|url| url.to_string())
        .unwrap_or_else(|()| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "esdev-alias-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        for (name, text) in files {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dunce::canonicalize(&dir).unwrap()
    }

    fn url_of(path: &Path) -> String {
        url::Url::from_file_path(path).unwrap().to_string()
    }

    /// The longest name wins, and a name matches whole or up to a `/` — `@`
    /// is not a prefix of `@scope/pkg`.
    #[test]
    fn a_project_alias_is_a_whole_name_or_a_path_prefix() {
        let dir = project(&[("src/ui/button.ts", ""), ("src/lib/index.ts", "")]);
        let aliases = Aliases::new(
            vec![
                ("@/ui".to_string(), dir.join("src/ui").display().to_string()),
                ("@".to_string(), dir.join("src").display().to_string()),
                ("react".to_string(), "preact/compat".to_string()),
            ],
            None,
        );
        let from = url_of(&dir.join("src/app.ts"));
        assert_eq!(
            aliases.alias("@/ui/button", &from),
            Some(url_of(&dir.join("src/ui/button.ts")))
        );
        assert_eq!(
            aliases.alias("@/lib", &from),
            Some(url_of(&dir.join("src/lib/index.ts")))
        );
        assert_eq!(aliases.alias("@scope/pkg", &from), None);
        assert_eq!(
            aliases.alias("react/jsx-runtime", &from).as_deref(),
            Some("preact/compat/jsx-runtime")
        );
        assert_eq!(aliases.alias("reactive", &from), None);
        assert_eq!(aliases.alias("./react", &from), None);
        assert_eq!(aliases.alias("node:fs", &from), None);
    }

    /// A `#` name the project aliased is the project's to say, as in a build.
    #[test]
    fn a_project_alias_may_be_a_hash_name() {
        let dir = project(&[("src/config.ts", "")]);
        let aliases = Aliases::new(
            vec![(
                "#config".to_string(),
                dir.join("src/config.ts").display().to_string(),
            )],
            None,
        );
        let from = url_of(&dir.join("src/app.ts"));
        assert_eq!(
            aliases.alias("#config", &from),
            Some(url_of(&dir.join("src/config.ts")))
        );
        assert_eq!(aliases.alias("#other", &from), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `paths` from the tsconfig that owns the file, through `extends`, taken
    /// only when it names a file.
    #[test]
    fn tsconfig_paths_map_to_files_through_extends() {
        let dir = project(&[
            (
                "tsconfig.base.json",
                r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["src/*"], "~db": ["src/db/index.ts"] } } }"#,
            ),
            ("tsconfig.json", r#"{ "extends": "./tsconfig.base.json" }"#),
            ("src/lib/two.ts", ""),
            ("src/db/index.ts", ""),
            ("src/app.ts", ""),
        ]);
        let aliases = Aliases::new(Vec::new(), None);
        let from = url_of(&dir.join("src/app.ts"));
        assert_eq!(
            aliases.alias("@/lib/two", &from),
            Some(url_of(&dir.join("src/lib/two.ts")))
        );
        assert_eq!(
            aliases.alias("@/lib/two.js", &from),
            Some(url_of(&dir.join("src/lib/two.ts")))
        );
        assert_eq!(
            aliases.alias("~db", &from),
            Some(url_of(&dir.join("src/db/index.ts")))
        );
        // Nothing there: resolved as written, which is a package.
        assert_eq!(aliases.alias("@/missing", &from), None);
        assert_eq!(aliases.alias("react", &from), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A dependency's imports are its own: the project's tsconfig does not
    /// rewrite them.
    #[test]
    fn a_dependency_is_not_under_the_projects_tsconfig() {
        let dir = project(&[
            (
                "tsconfig.json",
                r#"{ "compilerOptions": { "paths": { "@/*": ["./src/*"] } } }"#,
            ),
            ("src/x.ts", ""),
            ("node_modules/dep/index.js", ""),
        ]);
        let aliases = Aliases::new(Vec::new(), None);
        let from = url_of(&dir.join("node_modules/dep/index.js"));
        assert_eq!(aliases.alias("@/x", &from), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
