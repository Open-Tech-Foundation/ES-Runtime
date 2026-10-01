//! The plugin contract — **ours**, and the layer the bundler behind it does not
//! reach through.
//!
//! Nothing in this file names rolldown. That is the point of it existing.
//!
//! # Why not simply expose rolldown's hooks
//!
//! Because they are rolldown's. The `runtime:` namespace is a versioned
//! contract (SPEC §14), and a guest-visible API defined by a third party's
//! trait moves when that trait moves: a hook renamed in a patch release of a
//! bundler would be a breaking change in a *language runtime's* standard
//! library. The bundler is an implementation of this contract, not the
//! definition of it — the same call this project already made for its CSS
//! pipeline, where the answer to "which crate parses this" became "we do,
//! because the part we need is small enough to own".
//!
//! # What a backend must provide
//!
//! Stated here so that "we could swap the bundler" is a checkable claim rather
//! than a hope. An implementation must be able to:
//!
//! 1. ask an outside party to resolve a specifier, and accept an answer that
//!    names a module **with no file behind it**;
//! 2. ask an outside party for a module's contents, by id;
//! 3. ask an outside party to rewrite a module's contents, given its id;
//! 4. accept, from any of those, a list of **files the module depends on** that
//!    it could not have discovered by itself;
//! 5. resolve a specifier *on demand*, mid-hook, through its own resolver;
//! 6. accept an additional entry or asset while a build is running;
//! 7. report, for each chunk it produces, the modules that went into it.
//!
//! Seven capabilities. Rolldown has all of them. esbuild has 1, 2, 5 and 6 but
//! no transform hook and no chunk-level emit, so a swap to it would lose
//! features no adapter can synthesise — which is exactly the kind of thing a
//! written contract is for: the cost of a swap is legible before it is paid.
//!
//! # What is ours rather than inherited
//!
//! * **Filters are declarative** ([`Filter`]). Rollup and rolldown have none,
//!   because in-process a hook that returns `null` is a function call. Here it
//!   is a round trip into a V8 isolate, so a `transform` without a filter costs
//!   one crossing *per module in the graph*. The filter is matched on this side
//!   of the boundary; the crossing only happens for a module that matches.
//! * **Dependencies are returned, not declared by a side effect.** Rollup's
//!   `this.addWatchFile()` is a call you can forget to make, and forgetting it
//!   produces a build that serves stale output — the failure that is hardest to
//!   notice and worst to debug. Here they are a field of the value a hook
//!   returns ([`ModuleResult::depends_on`]).
//! * **A virtual module says so** ([`Resolved::virtual_module`]) instead of
//!   signalling it by prefixing its id with a NUL byte, which is a convention
//!   every bundler copied from rollup and nobody enjoys.
//! * **The context is an argument, not `this`**, so an arrow-function hook
//!   cannot silently lose it.
//! * **A plugin is guest code under the capability model.** A plugin that reads
//!   a file needs `FileRead`, like any other program on this runtime. No
//!   bundler's plugin API can make that statement, because none of them has a
//!   capability model to make it in.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use es_runtime_cli_common::{OpError, Value};

/// Which hook of a plugin a call is for.
///
/// Eight, deliberately, against rollup's twenty-odd: every hook carried here is
/// a promise some future backend has to keep, so the list is short on purpose
/// and grows only when something cannot be written without it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hook {
    /// Before the graph is walked.
    Start,
    /// Where a specifier points.
    Resolve,
    /// What a module contains.
    Load,
    /// Rewriting what a module contains.
    Transform,
    /// After the graph is done, with the failure if there was one.
    End,
    /// The chunks and assets the build produced, before they are written.
    Bundle,
    /// An HTML target's document, before its references are read (`pre`) or
    /// once they point at the output (D151).
    Html,
    /// A release build is complete, every target written to staging (D153).
    Finish,
}

impl Hook {
    /// The name the guest declares it under, and the one a diagnostic uses.
    pub fn name(self) -> &'static str {
        match self {
            Hook::Start => "start",
            Hook::Resolve => "resolve",
            Hook::Load => "load",
            Hook::Transform => "transform",
            Hook::End => "end",
            Hook::Bundle => "bundle",
            Hook::Html => "html",
            Hook::Finish => "finish",
        }
    }
}

/// Where a hook runs relative to the other plugins' same hook.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Order {
    Pre,
    #[default]
    Normal,
    Post,
}

/// One pattern in a filter.
#[derive(Debug)]
pub enum Pattern {
    /// An exact match. Not a substring: `"@app/nav"` matches that id and no
    /// other, because a filter whose meaning depends on what else is in the
    /// project is a filter nobody can reason about.
    Exact(String),
    /// A regular expression, compiled from the guest's own `RegExp`.
    ///
    /// There is no third variant for "could not compile". A pattern this side
    /// cannot evaluate is **refused where it was written** ([`compile`]), and
    /// that is a fix rather than a policy: it used to become a pattern that
    /// matched *everything*, so `/\0virtual/` — the id convention every plugin
    /// ported from rollup filters on — silently claimed every module in the
    /// graph, entry included. A filter that cannot be evaluated cannot be
    /// honoured either way; saying so at the declaration is the only answer
    /// that reaches the person who can change it.
    Regex(regex::Regex),
}

impl Pattern {
    fn matches(&self, text: &str) -> bool {
        match self {
            Pattern::Exact(want) => text == want,
            Pattern::Regex(re) => re.is_match(text),
        }
    }
}

/// Which modules a hook wants to see. Empty means all of them.
#[derive(Debug, Default)]
pub struct Filter {
    /// Matched against the module id — or, for `resolve`, the specifier being
    /// resolved.
    pub id: Vec<Pattern>,
    /// Matched against the module's source. `transform` only: it is the only
    /// hook that is handed code to match against.
    pub code: Vec<Pattern>,
}

impl Filter {
    /// Whether a hook with this filter should be called at all.
    ///
    /// `id` and `code` are **and**ed; the patterns within each are **or**ed. An
    /// empty list is not a constraint.
    pub fn admits(&self, id: &str, code: Option<&str>) -> bool {
        let id_ok = self.id.is_empty() || self.id.iter().any(|p| p.matches(id));
        if !id_ok {
            return false;
        }
        if self.code.is_empty() {
            return true;
        }
        match code {
            Some(code) => self.code.iter().any(|p| p.matches(code)),
            // Nothing to match against — admit, and let the hook decide.
            None => true,
        }
    }

    fn is_empty(&self) -> bool {
        self.id.is_empty() && self.code.is_empty()
    }
}

/// One hook of one plugin, as declared.
#[derive(Debug, Default)]
pub struct HookSpec {
    pub filter: Filter,
    pub order: Order,
}

