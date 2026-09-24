//! The resolved state of one `esdev` invocation: `esdev.json` and the command
//! line, combined once (DECISIONS D125).
//!
//! # Why one place
//!
//! A project says things in two places — its `esdev.json` and the flags a
//! command was given — and every command has to act on the two combined. When
//! each command combined them itself, each did it differently, and a setting
//! reached some paths and not others: a flag a project build accepted and
//! ignored, a `jsx` section `esdev test` honoured and `esdev <file>` did not.
//! Nothing failed; the setting just was not there.
//!
//! So the file is read **here and nowhere else**, the flags are applied to it
//! **here and nowhere else**, and what the rest of `esdev` consumes is the
//! result, [`Settings`]. [`config::Project`](crate::config::Project) is the
//! file as written; nothing outside this module reads it.
//!
//! # What is resolved
//!
//! - [`Source`]: what source means in this project — how JSX compiles, the
//!   aliases, the project's plugins. The same for every command, because a
//!   module means one thing whether it is built, tested or run.
//! - The targets, with the build flags applied, and the start and test
//!   sections, which [`crate::start`] and [`crate::test`] consume.
//!
//! # How a flag meets its setting
//!
//! One rule each: it **replaces** the file's value (`--sourcemap`, `--port`),
//! **adds** to it (`--define`, `--conditions`, `--allow-read`, and `--alias`
//! by name, the flag winning), or is **refused** in a mode it cannot mean
//! anything in. The rules are the methods below; the table test in
//! `tests/cli.rs` runs every flag against a project and fails on one that is
//! neither applied nor refused.

use std::path::PathBuf;
use std::sync::Arc;

use es_runtime_cli_common::run::{SourceTransform, SpecifierAlias};

use crate::config::{PluginSpec, Project, Start, Target, TestSettings};
use crate::transform::{JsxSettings, TypeStripper};

/// What source means in a project: the settings every command that compiles or
/// resolves a module applies the same way.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Source {
    /// The project's directory, or the working directory when there is no
    /// `esdev.json`. What plugin paths and aliases were resolved against.
    pub root: PathBuf,
    /// How JSX compiles, which a file's own pragma may still override.
    pub jsx: JsxSettings,
    /// Specifier rewrites: the file's `alias` with the flags' applied, longest
    /// name first, a path already absolute.
    pub alias: Vec<(String, String)>,
    /// The project's top-level `plugins` — every target's, and a test's.
    pub plugins: Vec<PluginSpec>,
    /// The tsconfig whose `paths` and `baseUrl` resolution reads when it
    /// cannot be found by looking: a JavaScript project's `jsconfig.json`, at
    /// the root, when there is no `tsconfig.json` beside it. `None` is the
    /// resolver's own search for the `tsconfig.json` that owns each file.
    pub tsconfig: Option<PathBuf>,
}

/// A root `jsconfig.json` with no `tsconfig.json` beside it: what TypeScript's
/// own tools read for a JavaScript project, and so what its `paths` mean.
fn jsconfig(root: &std::path::Path) -> Option<PathBuf> {
    let jsconfig = root.join("jsconfig.json");
    (jsconfig.is_file() && !root.join("tsconfig.json").is_file()).then_some(jsconfig)
}

/// One `esdev` invocation's settings.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Whether an `esdev.json` was read. Without one every section is empty,
    /// and a command that needs one says so.
    pub has_project: bool,
    pub source: Source,
    /// Every plugin the project loads — its own first, then each target's —
    /// which a target names by index.
    pub plugins: Vec<PluginSpec>,
    /// The targets, in name order, with the build flags applied.
    pub targets: Vec<Target>,
    pub start: Start,
    /// The grant the dev loop's child runs under, as flags.
    pub permissions: Vec<String>,
    /// The file's `test` section. [`crate::test`] resolves it against the
    /// test flags, which are the one command whose flags outnumber the file.
    pub test: TestSettings,
}

impl Settings {
    /// The settings of the project in the working directory — or the one
    /// `--config` named — before any flag.
    ///
    /// A missing `./esdev.json` is an empty project rooted here; a `--config`
    /// naming a file that is not there is an error, never a silent fallback.
    pub fn load(named: Option<&str>) -> Result<Settings, String> {
        Ok(match crate::config::load(named)? {
            Some(project) => Settings::from_project(project),
            None => Settings::empty(
                std::env::current_dir()
                    .map_err(|e| format!("cannot read working directory: {e}"))?,
            ),
        })
    }

