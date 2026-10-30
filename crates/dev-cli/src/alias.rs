//! A project's import aliases: in a bundle, and for a module `esdev` runs
//! unbundled.
//!
//! Two places name them: the project's own `alias` in `esdev.json`, and
//! `compilerOptions.paths` (with `baseUrl`) in `tsconfig.json`. Every importer
//! takes them the same way, whatever its extension and whichever command
//! reads it (D196):
//!
//! 1. **`esdev.json` `alias`**: the longest name that is the whole specifier or
//!    its start up to a `/`, replaced by a path or by another package's name.
//!    In a bundle that is [`Rewrite`], ahead of every other resolver; the
//!    bundler's own applies `paths` before an alias, so it is not given one.
//! 2. **`tsconfig.json` `paths`, then `baseUrl`**: from the `tsconfig.json`
//!    nearest the importing file, or the reference of it that covers the file,
//!    through the resolver `esdev build` uses — so `extends`, project
//!    references and the `*` patterns mean what they mean to the build. A
//!    mapping is taken only when it names a file, as TypeScript and the build
//!    take it; otherwise the specifier is resolved as written. That resolver
//!    is the vendored oxc_resolver, whose discovery is esdev's rule rather than
//!    "the tsconfig that would compile this file" (`crates/vendor/oxc_resolver`).
//!
//! A file inside `node_modules` is not the project's, so the project's
//! `tsconfig.json` does not apply to what it imports. `esdev.json`'s `alias`
//! does, as it does in a build: `"react": "preact/compat"` is for the
//! dependencies too.

use std::path::{Path, PathBuf};

use anyhow::anyhow;
use es_runtime_cli_common::run::SpecifierAlias;
use rolldown::plugin::{
    HookResolveIdArgs, HookResolveIdOutput, HookResolveIdReturn, HookUsage, Plugin, PluginContext,
    PluginContextResolveOptions, PluginHookMeta, PluginOrder,
};

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
            let rest = named(find, specifier)?;
            if Path::new(to).is_absolute() {
                // A path: the file it names, found as the build finds it —
                // otherwise the path as written, so a miss is reported
                // against the file the alias pointed at.
                let path = join(Path::new(to), rest);
                Some(file_url(&probe(&path).unwrap_or(path)))
            } else {
                Some(format!("{to}{rest}"))
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

/// What follows `find` in `specifier`, when `find` names it: the whole
/// specifier, or its start up to a `/` — `@` is not a prefix of `@scope/pkg`.
fn named<'a>(find: &str, specifier: &'a str) -> Option<&'a str> {
    let rest = specifier.strip_prefix(find)?;
    (rest.is_empty() || rest.starts_with('/')).then_some(rest)
}

/// A bundle's aliases — `esdev.json`'s, the flags', or those a `build()` call
/// names — applied before anything else resolves a specifier, as they are to
/// a module run unbundled.
///
/// The first name that matches is taken, so the list is longest first. Its
/// replacements are tried in order and the first that resolves wins; the
/// rewritten specifier goes through every other plugin and the bundler's
/// resolver, `paths` included, as one written that way would. One that names
/// nothing is an error rather than a fall back to the specifier as written,
/// which would quietly bundle a different copy of the module.
#[derive(Debug)]
pub struct Rewrite {
    alias: Vec<(String, Vec<String>)>,
}

impl Rewrite {
    pub fn new(alias: Vec<(String, Vec<String>)>) -> Rewrite {
        Rewrite { alias }
    }
}