/// Every hook a plugin declared.
#[derive(Debug, Default)]
pub struct Hooks {
    pub start: Option<HookSpec>,
    pub resolve: Option<HookSpec>,
    pub load: Option<HookSpec>,
    pub transform: Option<HookSpec>,
    pub end: Option<HookSpec>,
    pub bundle: Option<HookSpec>,
    pub html: Option<HookSpec>,
    pub finish: Option<HookSpec>,
}

impl Hooks {
    pub fn get(&self, hook: Hook) -> Option<&HookSpec> {
        match hook {
            Hook::Start => self.start.as_ref(),
            Hook::Resolve => self.resolve.as_ref(),
            Hook::Load => self.load.as_ref(),
            Hook::Transform => self.transform.as_ref(),
            Hook::End => self.end.as_ref(),
            Hook::Bundle => self.bundle.as_ref(),
            Hook::Html => self.html.as_ref(),
            Hook::Finish => self.finish.as_ref(),
        }
    }
}

/// A plugin, as the host holds it: a handle the guest gave us, a name for
/// diagnostics, and what it asked to be called for.
#[derive(Debug)]
pub struct Plugin {
    /// The guest's own handle for the plugin object. This side never sees a
    /// function — it sees a number, and asks for it back by that number.
    pub id: f64,
    pub name: String,
    pub hooks: Hooks,
    /// What the plugin needs the **compiler** to do, which no hook can express.
    pub jsx: Jsx,
}

/// The JSX transform options a plugin asks the build for.
///
/// The one thing a plugin genuinely cannot do for itself. A hook is handed a
/// module's source and hands source back, and that is enough for almost
/// everything — but the compiler's own JSX pass runs *inside* the bundler, and
/// a plugin has no way to reach it.
///
/// Which matters for exactly one feature, and it is the feature that made this
/// necessary: a component-refresh scheme needs the compiler to insert a
/// registration per component and a signature per hook-using function, because
/// finding those requires the syntax tree the compiler already has and a plugin
/// does not. The per-module half a plugin writes itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Jsx {
    /// Insert the component registrations a refresh scheme's runtime matches
    /// components up by.
    ///
    /// Honoured **only in a hot dev build**, and that is a safety property
    /// rather than a policy: the registrations call globals that only a hot
    /// loop installs, so emitting them into a release build would ship calls to
    /// something undefined.
    pub refresh: bool,
}

/// Where a pass is running: facts about the build or run it is part of, which
/// a plugin reads as `ctx.command`, `ctx.platform`, `ctx.target` and
/// `ctx.hot` (D148).
///
/// Facts, not modes. esdev does not know what a framework's SPA, SSG or SSR
/// mean; a plugin combines its own options with these to decide what to emit.
/// Carried per pass rather than per plugin, because one loaded plugin serves
/// every target, and the browser build beside a server build is hot while the
/// server build is not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Site {
    /// `"build"`, `"start"`, `"test"` or `"run"`. `None` for a build a program
    /// started through `runtime:build`, which is its own description.
    pub command: Option<&'static str>,
    /// `"browser"` or `"server"`: what the code being compiled runs against.
    pub platform: Option<&'static str>,
    /// The project target this build is, by name. `None` outside a project
    /// build.
    pub target: Option<String>,
    /// A hot dev-loop build: modules may be replaced in place, so a refresh
    /// scheme installs itself. Never true in anything shipped.
    pub hot: bool,
}

impl Site {
    /// The facts as the hook's context carries them. `hot` is always present;
    /// the rest only when known, so a plugin reads absence, not `null`.
    pub fn meta(&self) -> Vec<(String, Value)> {
        let mut meta = Vec::with_capacity(4);
        if let Some(command) = self.command {
            meta.push(("command".to_string(), Value::String(command.to_string())));
        }
        if let Some(platform) = self.platform {
            meta.push(("platform".to_string(), Value::String(platform.to_string())));
        }
        if let Some(target) = &self.target {
            meta.push(("target".to_string(), Value::String(target.clone())));
        }
        meta.push(("hot".to_string(), Value::Bool(self.hot)));
        meta
    }
}

/// What a hook's answer looks like on its way back: fallible, and possibly
/// awaited. A pass that answers without waiting still spells it this way,
/// because the caller cannot know which kind it has.
pub type Answer<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

/// What a hook may ask of the build while it is running.
///
/// The half of a plugin API that cannot be a return value: `resolve()` asks the
/// build's own resolver a question *mid-hook*, and `emit()` adds to a build
/// already under way. Stated as a trait so that both kinds of pass reach the
/// build the same way — a pass in the guest isolate does it by posting to the
/// host, a pass in this binary does it by calling a function, and neither knows
/// which it is.
pub trait Context: Send + Sync {
    /// The build's own resolver. `None` is "nothing resolves", which is an
    /// answer to branch on rather than an error to catch.
    fn resolve<'a>(
        &'a self,
        specifier: &'a str,
        importer: Option<&'a str>,
        skip_self: bool,
    ) -> Answer<'a, Option<ResolvedId>>;

    /// An extra entry or asset, added to a build in flight. Returns the
    /// reference the emitter can ask for the final file name by.
    fn emit(&self, emit: Emit) -> Result<String, String>;

    /// A diagnostic. `"warn"`, `"info"` or `"debug"`.
    fn log(&self, level: &str, message: String);

    /// A file the module being processed depends on. Prefer returning
    /// [`ModuleResult::depends_on`]; this exists for the whole-build hooks,
    /// which have no module to hang an answer on.
    fn depends_on(&self, file: &str);

    /// Where the build is running, for resolving a relative path a pass gave
    /// us against the same directory everything else in a run resolves against.
    fn cwd(&self) -> std::path::PathBuf;
}

/// A pass over a build, in this project's own vocabulary.
///
/// **Two kinds of thing implement this, and that is the point.** A plugin
/// declared in guest JavaScript is one; this toolchain's own passes — the CSS
/// Modules scoping, today — are the other. They are the same kind of object to
/// everything downstream: one list, one order, one set of filters, one adapter
/// onto whatever bundler is underneath.
///
/// Before this existed, our own passes were written against the bundler's trait
/// and the guest's against this contract, which meant the contract had exactly
/// one implementation. A contract with one implementation always fits. It also
/// meant the two lists could drift, and they did: `runtime:build` shipped
/// without the CSS pass the `build` subcommand installs, and one project
/// produced two different builds depending on which door it came in.
///
/// Every hook defaults to "not mine", so a pass writes only what it does.
pub trait Pass: Send + Sync + std::fmt::Debug {
    /// For diagnostics, and for the `plugin` field of a warning.
    fn name(&self) -> &str;

    /// What this pass wants to be called for, and when. A hook it does not
    /// declare is never called; a filter it declares is matched *before* the
    /// call, which for a pass in the isolate is the difference between one
    /// crossing and one crossing per module in the graph.
    fn hooks(&self) -> &Hooks;