    /// A parsed project, before any flag.
    pub fn from_project(project: Project) -> Settings {
        let shared = project.project_plugins().to_vec();
        Settings {
            has_project: true,
            source: Source {
                tsconfig: jsconfig(&project.dir),
                root: project.dir.clone(),
                jsx: project.jsx,
                alias: project.alias,
                plugins: shared,
            },
            plugins: project.plugins,
            targets: project.targets,
            start: project.start,
            permissions: project.permissions,
            test: project.test,
        }
    }

    fn empty(root: PathBuf) -> Settings {
        Settings {
            has_project: false,
            source: Source {
                tsconfig: jsconfig(&root),
                root,
                ..Source::default()
            },
            plugins: Vec::new(),
            targets: Vec::new(),
            start: Start::default(),
            permissions: Vec::new(),
            test: TestSettings::default(),
        }
    }

    /// `--alias`: each replaces the file's alias of the same name, or adds one.
    pub fn with_alias(mut self, flags: &[(String, String)]) -> Settings {
        for (find, to) in flags {
            self.source.alias.retain(|(existing, _)| existing != find);
            self.source.alias.push((find.clone(), to.clone()));
        }
        // Longest first, so `@/ui` wins over `@`: a resolver takes the first
        // match, and neither the file nor the command line is in that order.
        self.source
            .alias
            .sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        self
    }

    /// `esdev start`'s flags: `--port` replaces the file's, and each
    /// `--allow-read` adds to the grant the dev loop's child runs under.
    pub fn with_start(mut self, port: Option<u16>, grants: Vec<String>) -> Settings {
        if let Some(port) = port {
            self.start.port = Some(port);
        }
        self.permissions.extend(grants);
        self
    }

    /// The build flags, applied to every target.
    pub fn with_build(mut self, flags: &TargetFlags) -> Settings {
        for target in &mut self.targets {
            flags.apply(target);
        }
        self
    }

    /// The aliases a build resolves with — the project's, or none for a
    /// library.
    ///
    /// None for a library: a published module keeps the specifier its source
    /// wrote, and the build that consumes it resolves that. Rewriting one here
    /// would ship a package whose imports work under this toolchain only.
    pub fn alias(&self, lib: bool) -> Vec<(String, String)> {
        if lib {
            Vec::new()
        } else {
            self.source.alias.clone()
        }
    }
}

/// The flags that shape a build, and the one way each meets a target.
#[derive(Clone, Debug, Default)]
pub struct TargetFlags {
    /// `--minify`: on for every target, whatever the file says.
    pub minify: bool,
    /// `--sourcemap[=<kind>]`: replaces every target's.
    pub sourcemap: Option<String>,
    /// `--define`s: after each target's own, so a flag wins a name both set.
    pub defines: Vec<(String, String)>,
    /// `--conditions`: added to each target's own.
    pub conditions: Vec<String>,
}

impl TargetFlags {
    pub fn apply(&self, target: &mut Target) {
        target.minify |= self.minify;
        if let Some(kind) = &self.sourcemap {
            target.sourcemap = Some(kind.clone());
        }
        target.define.extend(self.defines.iter().cloned());
        target.conditions.extend(self.conditions.iter().cloned());
    }
}

/// A module run, before the project's settings are applied to it: what the
/// caller decides — what runs, under what grant — as opposed to what the
/// project decides, which is how its source is read.
pub struct Run {
    pub source: es_runtime_cli_common::Source,
    pub args: Vec<String>,
    pub capabilities: es_runtime_common::CapabilitySet,
    pub scopes: es_runtime_cli_common::permissions::Scopes,
    pub options: es_runtime_cli_common::args::RunOptions,
    /// The transform, with anything the caller adds — a test's setup prelude.
    /// How it compiles JSX is the project's, and is set from [`Source`].
    pub stripper: TypeStripper,
    pub extensions: Vec<Box<dyn es_runtime_cli_common::HostExtension>>,
    pub observer: Option<es_runtime_cli_common::SharedObserver>,
}

impl Source {
    /// How a module `esdev` runs unbundled finds what it imports: the
    /// project's aliases, and CommonJS packages converted as they load.
    pub fn resolution(&self) -> Resolution {
        Resolution {
            alias: Arc::new(crate::alias::Aliases::new(
                self.alias.clone(),
                self.tsconfig.clone(),
            )),
            converter: crate::commonjs::converter(),
        }
    }

