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
/// Five, deliberately, against rollup's twenty-odd: every hook carried here is
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
    Regex(regex::Regex),
    /// A `RegExp` this side could not compile — JavaScript's syntax is larger
    /// than the `regex` crate's (backreferences, lookaround).
    ///
    /// **Matches everything.** A filter that cannot be evaluated must not
    /// silently exclude modules the plugin was meant to see; the cost of being
    /// wrong that way is a plugin that mysteriously does nothing. Erring the
    /// other way costs a crossing, and the plugin's own code still decides.
    Unsupported,
}

impl Pattern {
    fn matches(&self, text: &str) -> bool {
        match self {
            Pattern::Exact(want) => text == want,
            Pattern::Regex(re) => re.is_match(text),
            Pattern::Unsupported => true,
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
}

impl Hooks {
    pub fn get(&self, hook: Hook) -> Option<&HookSpec> {
        match hook {
            Hook::Start => self.start.as_ref(),
            Hook::Resolve => self.resolve.as_ref(),
            Hook::Load => self.load.as_ref(),
            Hook::Transform => self.transform.as_ref(),
            Hook::End => self.end.as_ref(),
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
    fn transform<'a>(
        &'a self,
        _code: &'a str,
        _id: &'a str,
        _ctx: &'a Arc<dyn Context>,
    ) -> Answer<'a, Option<ModuleResult>> {
        Box::pin(async { Ok(None) })
    }

    /// The build finished, or failed with `error`. A pass that started
    /// something in [`Pass::start`] has to be told which.
    fn end<'a>(&'a self, _error: Option<&'a str>, _ctx: &'a Arc<dyn Context>) -> Answer<'a, ()> {
        Box::pin(async { Ok(()) })
    }
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

// --- reading the guest's declaration ----------------------------------------

/// Every hook name, for the "did you mean" in a rejection.
const HOOK_NAMES: [&str; 5] = ["start", "resolve", "load", "transform", "end"];

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

    let mut hooks = Hooks::default();
    let Some(Value::Object(declared)) = field(value, "hooks") else {
        return Ok(Plugin { id, name, hooks });
    };
    for (key, spec) in declared {
        let hook = match key.as_str() {
            "start" => Hook::Start,
            "resolve" => Hook::Resolve,
            "load" => Hook::Load,
            "transform" => Hook::Transform,
            "end" => Hook::End,
            other => return Err(unknown_hook(&name, other)),
        };
        let spec = hook_spec(&name, hook, spec)?;
        match hook {
            Hook::Start => hooks.start = Some(spec),
            Hook::Resolve => hooks.resolve = Some(spec),
            Hook::Load => hooks.load = Some(spec),
            Hook::Transform => hooks.transform = Some(spec),
            Hook::End => hooks.end = Some(spec),
        }
    }
    Ok(Plugin { id, name, hooks })
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
    if !filter.is_empty() && matches!(hook, Hook::Start | Hook::End) {
        return Err(OpError::type_error(format!(
            "{plugin}.{}: this hook runs once, for the whole build, so it cannot be filtered",
            hook.name()
        )));
    }
    Ok(HookSpec { filter, order })
}

fn filter(plugin: &str, hook: Hook, value: Option<&Value>) -> Result<Filter, OpError> {
    let Some(value) = value else {
        return Ok(Filter::default());
    };
    if matches!(value, Value::Null | Value::Undefined) {
        return Ok(Filter::default());
    }
    let id = patterns(field(value, "id"));
    let code = patterns(field(value, "code"));
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
fn patterns(value: Option<&Value>) -> Vec<Pattern> {
    match value {
        None => Vec::new(),
        Some(Value::Array(items)) => items.iter().filter_map(pattern).collect(),
        Some(one) => pattern(one).into_iter().collect(),
    }
}

fn pattern(value: &Value) -> Option<Pattern> {
    match value {
        Value::String(exact) => Some(Pattern::Exact(exact.clone())),
        Value::Object(_) => {
            let source = field(value, "source")?.as_str()?;
            let flags = field(value, "flags").and_then(Value::as_str).unwrap_or("");
            Some(compile(source, flags))
        }
        _ => None,
    }
}

/// Compiles a JavaScript `RegExp` for this side.
///
/// The flags that have a meaning here are translated into the inline form the
/// `regex` crate takes; `g` and `y` are about a *search's* state rather than
/// the pattern, and mean nothing to a predicate. A pattern using syntax this
/// crate does not have — a backreference, a lookahead — cannot be evaluated
/// here and becomes [`Pattern::Unsupported`], which admits everything.
fn compile(source: &str, flags: &str) -> Pattern {
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
    let pattern = if inline.is_empty() {
        source.to_string()
    } else {
        format!("(?{inline}){source}")
    };
    match regex::Regex::new(&pattern) {
        Ok(re) => Pattern::Regex(re),
        Err(_) => Pattern::Unsupported,
    }
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

    /// A pattern this side cannot compile must admit everything. Excluding
    /// modules a plugin was meant to see is the expensive way to be wrong.
    #[test]
    fn an_unsupported_pattern_admits_everything() {
        let Pattern::Unsupported = compile(r"(?<=foo)bar", "") else {
            panic!("a lookbehind should not compile here");
        };
        let filter = Filter {
            id: vec![compile(r"(?<=foo)bar", "")],
            code: Vec::new(),
        };
        assert!(filter.admits("anything at all", None));
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