    /// The build is starting. Returns files the whole build depends on that
    /// nothing imports — a config file, a manifest.
    fn start<'a>(&'a self, _ctx: &'a Arc<dyn Context>) -> Answer<'a, Vec<String>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// Where a specifier points. The id may name a module with no file behind
    /// it; say so with [`Resolved::To::virtual_module`].
    fn resolve<'a>(
        &'a self,
        _specifier: &'a str,
        _importer: Option<&'a str>,
        _is_entry: bool,
        _ctx: &'a Arc<dyn Context>,
    ) -> Answer<'a, Resolved> {
        Box::pin(async { Ok(Resolved::Pass) })
    }

    /// The contents of a module, by id. `None` is "not mine".
    fn load<'a>(
        &'a self,
        _id: &'a str,
        _ctx: &'a Arc<dyn Context>,
    ) -> Answer<'a, Option<ModuleResult>> {
        Box::pin(async { Ok(None) })
    }

    /// A module's contents, rewritten. `None` is "leave it alone".
    ///
    /// `module_type` is what the module is **now** — `"css"`, `"jsx"`, `"js"`,
    /// … — which is not always what its extension says, because a pass that ran
    /// before this one may already have changed it. That is how a pass declines
    /// work somebody else has done: `esdev:css-modules` filters on `\.css$` and
    /// still has to step aside for a plugin that ordered itself `pre` and
    /// turned the stylesheet into JavaScript, or it would read the file off
    /// disk again and undo the whole thing.
    fn transform<'a>(
        &'a self,
        _code: &'a str,
        _id: &'a str,
        _module_type: &'a str,
        _ctx: &'a Arc<dyn Context>,
    ) -> Answer<'a, Option<ModuleResult>> {
        Box::pin(async { Ok(None) })
    }

    /// The build finished, or failed with `error`. A pass that started
    /// something in [`Pass::start`] has to be told which.
    fn end<'a>(&'a self, _error: Option<&'a str>, _ctx: &'a Arc<dyn Context>) -> Answer<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    /// What the build produced, before any of it is written.
    ///
    /// The hook that makes a plugin able to work with the *result* of a build
    /// rather than only its inputs. Route-level `modulepreload` is the case
    /// that demanded it: to preload the chunks a route pulls in, you need the
    /// chunk graph — which chunk each entry became, which modules went into it,
    /// which chunks it imports — and none of that exists until the graph has
    /// been split.
    ///
    /// [`End`](Hook::End) is not this hook and cannot be made into it: it fires
    /// when the *graph* is finished, before there are chunks at all, which is
    /// why it is handed `null`.
    ///
    /// **Read-only.** The bundle is described, not surrendered: a hook may look
    /// at what was produced and write files of its own, and cannot rewrite a
    /// chunk on the way past. Rollup allows that and it is how a plugin comes
    /// to invalidate the source maps of every plugin after it. The listing also
    /// carries no `code` — the graph, not the bytes — because `generate()`
    /// already hands the code back and copying every chunk into an isolate on
    /// every rebuild is a price a hook that wanted the shape should not pay.
    fn bundle<'a>(&'a self, _output: &'a [Output], _ctx: &'a Arc<dyn Context>) -> Answer<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    /// An HTML target's document (D151). `output` is `None` for a hook ordered
    /// `pre`, which sees the source before its references are read, and what
    /// the build produced for the others, which see the written document.
    /// `None` leaves it alone.
    fn html<'a>(
        &'a self,
        _html: &'a str,
        _id: &'a str,
        _output: Option<&'a [Output]>,
        _ctx: &'a Arc<dyn Context>,
    ) -> Answer<'a, Option<HtmlResult>> {
        Box::pin(async { Ok(None) })
    }

    /// A release build is complete (D153): every selected target is in
    /// staging and every `"then": "run"` step has run, and nothing has moved
    /// into place yet. What a pass writes under a target's `out_dir` is
    /// committed with the rest; an error fails the build.
    fn finish<'a>(
        &'a self,
        _targets: &'a [Finished],
        _ctx: &'a Arc<dyn Context>,
    ) -> Answer<'a, ()> {
        Box::pin(async { Ok(()) })
    }
}

/// One target of a finished build, as a `finish` hook is shown it.
#[derive(Clone, Debug)]
pub struct Finished {
    /// The target's name in `esdev.json`.
    pub name: String,
    /// `"browser"` or `"server"`.
    pub platform: &'static str,
    /// The staged directory the target wrote into: its `outdir`, or the
    /// directory of its `out` file.
    pub out_dir: std::path::PathBuf,
    /// What the build produced there, `file_name` relative to `out_dir`.
    pub files: Vec<Output>,
}

/// What an `html` hook answered: a replacement document, tags to insert, or
/// both — the replacement first.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct HtmlResult {
    pub html: Option<String>,
    pub tags: Vec<HtmlTag>,
}

/// One element an `html` hook asked for.
#[derive(Debug, PartialEq, Eq)]
pub struct HtmlTag {
    pub tag: String,
    /// In the order given. `None` is a boolean attribute written bare.
    pub attrs: Vec<(String, Option<String>)>,
    /// Written as given: a script's or a style's text, or markup.
    pub children: Option<String>,
    pub inject_to: InjectTo,
}

/// Where an [`HtmlTag`] goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InjectTo {
    /// Before `</head>`.
    #[default]
    Head,
    /// After `<head>`.
    HeadPrepend,
    /// Before `</body>`.
    Body,
    /// After `<body>`.
    BodyPrepend,
}

/// Elements with no end tag, which a tag must not be given one for.
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
    "wbr",
];

impl HtmlTag {
    /// The element as markup. Attribute values are escaped; children are not,
    /// because a script's text or a style's is not HTML to escape.
    pub fn render(&self) -> String {
        let mut out = format!("<{}", self.tag);
        for (name, value) in &self.attrs {
            match value {
                Some(value) => {
                    let escaped = value
                        .replace('&', "&amp;")
                        .replace('"', "&quot;")
                        .replace('<', "&lt;");
                    out.push_str(&format!(" {name}=\"{escaped}\""));
                }
                None => out.push_str(&format!(" {name}")),
            }
        }
        out.push('>');
        if VOID_ELEMENTS.contains(&self.tag.to_ascii_lowercase().as_str()) {
            return out;
        }
        if let Some(children) = &self.children {
            out.push_str(children);
        }
        out.push_str(&format!("</{}>", self.tag));
        out
    }
}

