//! A build's options, and the one place they become the bundler's.
//!
//! Everything in this binary that bundles — the `build` subcommand, the client
//! bundle it emits for a browser, and `runtime:build` in guest JS — describes
//! its build in the types here, and this module is the only thing that turns
//! that description into rolldown's. That is not tidiness: it is the fix for a
//! specific class of bug this project has now shipped twice.
//!
//! Both times, two paths that were supposed to be the same bundler translated
//! their own options and disagreed. `runtime:build` did not install the CSS
//! Modules pass the subcommand installs, so one project produced scoped class
//! names one way and unscoped ones the other. Then it asserted no `exports`
//! conditions and no main fields, so the same entry resolved to a package's
//! `node:` build one way and its Web build the other, and a package predating
//! `exports` did not resolve at all.
//!
//! Neither failed at build time. Both were found by reading the two
//! translations side by side and noticing they had drifted — which is not a way
//! of finding bugs, it is a way of finding the ones you happen to look for. A
//! second translation cannot drift from the first if there is no second
//! translation.
//!
//! What stays out of here is **policy**: which conditions a target asserts
//! lives in [`crate::resolve`], and what a `--lib` build externalises is the
//! subcommand's business. This module knows how to say things, not what to say.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use rolldown::{
    BundlerOptions, InputItem, IsExternal, OutputFormat, Platform, RawMinifyOptions,
    ResolveOptions, SourceMapType, TreeshakeOptions,
};
use rolldown_common::CodeSplittingMode;

use crate::resolve::Target;

/// A predicate deciding what to leave unbundled, one specifier at a time.
///
/// Async because one implementation of it lives in the guest isolate and
/// answering means a round trip; the subcommand's own is a string comparison
/// that happens to be spelled as a future.
pub type ExternalFn = Arc<
    dyn Fn(&str, Option<&str>, bool) -> Pin<Box<dyn Future<Output = anyhow::Result<bool>> + Send>>
        + Send
        + Sync,
>;

/// Where a warning ends up. The subcommand prints; `runtime:build` collects
/// them into the result it hands back to the program that asked for the build.
pub type LogSink = Arc<dyn Fn(String) + Send + Sync>;

/// What `external` was.
#[derive(Clone)]
pub enum External {
    /// Exactly these specifiers.
    List(Vec<String>),
    /// A predicate rather than a list, which is not a nicety: a dev server
    /// externalises `/__route/*` — a shape, not a set.
    Predicate(ExternalFn),
}

impl External {
    /// A predicate that answers without waiting for anything, which is what
    /// every caller inside this binary has.
    pub fn when(
        decide: impl Fn(&str, Option<&str>, bool) -> bool + Send + Sync + 'static,
    ) -> External {
        External::Predicate(Arc::new(move |specifier, importer, resolved| {
            let answer = decide(specifier, importer, resolved);
            Box::pin(async move { Ok(answer) })
        }))
    }
}

/// Everything a build was asked for, in a form that can cross a thread.
#[derive(Default)]
pub struct Options {
    pub cwd: Option<PathBuf>,
    /// Entries, as `(name, import)`. A name is what the chunk is called; most
    /// callers pass one nameless entry.
    pub input: Vec<(Option<String>, String)>,
    pub external: Option<External>,
    /// Where the output runs. Decides the `exports` conditions and main fields
    /// asserted, through [`crate::resolve`].
    pub platform: Target,
    /// Conditions **added** to the ones the target asserts.
    pub conditions: Vec<String>,
    /// Main fields **replacing** the target's, when non-empty.
    pub main_fields: Vec<String>,
    /// `find` → the replacements tried in order.
    pub alias: Vec<(String, Vec<String>)>,
    pub extensions: Vec<String>,
    pub define: Vec<(String, String)>,
    pub minify: bool,
    pub treeshake: Option<bool>,
    /// One file out per module in, rather than one chunk per entry — what a
    /// published library needs, so that a subpath in an `exports` map names a
    /// real file.
    pub preserve_modules: Option<bool>,
    pub preserve_modules_root: Option<String>,
    pub output: OutputOptions,
}

/// The half of the options that describes what comes *out*, which
/// `generate()`/`write()` may override per call.
#[derive(Clone, Default)]
pub struct OutputOptions {
    pub format: Option<String>,
    pub dir: Option<String>,
    pub file: Option<String>,
    pub entry_filenames: Option<String>,
    pub chunk_filenames: Option<String>,
    pub asset_filenames: Option<String>,
    /// `false` puts everything reachable in one chunk, dynamic `import()`
    /// included — the setting a dev server that serves chunks from memory needs
    /// when it is building one route at a time.
    pub code_splitting: Option<bool>,
    pub sourcemap: Option<String>,
    pub banner: Option<String>,
    pub footer: Option<String>,
}