impl Plugin for Rewrite {
    fn name(&self) -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("esdev:alias")
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::ResolveId
    }

    fn resolve_id_meta(&self) -> Option<PluginHookMeta> {
        Some(PluginHookMeta {
            order: Some(PluginOrder::Pre),
        })
    }

    async fn resolve_id(
        &self,
        ctx: &PluginContext,
        args: &HookResolveIdArgs<'_>,
    ) -> HookResolveIdReturn {
        if is_path(args.specifier) {
            return Ok(None);
        }
        let Some((find, to, rest)) = self
            .alias
            .iter()
            .find_map(|(find, to)| named(find, args.specifier).map(|rest| (find, to, rest)))
        else {
            return Ok(None);
        };
        for target in to {
            let options = PluginContextResolveOptions {
                import_kind: args.kind,
                is_entry: args.is_entry,
                skip_self: true,
                ..PluginContextResolveOptions::default()
            };
            let rewritten = format!("{target}{rest}");
            if let Ok(resolved) = ctx
                .resolve(&rewritten, args.importer, Some(options))
                .await?
            {
                return Ok(Some(HookResolveIdOutput::from_resolved_id(resolved)));
            }
        }
        Err(anyhow!(
            "cannot resolve {:?}: the alias {find:?} points it at {}, which names no module",
            args.specifier,
            to.iter()
                .map(|target| format!("{:?}", format!("{target}{rest}")))
                .collect::<Vec<_>>()
                .join(" or ")
        ))
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
        .and_then(|url| es_runtime_default_providers::path::file_path(&url))
        // A directory URL resolves from inside it: the tsconfig that owns a
        // file there is the one that owns the directory.
        .map(|path| {
            if referrer.ends_with('/') {
                path.join("__esdev_entry__")
            } else {
                path
            }
        })
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

/// `base` with the `/`-separated `relative` appended one segment at a time.
/// A Windows verbatim path (`\\?\C:\…`, which `canonicalize` returns) takes
/// every character literally — `/` is no separator there and `..` no parent —
/// so pasting `lib/shared` onto one names a file that cannot exist.
pub(crate) fn join(base: &Path, relative: &str) -> PathBuf {
    let mut path = base.to_path_buf();
    let separator = |c: char| c == '/' || (cfg!(windows) && c == '\\');
    for segment in relative.split(separator) {
        match segment {
            "" | "." => {}
            ".." => {
                path.pop();
            }
            _ => path.push(segment),
        }
    }
    path
}

fn file_url(path: &Path) -> String {
    es_runtime_default_providers::path::file_url(path)
        .map(|url| url.to_string())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
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
        es_runtime_default_providers::path::canonicalize(&dir).unwrap()
    }

    fn url_of(path: &Path) -> String {
        es_runtime_default_providers::path::file_url(path)
            .unwrap()
            .to_string()
    }

    /// The longest name wins, and a name matches whole or up to a `/` — `@`
    /// is not a prefix of `@scope/pkg`.
    #[test]
    fn a_path_is_joined_a_segment_at_a_time() {
        let base = Path::new("/project/src");
        assert_eq!(join(base, "/lib/shared"), base.join("lib").join("shared"));
        assert_eq!(join(base, "./lib/./a"), base.join("lib").join("a"));
        assert_eq!(join(base, "../shared"), Path::new("/project/shared"));
        assert_eq!(join(base, ""), base);
    }

    /// `canonicalize` hands back `\\?\C:\…` on Windows, where `/` is a
    /// character of the name: the segments must be pushed, not pasted.
    #[cfg(windows)]
    #[test]
    fn a_verbatim_windows_path_is_joined_with_its_own_separator() {
        let base = Path::new(r"\\?\C:\project\lib");
        assert_eq!(
            join(base, "/shared"),
            Path::new(r"\\?\C:\project\lib\shared")
        );
        assert_eq!(join(base, "../x/y"), Path::new(r"\\?\C:\project\x\y"));
    }

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

    /// `paths` from the file's tsconfig, through `extends`, taken only when it
    /// names a file.
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

    /// The nearest tsconfig is the file's, whatever its extension (D196): a
    /// `.mdx` page and a `.jsx` file without `allowJs` take the project's
    /// `paths`, never a parent directory's. Upstream's discovery gave the first
    /// none and the second the parent's.
    #[test]
    fn the_nearest_tsconfig_applies_to_every_importer() {
        let dir = project(&[
            (
                "tsconfig.json",
                r#"{ "compilerOptions": { "allowJs": true, "paths": { "pkg": ["./parent/pkg.js"] } } }"#,
            ),
            ("parent/pkg.js", ""),
            (
                "app/tsconfig.json",
                r#"{ "compilerOptions": { "paths": { "pkg": ["./own/pkg.js"] } } }"#,
            ),
            ("app/own/pkg.js", ""),
        ]);
        let aliases = Aliases::new(Vec::new(), None);
        let own = Some(url_of(&dir.join("app/own/pkg.js")));
        for importer in ["page.mdx", "page.jsx", "page.js", "page.vue", "page.ts"] {
            let from = url_of(&dir.join("app/src").join(importer));
            assert_eq!(aliases.alias("pkg", &from), own, "{importer}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The nearest tsconfig applies even where its `include` does not reach,
    /// rather than handing the file to an ancestor's.
    #[test]
    fn the_nearest_tsconfig_applies_outside_its_include() {
        let dir = project(&[
            (
                "tsconfig.json",
                r#"{ "compilerOptions": { "paths": { "pkg": ["./parent/pkg.ts"] } } }"#,
            ),
            ("parent/pkg.ts", ""),
            (
                "app/tsconfig.json",
                r#"{ "include": ["src"], "compilerOptions": { "paths": { "pkg": ["./own/pkg.ts"] } } }"#,
            ),
            ("app/own/pkg.ts", ""),
        ]);
        let aliases = Aliases::new(Vec::new(), None);
        let from = url_of(&dir.join("app/scripts/seed.ts"));
        assert_eq!(
            aliases.alias("pkg", &from),
            Some(url_of(&dir.join("app/own/pkg.ts")))
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A solution config hands a file to the reference that covers it, by
    /// path, whatever the extension: the Vite scaffold's shape, with a page
    /// a plugin compiles.
    #[test]
    fn a_reference_covers_a_page_of_any_extension() {
        let dir = project(&[
            (
                "tsconfig.json",
                r#"{ "files": [], "references": [{ "path": "./tsconfig.app.json" }, { "path": "./tsconfig.node.json" }] }"#,
            ),
            (
                "tsconfig.app.json",
                r#"{ "include": ["src"], "compilerOptions": { "paths": { "@/*": ["./src/*"] } } }"#,
            ),
            (
                "tsconfig.node.json",
                r#"{ "include": ["vite.config.ts"], "compilerOptions": { "paths": { "@/*": ["./node/*"] } } }"#,
            ),
            ("src/ui/card.jsx", ""),
        ]);
        let aliases = Aliases::new(Vec::new(), None);
        let from = url_of(&dir.join("src/pages/post.mdx"));
        assert_eq!(
            aliases.alias("@/ui/card", &from),
            Some(url_of(&dir.join("src/ui/card.jsx")))
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_name_matches_whole_or_up_to_a_slash() {
        assert_eq!(named("pkg", "pkg"), Some(""));
        assert_eq!(named("pkg", "pkg/sub"), Some("/sub"));
        assert_eq!(named("pkg", "pkgs"), None);
        assert_eq!(named("@", "@scope/pkg"), None);
        assert_eq!(named("@", "@/ui"), Some("/ui"));
    }
}