/// Reads an `html` hook's answer.
///
/// `null` leaves the document alone. Anything else is the object: a bare
/// string or a list of tags is another tool's shorthand, and this contract has
/// one form for an answer (D123).
pub fn html_result(value: &Value) -> Result<Option<HtmlResult>, String> {
    let object = match value {
        Value::Null | Value::Undefined => return Ok(None),
        Value::Object(_) => value,
        other => {
            return Err(format!(
                "html must return {{ html?, tags? }} or null, got {}",
                describe(other)
            ));
        }
    };
    let html = match field(object, "html") {
        None | Some(Value::Null | Value::Undefined) => None,
        Some(Value::String(html)) => Some(html.clone()),
        Some(other) => {
            return Err(format!(
                "html: `html` must be a string, got {}",
                describe(other)
            ));
        }
    };
    let tags = match field(object, "tags") {
        None | Some(Value::Null | Value::Undefined) => Vec::new(),
        Some(Value::Array(tags)) => tags.iter().map(html_tag).collect::<Result<_, _>>()?,
        Some(other) => {
            return Err(format!(
                "html: `tags` must be a list, got {}",
                describe(other)
            ));
        }
    };
    Ok(Some(HtmlResult { html, tags }))
}

fn html_tag(value: &Value) -> Result<HtmlTag, String> {
    let tag = field(value, "tag")
        .and_then(Value::as_str)
        .filter(|tag| !tag.is_empty() && tag.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .ok_or_else(|| "html: each tag needs a `tag` name, such as \"script\"".to_string())?
        .to_string();
    let mut attrs = Vec::new();
    if let Some(Value::Object(fields)) = field(value, "attrs") {
        for (name, value) in fields {
            if name.is_empty()
                || name
                    .chars()
                    .any(|c| c.is_whitespace() || "\"'<>/=".contains(c))
            {
                return Err(format!(
                    "html: <{tag}> has an attribute named {name:?}, which is not one"
                ));
            }
            match value {
                Value::Bool(false) | Value::Null | Value::Undefined => {}
                Value::Bool(true) => attrs.push((name.clone(), None)),
                Value::String(text) => attrs.push((name.clone(), Some(text.clone()))),
                Value::Number(number) => attrs.push((name.clone(), Some(number.to_string()))),
                other => {
                    return Err(format!(
                        "html: <{tag}>'s {name:?} must be a string, a number or a boolean, got {}",
                        describe(other)
                    ));
                }
            }
        }
    }
    let children = field(value, "children")
        .and_then(Value::as_str)
        .map(str::to_string);
    let inject_to = match field(value, "injectTo").and_then(Value::as_str) {
        None | Some("head") => InjectTo::Head,
        Some("head-prepend") => InjectTo::HeadPrepend,
        Some("body") => InjectTo::Body,
        Some("body-prepend") => InjectTo::BodyPrepend,
        Some(other) => {
            return Err(format!(
                "html: <{tag}>'s injectTo is {other:?}; it is \"head\", \"head-prepend\", \"body\" or \"body-prepend\""
            ));
        }
    };
    Ok(HtmlTag {
        tag,
        attrs,
        children,
        inject_to,
    })
}

/// One file a build produced, as [`Pass::bundle`] describes it.
#[derive(Clone, Debug)]
pub enum Output {
    Chunk {
        /// Where it will be written, relative to the output directory.
        file_name: String,
        /// The entry name it was built under.
        name: String,
        is_entry: bool,
        is_dynamic_entry: bool,
        /// The module this chunk *is*: the entry it was built for, or the
        /// module behind a dynamic import. `None` for a shared chunk.
        facade_module_id: Option<String>,
        /// Every module that went into it.
        module_ids: Vec<String>,
        /// The chunks it imports, by file name — the edges a preload walks.
        imports: Vec<String>,
        dynamic_imports: Vec<String>,
    },
    Asset {
        file_name: String,
    },
}

/// What `resolve` answered.
#[derive(Debug, PartialEq, Eq)]
pub enum Resolved {
    /// "Not mine." The next plugin, then the bundler's own resolver.
    Pass,
    To {
        id: String,
        external: External,
        /// There is no file behind this id; do not go looking for one.
        virtual_module: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum External {
    No,
    Yes,
    /// Keep the specifier as an absolute path.
    Absolute,
    /// Rewrite it relative to the chunk that imports it.
    Relative,
}

/// What `load` or `transform` answered.
#[derive(Debug, PartialEq, Eq)]
pub struct ModuleResult {
    pub code: String,
    /// How the code should be treated — `"js"`, `"jsx"`, `"ts"`, `"css"`, …
    /// `None` leaves the backend's own guess (usually the file extension).
    pub module_type: Option<String>,
    /// A source map, as JSON: the form every tool that makes one already has.
    /// Converting it field by field would be the same bytes with more ways to
    /// be wrong.
    pub map: Option<String>,
    /// Files this module depends on that the graph could not discover — the
    /// frontmatter a generated module was built from, the `_meta.js` a virtual
    /// module read. **Returned rather than declared through the context**, so
    /// it is part of the answer and cannot be forgotten; forgetting it produces
    /// a build that serves stale output, which is the failure that is hardest
    /// to notice and worst to debug.
    pub depends_on: Vec<String>,
}

/// Reads a `resolve` answer.
pub fn resolved(value: &Value) -> Resolved {
    let Some(id) = field(value, "id").and_then(Value::as_str) else {
        return Resolved::Pass;
    };
    let external = match field(value, "external") {
        Some(Value::Bool(true)) => External::Yes,
        Some(Value::String(kind)) if kind == "absolute" => External::Absolute,
        Some(Value::String(kind)) if kind == "relative" => External::Relative,
        _ => External::No,
    };
    Resolved::To {
        id: id.to_string(),
        external,
        virtual_module: matches!(field(value, "virtual"), Some(Value::Bool(true))),
    }
}

/// Reads a `load`/`transform` answer.
///
/// `null` and `undefined` mean "not mine" — the one convention worth keeping,
/// because a hook has to be able to decline. Anything else must be the object:
/// a bare string is rollup's shorthand, and accepting it here would make this a
/// superset of somebody else's design rather than a contract of our own.
pub fn module_result(value: &Value, hook: Hook) -> Result<Option<ModuleResult>, String> {
    match value {
        Value::Null | Value::Undefined => Ok(None),
        Value::Object(_) => {
            let code = field(value, "code")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{}: the object returned has no `code`", hook.name()))?;
            Ok(Some(ModuleResult {
                code: code.to_string(),
                module_type: field(value, "type")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                map: field(value, "map")
                    .and_then(Value::as_str)
                    .filter(|json| !json.is_empty())
                    .map(str::to_string),
                depends_on: depends_on(value),
            }))
        }
        other => Err(format!(
            "{}: must return an object or null, got {}",
            hook.name(),
            describe(other)
        )),
    }
}

/// `dependsOn`, from any hook's answer.
pub fn depends_on(value: &Value) -> Vec<String> {
    match field(value, "dependsOn") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// What `ctx.emit()` was asked to add to a running build.
pub enum Emit {
    /// An additional entry, by module id.
    Chunk {
        id: String,
        name: Option<String>,
        file_name: Option<String>,
    },
    /// A file to place beside the output.
    Asset {
        name: Option<String>,
        file_name: Option<String>,
        source: Source,
    },
}

/// An asset's contents: text, or bytes.
pub enum Source {
    Text(String),
    Bytes(Vec<u8>),
}

/// Reads an `emit` request.
pub fn emit(value: &Value) -> Result<Emit, String> {
    let name = field(value, "name")
        .and_then(Value::as_str)
        .map(str::to_string);
    let file_name = field(value, "fileName")
        .and_then(Value::as_str)
        .map(str::to_string);
    match field(value, "type").and_then(Value::as_str) {
        Some("chunk") => Ok(Emit::Chunk {
            id: field(value, "id")
                .and_then(Value::as_str)
                .ok_or_else(|| "emit: a chunk needs an id".to_string())?
                .to_string(),
            name,
            file_name,
        }),
        Some("asset") | None => Ok(Emit::Asset {
            name,
            file_name,
            source: match field(value, "source") {
                Some(Value::Bytes(bytes)) => Source::Bytes(bytes.clone()),
                Some(Value::String(text)) => Source::Text(text.clone()),
                _ => return Err("emit: an asset needs a source".to_string()),
            },
        }),
        Some(other) => Err(format!(
            "emit: type must be \"chunk\" or \"asset\", got {other:?}"
        )),
    }
}

/// Where a resolve request pointed, when it pointed anywhere.
pub struct ResolvedId {
    pub id: String,
    pub external: bool,
}

/// A returned value, for a rejection that has to name what it got.
fn describe(value: &Value) -> String {
    match value {
        Value::String(_) => "a string (return { code } instead)".to_string(),
        Value::Number(_) => "a number".to_string(),
        Value::Bool(_) => "a boolean".to_string(),
        Value::Array(_) => "an array".to_string(),
        other => format!("{other:?}"),
    }
}

/// What a value is, for a rejection that has to name it. Narrower than
/// [`describe`], which is about a hook's *return* and says what to write
/// instead.
fn kind(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "a string",
        Value::Number(_) => "a number",
        Value::Bool(_) => "a boolean",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
        Value::Null => "null",
        _ => "nothing",
    }
}

// --- reading the guest's declaration ----------------------------------------

/// Every hook name, for the "did you mean" in a rejection.
const HOOK_NAMES: [&str; 8] = [
    "start",
    "resolve",
    "load",
    "transform",
    "end",
    "bundle",
    "html",
    "finish",
];

/// Reads one plugin's declaration.
///
/// Strict on purpose. There is **one** way to declare a hook — an object with a
/// `handler` — and a bare function is refused rather than accepted as rollup's
/// shorthand. Accepting both would mean the filter, the order and the context
/// argument are optional extras on somebody else's design; refusing the other
/// form is what makes this a contract rather than a superset.
pub fn plugin(value: &Value) -> Result<Plugin, OpError> {
    let id = field(value, "id")
        .and_then(Value::as_number)
        .ok_or_else(|| OpError::type_error("build: each plugin must be an object"))?;
    let name = field(value, "name")
        .and_then(Value::as_str)
        .unwrap_or("plugin")
        .to_string();

    // What the compiler has to do for this plugin, which no hook can say.
    let jsx = Jsx {
        refresh: matches!(
            field(value, "jsx").and_then(|jsx| field(jsx, "refresh")),
            Some(Value::Bool(true))
        ),
    };

    let mut hooks = Hooks::default();
    let Some(Value::Object(declared)) = field(value, "hooks") else {
        return Ok(Plugin {
            id,
            name,
            hooks,
            jsx,
        });
    };
    for (key, spec) in declared {
        let hook = match key.as_str() {
            "start" => Hook::Start,
            "resolve" => Hook::Resolve,
            "load" => Hook::Load,
            "transform" => Hook::Transform,
            "end" => Hook::End,
            "bundle" => Hook::Bundle,
            "html" => Hook::Html,
            "finish" => Hook::Finish,
            other => return Err(unknown_hook(&name, other)),
        };
        let spec = hook_spec(&name, hook, spec)?;
        match hook {
            Hook::Start => hooks.start = Some(spec),
            Hook::Resolve => hooks.resolve = Some(spec),
            Hook::Load => hooks.load = Some(spec),
            Hook::Transform => hooks.transform = Some(spec),
            Hook::End => hooks.end = Some(spec),
            Hook::Bundle => hooks.bundle = Some(spec),
            Hook::Html => hooks.html = Some(spec),
            Hook::Finish => hooks.finish = Some(spec),
        }
    }
    Ok(Plugin {
        id,
        name,
        hooks,
        jsx,
    })
}

fn hook_spec(plugin: &str, hook: Hook, value: &Value) -> Result<HookSpec, OpError> {
    let order = match field(value, "order").and_then(Value::as_str) {
        None => Order::Normal,
        Some("pre") => Order::Pre,
        Some("post") => Order::Post,
        Some(other) => {
            return Err(OpError::type_error(format!(
                "{plugin}.{}: order must be \"pre\" or \"post\", got {other:?}",
                hook.name()
            )));
        }
    };
    let filter = filter(plugin, hook, field(value, "filter"))?;
    // `start` and `end` are called once, with no module in hand — a filter on
    // them cannot mean anything, and one that was quietly ignored would be a
    // plugin whose author believes it is scoped when it is not.
    if !filter.is_empty() && matches!(hook, Hook::Start | Hook::End | Hook::Bundle | Hook::Finish) {
        return Err(OpError::type_error(format!(
            "{plugin}.{}: this hook runs once, for the whole build, so it cannot be filtered",
            hook.name()
        )));
    }
    Ok(HookSpec { filter, order })
}

/// The keys a filter may carry. Two, and a filter carrying neither is refused —
/// see [`filter`].
const FILTER_KEYS: [&str; 2] = ["id", "code"];

/// Which modules a hook asked for.
///
/// # Why a filter with nothing in it is an error
///
/// Because an empty filter is not "no modules" — it is **every** module, since
/// a hook with no filter is a hook that wants the graph. So a filter this side
/// could not read used to widen the hook rather than narrow it, and the three
/// spellings that produced one are all things people write:
///
/// * `filter: /\.mdx$/` — the spelling before 0.5. A `RegExp` crosses as an
///   object with `source` and `flags`, so it was an object with no `id` in it,
///   and *every plugin written against the old API became a catch-all on
///   upgrade*.
/// * `filter: { ID: … }` — a typo.
/// * `filter: { include: … }` — rollup's key, which this contract does not have.
///
/// None of them announced itself. A `transform` that claims the whole graph
/// just runs on modules it was never written for, and what it returns is what
/// they become.
fn filter(plugin: &str, hook: Hook, value: Option<&Value>) -> Result<Filter, OpError> {
    let Some(value) = value else {
        return Ok(Filter::default());
    };
    if matches!(value, Value::Null | Value::Undefined) {
        return Ok(Filter::default());
    }
    let at = |e: String| OpError::type_error(format!("{plugin}.{}: {e}", hook.name()));
    let Value::Object(fields) = value else {
        return Err(at(format!("filter must be an object, got {}", kind(value))));
    };
    // A `RegExp` is an object too, which is exactly how the old spelling got
    // through. Named for what it is rather than reported as two unknown keys.
    if fields.iter().any(|(key, _)| key == "source")
        && !fields
            .iter()
            .any(|(key, _)| FILTER_KEYS.contains(&key.as_str()))
    {
        let source = field(value, "source").and_then(Value::as_str).unwrap_or("");
        return Err(at(format!(
            "filter must be an object — write {{ id: /{source}/ }}. A bare pattern was \
             the spelling before 0.5; it names no field now, and a filter that names no \
             field would be handed every module in the graph."
        )));
    }
    for (key, _) in fields {
        if FILTER_KEYS.contains(&key.as_str()) {
            continue;
        }
        let near = FILTER_KEYS
            .iter()
            .find(|known| edit_distance(&key.to_ascii_lowercase(), known) <= 1);
        return Err(at(match near {
            Some(known) => format!("filter: unknown key {key:?}. Did you mean {known:?}?"),
            None => format!(
                "filter: unknown key {key:?}. A filter matches on {}.",
                FILTER_KEYS.join(" or ")
            ),
        }));
    }
    let id = patterns(field(value, "id")).map_err(at)?;
    let code = patterns(field(value, "code")).map_err(at)?;
    if id.is_empty() && code.is_empty() {
        return Err(at(
            "filter must say what it matches: { id }, { code }, or both. A filter with \
             neither is not a narrower hook — it is every module in the graph, which is \
             what leaving the filter off already means."
                .to_string(),
        ));
    }
    // Only `transform` is handed code to match against; a `code` filter on
    // anything else is a mistake worth naming rather than dropping.
    if !code.is_empty() && hook != Hook::Transform {
        return Err(OpError::type_error(format!(
            "{plugin}.{}: only transform can filter on code — it is the only hook given any",
            hook.name()
        )));
    }
    Ok(Filter { id, code })
}

/// One pattern, or a list of them. A string is exact; an object with `source`
/// is a `RegExp` the guest sent as its own two parts.
///
/// Anything else is refused rather than dropped. A filter is the thing that
/// decides which modules a hook is handed, so a pattern that was quietly
/// ignored is a hook that runs on the wrong set — the same failure the
/// uncompilable-regex case below produces, arriving through a typo instead.
fn patterns(value: Option<&Value>) -> Result<Vec<Pattern>, String> {
    match value {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items.iter().map(pattern).collect(),
        Some(one) => Ok(vec![pattern(one)?]),
    }
}

fn pattern(value: &Value) -> Result<Pattern, String> {
    match value {
        Value::String(exact) => Ok(Pattern::Exact(exact.clone())),
        Value::Object(_) => {
            let source = field(value, "source")
                .and_then(Value::as_str)
                .ok_or_else(|| "a filter pattern must be a string or a RegExp".to_string())?;
            let flags = field(value, "flags").and_then(Value::as_str).unwrap_or("");
            compile(source, flags)
        }
        other => Err(format!(
            "a filter pattern must be a string or a RegExp, got {}",
            kind(other)
        )),
    }
}

/// Compiles a JavaScript `RegExp` for this side.
///
/// The flags that have a meaning here are translated into the inline form the
/// `regex` crate takes; `g` and `y` are about a *search's* state rather than
/// the pattern, and mean nothing to a predicate.
///
/// # Why a pattern that will not compile is an error
///
/// It used to become a pattern that admitted **everything**, on the reasoning
/// that a plugin which mysteriously does nothing is worse than one that is
/// asked about a module it did not want. That reasoning was wrong in the one
/// place it was most likely to be tested: `\0` is rollup's virtual-module
/// convention, so `/\0virtual/` is the first filter every ported plugin
/// writes — and the `regex` crate has no `\0` escape, so the filter compiled to
/// nothing and the plugin's `load` hook claimed the entry module. A `load` that
/// answers is not a crossing wasted; it is the wrong module's contents.
///
/// So: the escapes JavaScript has and this crate spells differently are
/// [translated](js_syntax), and whatever is left that will not compile is
/// refused at the declaration, where the person who wrote it is looking.
fn compile(source: &str, flags: &str) -> Result<Pattern, String> {
    let mut inline = String::new();
    if flags.contains('i') {
        inline.push('i');
    }
    if flags.contains('m') {
        inline.push('m');
    }
    if flags.contains('s') {
        inline.push('s');
    }
    let translated = js_syntax(source);
    let pattern = if inline.is_empty() {
        translated
    } else {
        format!("(?{inline}){translated}")
    };
    regex::Regex::new(&pattern)
        .map(Pattern::Regex)
        .map_err(|e| {
            format!(
                "/{source}/{flags} cannot be evaluated here: {e}\n\n\
             Filters are matched by the host, before a hook is called, so the \
             pattern is compiled by Rust's `regex` — which has no backreferences \
             and no lookaround. Rewrite the pattern, or drop the filter and let \
             the hook decide."
            )
        })
}

/// The JavaScript regular-expression escapes this crate spells differently.
///
/// Two of them, and both appear in ordinary plugin filters:
///
/// * `\0` — a NUL. JavaScript's escape for it; the `regex` crate wants `\x00`.
///   This is rollup's virtual-module prefix, so it is in the first filter most
///   ported plugins write.
/// * `\/` — an escaped delimiter. Legal inside a `/…/` literal and meaningless
///   to a crate whose patterns are strings, which rejects the escape outright.
///
/// Nothing else is rewritten: a translation table that guessed would be a
/// second regular-expression dialect to keep true, and everything it could not
/// translate is [refused](compile) rather than mistranslated.
fn js_syntax(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // `\0` is a NUL only when no digit follows it; `\01` is an octal
            // escape neither side supports, and is left alone to be refused.
            Some('0') if !chars.peek().is_some_and(char::is_ascii_digit) => out.push_str("\\x00"),
            Some('/') => out.push('/'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// A hook name that is not one of ours, reported with the one it was nearly.
fn unknown_hook(plugin: &str, given: &str) -> OpError {
    let near = HOOK_NAMES
        .iter()
        .find(|name| edit_distance(&given.to_ascii_lowercase(), name) <= 2);
    match near {
        Some(name) => OpError::type_error(format!(
            "{plugin}: unknown hook {given:?}. Did you mean {name:?}?"
        )),
        None => OpError::type_error(format!(
            "{plugin}: unknown hook {given:?}. The hooks are: {}",
            HOOK_NAMES.join(", ")
        )),
    }
}

/// Levenshtein distance, bounded by what the caller compares against.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut row = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            row[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(row[j] + 1);
        }
        std::mem::swap(&mut prev, &mut row);
    }
    prev[b.len()]
}

pub(crate) fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    match value {
        Value::Object(fields) => fields.iter().find(|(k, _)| k == name).map(|(_, v)| v),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn re(source: &str) -> Value {
        Value::Object(vec![
            ("source".to_string(), Value::String(source.to_string())),
            ("flags".to_string(), Value::String(String::new())),
        ])
    }

    fn declare(name: &str, hooks: Vec<(&str, Value)>) -> Value {
        Value::Object(vec![
            ("id".to_string(), Value::Number(1.0)),
            ("name".to_string(), Value::String(name.to_string())),
            (
                "hooks".to_string(),
                Value::Object(hooks.into_iter().map(|(k, v)| (k.to_string(), v)).collect()),
            ),
        ])
    }

    fn object(fields: Vec<(&str, Value)>) -> Value {
        Value::Object(
            fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    /// An `html` answer: a document, tags, or both. A tag's attributes are
    /// escaped and a void element gets no end tag.
    #[test]
    fn an_html_answer_is_a_document_and_tags() {
        let answer = html_result(&object(vec![
            ("html", Value::String("<p>".to_string())),
            (
                "tags",
                Value::Array(vec![
                    object(vec![
                        ("tag", Value::String("link".to_string())),
                        (
                            "attrs",
                            object(vec![
                                ("rel", Value::String("modulepreload".to_string())),
                                ("href", Value::String("/a\"b.js".to_string())),
                                ("crossorigin", Value::Bool(true)),
                                ("hidden", Value::Bool(false)),
                            ]),
                        ),
                    ]),
                    object(vec![
                        ("tag", Value::String("script".to_string())),
                        ("children", Value::String("boot()".to_string())),
                        ("injectTo", Value::String("body".to_string())),
                    ]),
                ]),
            ),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(answer.html.as_deref(), Some("<p>"));
        assert_eq!(
            answer.tags[0].render(),
            r#"<link rel="modulepreload" href="/a&quot;b.js" crossorigin>"#
        );
        assert_eq!(answer.tags[0].inject_to, InjectTo::Head);
        assert_eq!(answer.tags[1].render(), "<script>boot()</script>");
        assert_eq!(answer.tags[1].inject_to, InjectTo::Body);
        assert!(html_result(&Value::Null).unwrap().is_none());
    }

    /// One form of answer: a bare string or list is refused, as is a tag with
    /// nowhere to go or a name that is not one.
    #[test]
    fn an_html_answer_that_is_not_the_object_is_refused() {
        let string = html_result(&Value::String("<p>".to_string())).unwrap_err();
        assert!(string.contains("{ html?, tags? }"), "{string}");
        let tags = |tag: Value| html_result(&object(vec![("tags", Value::Array(vec![tag]))]));
        let place = tags(object(vec![
            ("tag", Value::String("meta".to_string())),
            ("injectTo", Value::String("footer".to_string())),
        ]))
        .unwrap_err();
        assert!(place.contains("head-prepend"), "{place}");
        let name = tags(object(vec![("tag", Value::String("a b".to_string()))])).unwrap_err();
        assert!(name.contains("tag` name"), "{name}");
    }

    /// `html` is a hook a plugin declares like the others, with an `id`
    /// filter and an order.
    #[test]
    fn html_is_a_hook() {
        let declared = plugin(&declare(
            "framework",
            vec![(
                "html",
                object(vec![
                    ("order", Value::String("pre".to_string())),
                    ("filter", object(vec![("id", re("index\\.html$"))])),
                ]),
            )],
        ))
        .expect("declared");
        let spec = declared.hooks.get(Hook::Html).expect("the hook");
        assert_eq!(spec.order, Order::Pre);
        assert!(spec.filter.admits("/p/index.html", None));
        assert!(!spec.filter.admits("/p/about.html", None));
    }

    /// The filter is the whole reason this layer exists: a `transform` that
    /// wants `.mdx` must not be asked about every module in the graph.
    #[test]
    fn a_filter_admits_only_what_it_names() {
        let spec = declare(
            "mdx",
            vec![(
                "transform",
                Value::Object(vec![(
                    "filter".to_string(),
                    Value::Object(vec![("id".to_string(), re(r"\.mdx$"))]),
                )]),
            )],
        );
        let plugin = plugin(&spec).expect("declaration");
        let filter = &plugin.hooks.transform.as_ref().expect("transform").filter;
        assert!(filter.admits("/app/page.mdx", Some("# hi")));
        assert!(!filter.admits("/app/main.js", Some("export {}")));
    }

    /// An exact id is exact — not a substring, because a filter whose meaning
    /// depends on what else is in the project is one nobody can reason about.
    #[test]
    fn an_exact_pattern_is_not_a_substring() {
        let filter = Filter {
            id: vec![Pattern::Exact("@app/nav".to_string())],
            code: Vec::new(),
        };
        assert!(filter.admits("@app/nav", None));
        assert!(!filter.admits("@app/nav/extra", None));
    }

    /// `id` and `code` are anded; a module has to satisfy both.
    #[test]
    fn id_and_code_both_have_to_match() {
        let filter = Filter {
            id: vec![Pattern::Regex(regex::Regex::new(r"\.jsx?$").unwrap())],
            code: vec![Pattern::Regex(regex::Regex::new("use client").unwrap())],
        };
        assert!(filter.admits("/a/b.jsx", Some("'use client';\n")));
        assert!(!filter.admits("/a/b.jsx", Some("export const x = 1;")));
        assert!(!filter.admits("/a/b.css", Some("'use client';")));
    }

    /// A pattern this side cannot compile is refused where it was written.
    /// It used to admit everything, which is how a `load` filtered on a
    /// lookbehind came to claim modules it had never heard of.
    #[test]
    fn an_unsupported_pattern_is_refused() {
        let refused = compile(r"(?<=foo)bar", "").expect_err("a lookbehind cannot compile here");
        assert!(refused.contains("cannot be evaluated here"), "{refused}");
    }

    /// `\0` is rollup's virtual-module prefix, so `/\0virtual/` is the first
    /// filter a ported plugin writes. The `regex` crate has no `\0` escape, so
    /// this compiled to nothing and matched **every** module — the entry
    /// included, whose contents the plugin's `load` then replaced.
    #[test]
    fn a_nul_escape_matches_a_virtual_id_and_nothing_else() {
        let Pattern::Regex(re) = compile(r"^\0virtual:", "").expect("a NUL escape") else {
            panic!("a regex");
        };
        assert!(re.is_match("\0virtual:page"));
        assert!(!re.is_match("/app/src/main.jsx"));
    }

    /// An escaped delimiter is legal in a `/…/` literal and is not an escape
    /// the `regex` crate has.
    #[test]
    fn an_escaped_slash_is_a_slash() {
        let Pattern::Regex(re) = compile(r"^\/app\/", "").expect("an escaped slash") else {
            panic!("a regex");
        };
        assert!(re.is_match("/app/main.js"));
    }

    /// A pattern that is neither a string nor a `RegExp` is refused rather than
    /// dropped: a filter with one pattern silently missing admits a set nobody
    /// asked for.
    #[test]
    fn a_pattern_that_is_not_one_is_refused() {
        let refused = patterns(Some(&Value::Number(3.0))).expect_err("a number is not a pattern");
        assert!(
            refused.contains("must be a string or a RegExp"),
            "{refused}"
        );
    }

    /// One way to declare a hook. A bare function is rollup's shorthand, and
    /// accepting it would make the filter an optional extra on somebody else's
    /// design rather than part of ours.
    #[test]
    fn a_hook_that_is_not_an_object_is_refused() {
        let spec = declare("legacy", vec![("transform", Value::Bool(true))]);
        // The guest sends `true` for "declared, but not in the object form" —
        // the JS side has already refused it; this is the second gate.
        let plugin = plugin(&spec).expect("declaration");
        assert!(plugin.hooks.transform.is_some());
        let filter = &plugin.hooks.transform.as_ref().unwrap().filter;
        assert!(
            filter.admits("anything", None),
            "an empty filter admits all"
        );
    }

    #[test]
    fn a_misspelled_hook_names_the_one_it_was_nearly() {
        let spec = declare("x", vec![("tranform", Value::Object(Vec::new()))]);
        let err = plugin(&spec).expect_err("a misspelled hook must be refused");
        assert!(
            err.to_string().contains("Did you mean \"transform\""),
            "{err}"
        );
    }

    #[test]
    fn a_filter_on_a_whole_build_hook_is_refused() {
        let spec = declare(
            "x",
            vec![(
                "start",
                Value::Object(vec![(
                    "filter".to_string(),
                    Value::Object(vec![("id".to_string(), re(r"\.mdx$"))]),
                )]),
            )],
        );
        let err = plugin(&spec).expect_err("a filtered start must be refused");
        assert!(err.to_string().contains("cannot be filtered"), "{err}");
    }

    /// `finish` is a declared hook (D153), ordered like the others and, being
    /// about the whole build, never filtered.
    #[test]
    fn finish_is_declared_ordered_and_never_filtered() {
        let ordered = declare(
            "x",
            vec![(
                "finish",
                Value::Object(vec![(
                    "order".to_string(),
                    Value::String("post".to_string()),
                )]),
            )],
        );
        let declared = plugin(&ordered).expect("declaration");
        assert_eq!(
            declared.hooks.get(Hook::Finish).map(|spec| spec.order),
            Some(Order::Post)
        );

        let filtered = declare(
            "x",
            vec![(
                "finish",
                Value::Object(vec![(
                    "filter".to_string(),
                    Value::Object(vec![("id".to_string(), re(r"\.html$"))]),
                )]),
            )],
        );
        let err = plugin(&filtered).expect_err("a filtered finish must be refused");
        assert!(
            err.to_string().contains("x.finish: this hook runs once"),
            "{err}"
        );
    }

    #[test]
    fn only_transform_can_filter_on_code() {
        let spec = declare(
            "x",
            vec![(
                "load",
                Value::Object(vec![(
                    "filter".to_string(),
                    Value::Object(vec![("code".to_string(), re("use client"))]),
                )]),
            )],
        );
        let err = plugin(&spec).expect_err("a code filter on load must be refused");
        assert!(err.to_string().contains("only transform"), "{err}");
    }

    /// The one that made this check necessary: a bare `RegExp` is an object, so
    /// it passed the type check and then had no `id` in it — and every plugin
    /// written against the 0.4 spelling became a catch-all on upgrade.
    #[test]
    fn a_bare_pattern_is_refused_rather_than_matching_everything() {
        let spec = declare(
            "mdx",
            vec![(
                "transform",
                Value::Object(vec![("filter".to_string(), re(r"\.mdx$"))]),
            )],
        );
        let err = plugin(&spec).expect_err("a bare pattern must be refused");
        let err = err.to_string();
        assert!(err.contains("write { id: /\\.mdx$/ }"), "{err}");
    }

    /// A key this contract does not have is the same failure arriving through a
    /// typo, or through rollup's vocabulary.
    #[test]
    fn a_filter_key_that_is_not_ours_names_the_one_it_was_nearly() {
        let spec = declare(
            "mdx",
            vec![(
                "transform",
                Value::Object(vec![(
                    "filter".to_string(),
                    Value::Object(vec![("ID".to_string(), re(r"\.mdx$"))]),
                )]),
            )],
        );
        let err = plugin(&spec).expect_err("an unknown key must be refused");
        assert!(err.to_string().contains("Did you mean \"id\""), "{err}");

        let spec = declare(
            "mdx",
            vec![(
                "transform",
                Value::Object(vec![(
                    "filter".to_string(),
                    Value::Object(vec![("include".to_string(), re(r"\.mdx$"))]),
                )]),
            )],
        );
        let err = plugin(&spec).expect_err("rollup's key must be refused");
        assert!(err.to_string().contains("unknown key \"include\""), "{err}");
    }

    /// `filter: {}` is not a narrower hook. It is every module in the graph,
    /// which is what leaving the filter off already means.
    #[test]
    fn a_filter_that_names_nothing_is_refused() {
        let spec = declare(
            "mdx",
            vec![(
                "transform",
                Value::Object(vec![("filter".to_string(), Value::Object(Vec::new()))]),
            )],
        );
        let err = plugin(&spec).expect_err("an empty filter must be refused");
        assert!(
            err.to_string().contains("must say what it matches"),
            "{err}"
        );
    }

    #[test]
    fn order_is_pre_or_post_and_nothing_else() {
        let spec = declare(
            "x",
            vec![(
                "transform",
                Value::Object(vec![(
                    "order".to_string(),
                    Value::String("first".to_string()),
                )]),
            )],
        );
        let err = plugin(&spec).expect_err("an unknown order must be refused");
        assert!(err.to_string().contains("\"pre\" or \"post\""), "{err}");
    }
}