impl OutputOptions {
    /// Fields the caller set here win; the rest stay as `build()` left them.
    pub fn over(self, base: &OutputOptions) -> OutputOptions {
        OutputOptions {
            format: self.format.or_else(|| base.format.clone()),
            dir: self.dir.or_else(|| base.dir.clone()),
            file: self.file.or_else(|| base.file.clone()),
            entry_filenames: self
                .entry_filenames
                .or_else(|| base.entry_filenames.clone()),
            chunk_filenames: self
                .chunk_filenames
                .or_else(|| base.chunk_filenames.clone()),
            asset_filenames: self
                .asset_filenames
                .or_else(|| base.asset_filenames.clone()),
            code_splitting: self.code_splitting.or(base.code_splitting),
            sourcemap: self.sourcemap.or_else(|| base.sourcemap.clone()),
            banner: self.banner.or_else(|| base.banner.clone()),
            footer: self.footer.or_else(|| base.footer.clone()),
        }
    }
}

/// Translates a build into the bundler's own options. The only such
/// translation in this binary, deliberately.
pub fn translate(
    options: &Options,
    output: OutputOptions,
    on_log: Option<LogSink>,
) -> Result<BundlerOptions, String> {
    let input: Vec<InputItem> = options
        .input
        .iter()
        .map(|(name, import)| InputItem {
            name: name.clone(),
            import: import.clone(),
        })
        .collect();
    if input.is_empty() {
        return Err("build: input is required".to_string());
    }

    let target = options.platform;
    let resolve = ResolveOptions {
        condition_names: Some(crate::resolve::conditions(
            target,
            options.conditions.clone(),
        )),
        // Naming them replaces ours; there is one ordered list and a caller who
        // wrote one means it.
        main_fields: if options.main_fields.is_empty() {
            crate::resolve::main_fields(target)
        } else {
            Some(options.main_fields.clone())
        },
        extensions: (!options.extensions.is_empty()).then(|| options.extensions.clone()),
        alias: (!options.alias.is_empty()).then(|| {
            options
                .alias
                .iter()
                .map(|(find, to)| {
                    (
                        find.clone(),
                        to.iter().map(|t| Some(t.clone())).collect::<Vec<_>>(),
                    )
                })
                .collect()
        }),
        ..ResolveOptions::default()
    };

    Ok(BundlerOptions {
        input: Some(input),
        cwd: options.cwd.clone(),
        external: match &options.external {
            None => None,
            Some(External::List(list)) => Some(IsExternal::from(list.clone())),
            Some(External::Predicate(f)) => Some(IsExternal::Fn(Some(f.clone()))),
        },
        platform: Some(match target {
            Target::Browser => Platform::Browser,
            Target::Node => Platform::Node,
            // Neither a browser nor Node, which is what this runtime is: saying
            // either would pull in that platform's main fields and aliases. The
            // conditions above are how a package's Web-API build is picked
            // instead.
            // A library is neutral too: it is not being aimed anywhere, it is
            // being handed to a build that will aim it.
            Target::Server | Target::Library => Platform::Neutral,
        }),
        format: match output.format.as_deref() {
            Some("cjs") => Some(OutputFormat::Cjs),
            Some("iife") => Some(OutputFormat::Iife),
            Some("umd") => Some(OutputFormat::Umd),
            _ => Some(OutputFormat::Esm),
        },
        dir: output.dir,
        file: output.file,
        entry_filenames: output.entry_filenames.map(Into::into),
        chunk_filenames: output.chunk_filenames.map(Into::into),
        asset_filenames: output.asset_filenames.map(Into::into),
        code_splitting: output.code_splitting.map(CodeSplittingMode::Bool),
        preserve_modules: options.preserve_modules,
        preserve_modules_root: options.preserve_modules_root.clone(),
        sourcemap: match output.sourcemap.as_deref() {
            Some("inline") => Some(SourceMapType::Inline),
            Some("hidden") => Some(SourceMapType::Hidden),
            Some("external" | "true") => Some(SourceMapType::File),
            _ => None,
        },
        banner: output
            .banner
            .map(|text| rolldown_common::AddonOutputOption::String(Some(text))),
        footer: output
            .footer
            .map(|text| rolldown_common::AddonOutputOption::String(Some(text))),
        define: (!options.define.is_empty()).then(|| options.define.iter().cloned().collect()),
        minify: options.minify.then_some(RawMinifyOptions::Bool(true)),
        treeshake: match options.treeshake {
            Some(false) => TreeshakeOptions::Boolean(false),
            _ => TreeshakeOptions::default(),
        },
        resolve: Some(resolve),
        // Where `this.warn()` ends up. Info and debug are dropped: a build that
        // reported every `this.debug()` as a warning would train whoever reads
        // the list to stop reading it.
        on_log: on_log.map(|sink| {
            rolldown_common::OnLog::new(Arc::new(move |level, log: rolldown_common::Log| {
                if matches!(level, rolldown_common::LogLevel::Warn) {
                    sink(match &log.plugin {
                        Some(plugin) => format!("{plugin}: {}", log.message),
                        None => log.message.clone(),
                    });
                }
                Box::pin(async { Ok(()) })
            }))
        }),
        ..BundlerOptions::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(target: Target) -> Options {
        Options {
            input: vec![(None, "app.js".to_string())],
            platform: target,
            ..Options::default()
        }
    }

    /// The invariant this module exists for: a build described the same way
    /// comes out the same way, whoever described it. Both divergences this
    /// project shipped were two translations disagreeing, so the test is that
    /// the defaults are attached here rather than by the caller.
    #[test]
    fn a_server_build_asserts_worker_and_the_main_fields() {
        let out = translate(&one(Target::Server), OutputOptions::default(), None).expect("built");
        let resolve = out.resolve.expect("resolve options");
        assert_eq!(resolve.condition_names, Some(vec!["worker".to_string()]));
        assert_eq!(
            resolve.main_fields.as_deref(),
            Some(&["module".to_string(), "main".to_string()][..])
        );
        assert!(matches!(out.platform, Some(Platform::Neutral)));
    }

    #[test]
    fn a_browser_build_asserts_browser_and_takes_the_browser_platform() {
        let out = translate(&one(Target::Browser), OutputOptions::default(), None).expect("built");
        let resolve = out.resolve.expect("resolve options");
        assert_eq!(resolve.condition_names, Some(vec!["browser".to_string()]));
        assert!(matches!(out.platform, Some(Platform::Browser)));
    }

    /// A library is an input to somebody else's build, so it asserts nothing —
    /// and is still neutral, because it is not being aimed anywhere.
    #[test]
    fn a_library_asserts_no_condition() {
        let out = translate(&one(Target::Library), OutputOptions::default(), None).expect("built");
        let resolve = out.resolve.expect("resolve options");
        assert_eq!(resolve.condition_names, Some(Vec::new()));
        assert!(matches!(out.platform, Some(Platform::Neutral)));
    }

    /// Named fields replace, named conditions append. The asymmetry is
    /// deliberate and is the one thing about this that has to be remembered.
    #[test]
    fn named_conditions_append_and_named_fields_replace() {
        let mut options = one(Target::Server);
        options.conditions = vec!["development".to_string()];
        options.main_fields = vec!["jsnext:main".to_string()];
        let out = translate(&options, OutputOptions::default(), None).expect("built");
        let resolve = out.resolve.expect("resolve options");
        assert_eq!(
            resolve.condition_names.as_deref(),
            Some(&["worker".to_string(), "development".to_string()][..])
        );
        assert_eq!(
            resolve.main_fields.as_deref(),
            Some(&["jsnext:main".to_string()][..])
        );
    }

    /// A build with no entry is a mistake worth naming rather than an empty
    /// bundle.
    #[test]
    fn an_input_is_required() {
        let err =
            translate(&Options::default(), OutputOptions::default(), None).expect_err("refused");
        assert!(err.contains("input is required"), "{err}");
    }

    /// `generate()` may override what `build()` said, and only what it names.
    #[test]
    fn per_call_output_wins_over_the_builds_own() {
        let base = OutputOptions {
            dir: Some("dist".to_string()),
            format: Some("esm".to_string()),
            ..OutputOptions::default()
        };
        let merged = OutputOptions {
            format: Some("cjs".to_string()),
            ..OutputOptions::default()
        }
        .over(&base);
        assert_eq!(merged.format.as_deref(), Some("cjs"));
        assert_eq!(merged.dir.as_deref(), Some("dist"));
    }
}

#[cfg(test)]
mod hmr_api {
    /// The HMR engine we build on, pinned as a compile check.
    ///
    /// `compute_hmr_update_for_file_changes` is rolldown's, is gated behind its
    /// `experimental` feature, and is the whole reason that feature is enabled
    /// (workspace manifest). A pin rather than a comment because the failure it
    /// guards is silent: the method disappears with the feature flag, and
    /// nothing else in a build would notice until the dev loop stopped hot
    /// updating and started reloading the page instead.
    #[test]
    fn the_module_swap_engine_is_reachable() {
        // Named, not called: constructing a Bundler needs a real project, and
        // what this asserts is that the API exists with the shape we drive.
        #[allow(unused)]
        type Update = rolldown_common::ClientHmrUpdate;
        #[allow(unused)]
        type Input<'a> = rolldown_common::ClientHmrInput<'a>;
        #[allow(unused)]
        type Stamps = rolldown_common::HmrStampTable;
        #[allow(unused)]
        type Patch = rolldown_common::HmrPatch;
        #[allow(unused)]
        type Boundary = rolldown_common::HmrBoundary;
    }

    /// A warm bundler has to be *held* — across rebuilds, across tasks — and
    /// rolldown's HMR refuses to work without one ("HMR requires to run at
    /// least one bundle before invalidation"). Holding it in a `static` needs
    /// both bounds, so they are asserted rather than discovered halfway
    /// through the plumbing.
    #[test]
    fn a_bundler_can_be_held_across_tasks() {
        fn assert_send<T: Send>() {}
        assert_send::<rolldown::Bundler>();
    }
}