    /// The `cli_common::Config` for a module run unbundled in this project —
    /// the one way `esdev` assembles one, so a setting the project states
    /// reaches every run or none.
    pub async fn run_config(&self, run: Run) -> Result<es_runtime_cli_common::Config, String> {
        let stripper = run.stripper.compiling_jsx(self.jsx.clone());
        let transform: Arc<dyn SourceTransform> =
            crate::plugins::transform(self, Arc::new(stripper)).await?;
        let resolution = self.resolution();
        Ok(es_runtime_cli_common::Config {
            source: run.source,
            args: run.args,
            capabilities: run.capabilities,
            scopes: run.scopes,
            options: run.options,
            transform: Some(transform),
            // The file being edited resolves the way the build that bundles
            // it does.
            bundler_style_resolution: true,
            package_converter: Some(resolution.converter),
            specifier_alias: Some(resolution.alias),
            extensions: run.extensions,
            observer: run.observer,
            inspector: None,
        })
    }
}

/// The loader hooks a project's unbundled runs share.
pub struct Resolution {
    pub alias: Arc<dyn SpecifierAlias>,
    pub converter: Arc<dyn es_runtime_cli_common::run::PackageConverter>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(name: &str, lib: bool) -> Target {
        let mut project = crate::config::tests_support::read(&format!(
            r#"{{ "targets": {{ "{name}": {{ "entry": "src/a.ts", "{key}": "dist", "lib": {lib},
                  "define": {{ "A": "1" }}, "conditions": ["c"] }} }} }}"#,
            key = "outdir",
        ))
        .expect("parsed");
        project.targets.remove(0)
    }

    /// A flag's alias replaces the file's of the same name and keeps the
    /// rest, longest name first.
    #[test]
    fn a_flag_alias_replaces_the_files_by_name() {
        let mut settings = Settings::empty(PathBuf::from("/p"));
        settings.source.alias = vec![
            ("@".to_string(), "/p/src".to_string()),
            ("~".to_string(), "/p/lib".to_string()),
        ];
        let settings = settings.with_alias(&[
            ("@".to_string(), "/p/other".to_string()),
            ("@/ui".to_string(), "/p/ui".to_string()),
        ]);
        assert_eq!(
            settings.source.alias,
            [
                ("@/ui".to_string(), "/p/ui".to_string()),
                ("@".to_string(), "/p/other".to_string()),
                ("~".to_string(), "/p/lib".to_string()),
            ]
        );
    }

    /// Every build flag has one rule against a target's setting.
    #[test]
    fn build_flags_replace_or_add_to_a_targets_settings() {
        let mut target = target("web", false);
        TargetFlags {
            minify: true,
            sourcemap: Some("inline".to_string()),
            defines: vec![("B".to_string(), "2".to_string())],
            conditions: vec!["d".to_string()],
        }
        .apply(&mut target);
        assert!(target.minify);
        assert_eq!(target.sourcemap.as_deref(), Some("inline"));
        assert_eq!(
            target.define,
            [
                // The file's value is JSON, as a `define` replacement is.
                ("A".to_string(), "\"1\"".to_string()),
                ("B".to_string(), "2".to_string())
            ]
        );
        assert_eq!(target.conditions, ["c", "d"]);
    }

    /// `--port` replaces the file's; `--allow-read` adds to its grant.
    #[test]
    fn start_flags_replace_the_port_and_add_to_the_grant() {
        let mut settings = Settings::empty(PathBuf::from("/p"));
        settings.start.port = Some(3000);
        settings.permissions = vec!["--allow-net".to_string()];
        let settings = settings.with_start(Some(4000), vec!["--allow-read=./data".to_string()]);
        assert_eq!(settings.start.port, Some(4000));
        assert_eq!(settings.permissions, ["--allow-net", "--allow-read=./data"]);
        let kept = Settings::empty(PathBuf::from("/p")).with_start(None, Vec::new());
        assert_eq!(kept.start.port, None);
    }

    /// A library target is built with no alias, whatever the project says.
    #[test]
    fn a_library_target_takes_no_alias() {
        let mut settings = Settings::empty(PathBuf::from("/p"));
        settings.source.alias = vec![("@".to_string(), "/p/src".to_string())];
        assert!(settings.alias(target("lib", true).lib).is_empty());
        assert_eq!(settings.alias(target("app", false).lib).len(), 1);
    }
}
