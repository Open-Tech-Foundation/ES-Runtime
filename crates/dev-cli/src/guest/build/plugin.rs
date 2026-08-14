//! The adapter: our [plugin contract](super::contract) on one side, rolldown on
//! the other, and the isolate boundary in the middle.
//!
//! **Everything that names rolldown in the plugin path lives here.** That is
//! not tidiness — it is what makes the contract a contract. A guest-visible API
//! defined by a third party's trait moves when that trait moves, and the
//! `runtime:` namespace is a versioned promise. Behind this file the bundler is
//! replaceable; in front of it, nothing changes if it is replaced.
//!
//! # Why the guest pulls instead of the host pushing
//!
//! Rolldown's plugin trait is async and its graph walk is parallel: `transform`
//! runs on a tokio worker, for many modules at once, on whatever thread the
//! scheduler picked. A V8 isolate is the opposite — single-threaded, and the
//! thread it belongs to is the one running the guest's own program. So every
//! hook call has to cross from "any thread, many at a time" to "that thread,
//! one at a time", and the obvious way to do it (have the host reach into the
//! isolate) is the one that cannot work: the isolate is busy running JS, and
//! nothing on another thread may touch it.
//!
//! So the direction is inverted. A hook does not call JavaScript; it **posts a
//! request and waits**. On the isolate side, `runtime:build`'s pump is an
//! ordinary async op — `build_hook()` — that resolves with the next request,
//! and the guest replies through another. The isolate never blocks, the hook
//! never touches V8, and the queue between them is a channel:
//!
//! ```text
//!   rolldown worker threads          the isolate thread
//!   ─────────────────────────        ──────────────────────────
//!   transform(code, id)  ──┐
//!   resolveId(spec)      ──┼── HookCall ──▶  build_hook() resolves
//!   load(id)             ──┘                 plugin.transform(...) runs
//!                          ◀── HookReply ──  build_hook_reply(id, …)
//! ```
//!
//! Several hooks may be in flight at once, which is the point: the pump does
//! not await one JS hook before asking for the next, so rolldown's parallelism
//! survives the crossing — it is bounded by the isolate being one thread, not
//! by the bridge being a queue of one. Each call carries an id, and the reply
//! finds its way back through it.
//!
//! # Why a filter earns its keep here and nowhere else
//!
//! In rollup a hook that returns `null` cost a function call. Here it costs a
//! **round trip into a V8 isolate**, so an unfiltered `transform` is one
//! crossing per module in the graph — four hundred of them on a middling app,
//! to reach a plugin that wanted one `.mdx` file. The contract's
//! [`Filter`](super::contract::Filter) is evaluated on *this* side, before the
//! call is posted, which is why it is declarative: a predicate the guest owns
//! could only be consulted by crossing.
//!
//! # `this`
//!
//! A hook's context is the other half of the API, and the half that cannot be
//! a message: `ctx.resolve()` asks the *bundler's own resolver* a question
//! mid-hook. [`PluginContext`] is `Arc`-backed and `Send`, so each in-flight
//! call parks its context beside its reply channel, and the context ops reach
//! it by call id — for exactly as long as that hook is running.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::anyhow;
use es_runtime_cli_common::Value;
use rolldown::plugin::{
    HookBuildEndArgs, HookBuildStartArgs, HookLoadArgs, HookLoadOutput, HookLoadReturn,
    HookNoopReturn, HookResolveIdArgs, HookResolveIdOutput, HookResolveIdReturn, HookTransformArgs,
    HookTransformOutput, HookTransformOutputMap, HookTransformReturn, HookUsage, Plugin,
    PluginContext, PluginHookMeta, PluginOrder, SharedLoadPluginContext,
    SharedTransformPluginContext,
};
use rolldown_common::{ModuleType, ResolvedExternal};

use super::contract::{self, Hook, Order};
use tokio::sync::{mpsc, oneshot};

/// One hook call on its way to the guest.
pub struct HookCall {
    /// Identifies the call, so the reply and the context ops find it again.
    pub id: u64,
    /// Which JS plugin object the guest should dispatch to. Assigned by the
    /// guest itself when it registers the plugin, so this side never has to
    /// know what a plugin *is*.
    pub plugin: f64,
    /// The hook's name, spelled as the guest writes it.
    pub hook: &'static str,
    /// Its arguments, in the order the guest's function takes them. The
    /// context is appended after these by the guest, so every hook reads
    /// *data first, context last* — uniformly, with nothing positional in
    /// between for a signature to get wrong.
    pub args: Vec<Value>,
    /// What the context carries for this call, beyond the methods every
    /// context has: `isEntry` on a resolve, and nothing so far on the rest.
    pub meta: Vec<(String, Value)>,
}

/// What the guest's hook returned, or threw.
pub enum HookReply {
    Returned(Value),
    Threw(String),
}

/// The context of the hook currently running, kept alive for the length of the
/// call so `this.resolve()` and friends have something to reach through.
#[derive(Clone)]
pub enum HookCtx {
    Plain(PluginContext),
    Load(SharedLoadPluginContext),
    Transform(SharedTransformPluginContext),
}

impl HookCtx {
    /// The plugin context underneath, whichever flavour of hook this is.
    pub fn plugin(&self) -> &PluginContext {
        match self {
            HookCtx::Plain(ctx) => ctx,
            HookCtx::Load(ctx) => &ctx.inner,
            HookCtx::Transform(ctx) => &ctx.inner,
        }
    }

    /// Declares a file the module being processed depends on.
    ///
    /// Routed through the *load* context when there is one, because that
    /// variant records the file against the module as well as globally — which
    /// is the difference between "this build read `_meta.js`" and "this module
    /// must be rebuilt when `_meta.js` changes", and fine-grained invalidation
    /// needs the second.
    pub fn add_watch_file(&self, file: &str) {
        match self {
            HookCtx::Load(ctx) => ctx.add_watch_file(file),
            other => other.plugin().add_watch_file(file),
        }
    }
}

/// The queue between rolldown's threads and the isolate's.
pub struct Bridge {
    calls: mpsc::UnboundedSender<HookCall>,
    state: std::sync::Mutex<State>,
    next_id: AtomicU64,
}

#[derive(Default)]
struct State {
    waiting: HashMap<u64, oneshot::Sender<HookReply>>,
    contexts: HashMap<u64, HookCtx>,
}

impl fmt::Debug for Bridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Bridge")
    }
}

impl Bridge {
    /// Builds the bridge and hands back the receiving end the isolate's pump
    /// reads.
    pub fn new() -> (Arc<Bridge>, mpsc::UnboundedReceiver<HookCall>) {
        let (calls, rx) = mpsc::unbounded_channel();
        (
            Arc::new(Bridge {
                calls,
                state: std::sync::Mutex::new(State::default()),
                next_id: AtomicU64::new(1),
            }),
            rx,
        )
    }

    /// Posts one hook call and waits for the guest's answer.
    pub async fn call(
        &self,
        plugin: f64,
        hook: &'static str,
        args: Vec<Value>,
        meta: Vec<(String, Value)>,
        ctx: Option<HookCtx>,
    ) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (reply_tx, reply_rx) = oneshot::channel();
        {
            let mut state = self.state.lock().expect("bridge state");
            state.waiting.insert(id, reply_tx);
            if let Some(ctx) = ctx {
                state.contexts.insert(id, ctx);
            }
        }
        let posted = self.calls.send(HookCall {
            id,
            plugin,
            hook,
            args,
            meta,
        });
        let outcome = match posted {
            // The program dropped the pump — it exited, or the run was
            // terminated. Nothing will ever answer, so fail the build rather
            // than wait out a build that cannot finish.
            Err(_) => Err(anyhow!("the build was abandoned")),
            Ok(()) => match reply_rx.await {
                Ok(HookReply::Returned(value)) => Ok(value),
                Ok(HookReply::Threw(message)) => Err(anyhow!("{message}")),
                Err(_) => Err(anyhow!("the build was abandoned")),
            },
        };
        // The hook is over either way: its context must not outlive it, or a
        // stale `this.resolve()` would reach into a finished build.
        self.finish(id);
        outcome
    }

    /// Delivers the guest's answer to a waiting hook. Unknown ids are ignored:
    /// a reply that arrives after its build was abandoned is not an error.
    pub fn reply(&self, id: u64, reply: HookReply) {
        let waiting = self.state.lock().expect("bridge state").waiting.remove(&id);
        if let Some(tx) = waiting {
            let _ = tx.send(reply);
        }
    }

    /// The context of an in-flight call, for the `this.*` ops.
    pub fn context(&self, id: u64) -> Option<HookCtx> {
        self.state
            .lock()
            .expect("bridge state")
            .contexts
            .get(&id)
            .cloned()
    }

    fn finish(&self, id: u64) {
        let mut state = self.state.lock().expect("bridge state");
        state.waiting.remove(&id);
        state.contexts.remove(&id);
    }
}

/// One plugin from the contract, wearing rolldown's trait.
///
/// The translation is the whole of this type: hooks in, filters applied,
/// contract-shaped arguments across the bridge, contract-shaped answers back,
/// rolldown's types on the way out.
#[derive(Debug)]
pub struct JsPlugin {
    bridge: Arc<Bridge>,
    /// Shared rather than cloned: a filter is a compiled regular expression,
    /// and a dev server builds forty times a minute.
    plugin: Arc<contract::Plugin>,
    usage: HookUsage,
}

impl JsPlugin {
    pub fn new(bridge: Arc<Bridge>, plugin: Arc<contract::Plugin>) -> Self {
        // Declaring the usage is not bookkeeping: a hook rolldown does not know
        // this plugin has is a hook it never calls, and one it thinks it has is
        // a crossing per module. Derived from the declaration rather than
        // trusted from the guest.
        let mut usage = HookUsage::empty();
        if plugin.hooks.start.is_some() {
            usage |= HookUsage::BuildStart;
        }
        if plugin.hooks.resolve.is_some() {
            usage |= HookUsage::ResolveId;
        }
        if plugin.hooks.load.is_some() {
            usage |= HookUsage::Load;
        }
        if plugin.hooks.transform.is_some() {
            usage |= HookUsage::Transform;
        }
        if plugin.hooks.end.is_some() {
            usage |= HookUsage::BuildEnd;
        }
        JsPlugin {
            bridge,
            plugin,
            usage,
        }
    }

    /// Whether this hook wants this module, decided **here** — before anything
    /// crosses into the isolate.
    fn admits(&self, hook: Hook, id: &str, code: Option<&str>) -> bool {
        self.plugin
            .hooks
            .get(hook)
            .is_some_and(|spec| spec.filter.admits(id, code))
    }

    fn order(&self, hook: Hook) -> Option<PluginHookMeta> {
        let order = match self.plugin.hooks.get(hook)?.order {
            Order::Pre => PluginOrder::Pre,
            Order::Post => PluginOrder::Post,
            Order::Normal => return None,
        };
        Some(PluginHookMeta { order: Some(order) })
    }

    async fn call(&self, hook: Hook, args: Vec<Value>, ctx: HookCtx) -> anyhow::Result<Value> {
        self.call_with(hook, args, Vec::new(), ctx).await
    }

    async fn call_with(
        &self,
        hook: Hook,
        args: Vec<Value>,
        meta: Vec<(String, Value)>,
        ctx: HookCtx,
    ) -> anyhow::Result<Value> {
        self.bridge
            .call(self.plugin.id, hook.name(), args, meta, Some(ctx))
            .await
    }
}

impl Plugin for JsPlugin {
    fn name(&self) -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Owned(self.plugin.name.clone())
    }

    fn register_hook_usage(&self) -> HookUsage {
        self.usage
    }

    fn build_start_meta(&self) -> Option<PluginHookMeta> {
        self.order(Hook::Start)
    }

    fn resolve_id_meta(&self) -> Option<PluginHookMeta> {
        self.order(Hook::Resolve)
    }

    fn load_meta(&self) -> Option<PluginHookMeta> {
        self.order(Hook::Load)
    }

    fn transform_meta(&self) -> Option<PluginHookMeta> {
        self.order(Hook::Transform)
    }

    fn build_end_meta(&self) -> Option<PluginHookMeta> {
        self.order(Hook::End)
    }

    async fn build_start(
        &self,
        ctx: &PluginContext,
        _args: &HookBuildStartArgs<'_>,
    ) -> HookNoopReturn {
        let out = self
            .call(Hook::Start, Vec::new(), HookCtx::Plain(ctx.clone()))
            .await?;
        // A build-wide input: a config file or a manifest the whole build was
        // read from, which no module imports.
        for file in contract::depends_on(&out) {
            ctx.add_watch_file(&file);
        }
        Ok(())
    }

    async fn resolve_id(
        &self,
        ctx: &PluginContext,
        args: &HookResolveIdArgs<'_>,
    ) -> HookResolveIdReturn {
        // `resolve` filters on the *specifier* — the id does not exist yet.
        if !self.admits(Hook::Resolve, args.specifier, None) {
            return Ok(None);
        }
        let out = self
            .call_with(
                Hook::Resolve,
                vec![
                    Value::String(args.specifier.to_string()),
                    match args.importer {
                        Some(importer) => Value::String(importer.to_string()),
                        None => Value::Null,
                    },
                ],
                // On the context rather than a third positional argument, so
                // `(source, importer, ctx)` is the signature and stays the
                // signature if anything else is ever added.
                vec![("isEntry".to_string(), Value::Bool(args.is_entry))],
                HookCtx::Plain(ctx.clone()),
            )
            .await?;
        Ok(match contract::resolved(&out) {
            contract::Resolved::Pass => None,
            contract::Resolved::To {
                id,
                external,
                virtual_module,
            } => Some(HookResolveIdOutput {
                // A module the plugin invented: rolldown's way of saying "no
                // file behind this" is the `\0` prefix every bundler inherited
                // from rollup. Applied here rather than asked of the plugin
                // author, and stripped again before any id reaches them.
                external: match external {
                    contract::External::No => None,
                    contract::External::Yes => Some(ResolvedExternal::Bool(true)),
                    contract::External::Absolute => Some(ResolvedExternal::Absolute),
                    contract::External::Relative => Some(ResolvedExternal::Relative),
                },
                ..HookResolveIdOutput::from_id(if virtual_module { virtual_id(&id) } else { id })
            }),
        })
    }

    async fn load(&self, ctx: SharedLoadPluginContext, args: &HookLoadArgs<'_>) -> HookLoadReturn {
        // The id the plugin knows it by. A virtual module carries the backend's
        // NUL prefix internally, and a filter written against `"@app/nav"` has
        // to match the module the plugin itself named.
        let id = guest_id(args.id);
        if !self.admits(Hook::Load, id, None) {
            return Ok(None);
        }
        let ctx = HookCtx::Load(ctx);
        let out = self
            .call(Hook::Load, vec![Value::String(id.to_string())], ctx.clone())
            .await?;
        let Some(result) = contract::module_result(&out, Hook::Load).map_err(|e| anyhow!(e))?
        else {
            return Ok(None);
        };
        let result = convert(result, Hook::Load)?;
        declare(&ctx, &result.depends_on);
        Ok(Some(HookLoadOutput {
            code: result.code.into(),
            map: result.map,
            module_type: result.module_type,
            ..HookLoadOutput::default()
        }))
    }

    async fn transform(
        &self,
        ctx: SharedTransformPluginContext,
        args: &HookTransformArgs<'_>,
    ) -> HookTransformReturn {
        // The crossing this filter exists to avoid: without it, every module in
        // the graph is shipped into the isolate so the plugin can look at its
        // id and hand it straight back.
        let id = guest_id(args.id);
        if !self.admits(Hook::Transform, id, Some(args.code)) {
            return Ok(None);
        }
        let ctx = HookCtx::Transform(ctx);
        let out = self
            .call(
                Hook::Transform,
                vec![
                    Value::String(args.code.to_string()),
                    Value::String(id.to_string()),
                ],
                ctx.clone(),
            )
            .await?;
        let Some(result) =
            contract::module_result(&out, Hook::Transform).map_err(|e| anyhow!(e))?
        else {
            return Ok(None);
        };
        let result = convert(result, Hook::Transform)?;
        declare(&ctx, &result.depends_on);
        Ok(Some(HookTransformOutput {
            code: Some(result.code),
            map: match result.map {
                Some(map) => HookTransformOutputMap::Sourcemap(Box::new(map)),
                // `Omitted`, not `Null`: the plugin said nothing about the map,
                // which leaves the chain alone. `Null` would *break* it.
                None => HookTransformOutputMap::Omitted,
            },
            module_type: result.module_type,
            ..HookTransformOutput::default()
        }))
    }

    async fn build_end(
        &self,
        ctx: &PluginContext,
        args: Option<&HookBuildEndArgs<'_>>,
    ) -> HookNoopReturn {
        // The failure, when there was one: a plugin that started something in
        // `start` has to be told the build died rather than finished.
        let error = match args {
            Some(args) => Value::String(
                args.errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            None => Value::Null,
        };
        self.call(Hook::End, vec![error], HookCtx::Plain(ctx.clone()))
            .await?;
        Ok(())
    }
}

/// Hands the backend the files a hook said its module depends on.
///
/// The contract returns them; rolldown takes them one at a time through the
/// context. Which is the translation this whole file is: the guest's shape is
/// the one that cannot be got wrong, and the conversion happens once, here.
///
/// A relative path is resolved against the build's directory, because rolldown
/// wants an absolute one and a guest writing `dependsOn: ["docs/_meta.js"]`
/// means the same thing there as everywhere else in a run. Without this the
/// entry lands verbatim, and the same file appears in `watchFiles` twice —
/// once as the graph found it and once as the plugin spelled it — so a
/// consumer matching a change against its dependency set misses half the time.
fn declare(ctx: &HookCtx, files: &[String]) {
    let cwd = ctx.plugin().cwd().clone();
    for file in files {
        let path = std::path::Path::new(file);
        if path.is_absolute() {
            ctx.add_watch_file(file);
        } else {
            ctx.add_watch_file(&cwd.join(path).to_string_lossy());
        }
    }
}

/// The id a virtual module is given inside the bundler.
///
/// Rolldown, like every bundler descended from rollup, marks "there is no file
/// behind this" by a leading NUL byte in the id — a convention the plugin
/// author is normally expected to know and apply by hand. The contract has a
/// `virtual: true` flag instead, and the convention is applied here, which is
/// where a backend's private notation belongs.
fn virtual_id(id: &str) -> String {
    if id.starts_with('\0') {
        id.to_string()
    } else {
        format!("\0{id}")
    }
}

/// The id as the plugin named it, with the backend's private notation removed.
fn guest_id(id: &str) -> &str {
    id.strip_prefix('\0').unwrap_or(id)
}

/// A contract answer, in rolldown's types.
struct Module {
    code: String,
    map: Option<rolldown_sourcemap::SourceMap>,
    module_type: Option<ModuleType>,
    depends_on: Vec<String>,
}

fn convert(result: contract::ModuleResult, hook: Hook) -> anyhow::Result<Module> {
    let map = match result.map {
        Some(json) => Some(
            rolldown_sourcemap::OwnedSourceMap::from_json_string(&json)
                .map(rolldown_sourcemap::SourceMap::from)
                .map_err(|e| {
                    anyhow!("{}: the source map returned is not valid: {e}", hook.name())
                })?,
        ),
        None => None,
    };
    let module_type = match &result.module_type {
        Some(name) => Some(
            ModuleType::from_known_str(name)
                .map_err(|_| anyhow!("{}: unknown module type {name:?}", hook.name()))?,
        ),
        None => None,
    };
    Ok(Module {
        code: result.code,
        map,
        module_type,
        depends_on: result.depends_on,
    })
}

// --- the context, from the ops' side ----------------------------------------
//
// The three below are what `ctx.resolve()`, `ctx.emit()` and `ctx.warn()` reach
// when the guest calls them. They live here rather than beside the ops for the
// reason the whole file exists: this is where the backend is named, and an op
// that constructed a `rolldown_common::EmittedAsset` would put it back in front
// of the contract.

/// `ctx.resolve(source, importer)` — the bundler's own resolver, mid-hook.
pub async fn ctx_resolve(
    ctx: &HookCtx,
    specifier: &str,
    importer: Option<&str>,
    skip_self: bool,
) -> Result<Option<contract::ResolvedId>, String> {
    let options = rolldown::plugin::PluginContextResolveOptions {
        skip_self,
        ..rolldown::plugin::PluginContextResolveOptions::default()
    };
    match ctx
        .plugin()
        .resolve(specifier, importer, Some(options))
        .await
    {
        Err(err) => Err(err.to_string()),
        // Unresolvable is `null`, not a throw — the answer a plugin branches
        // on, rather than an exception it has to catch to ask a question.
        Ok(Err(_)) => Ok(None),
        Ok(Ok(resolved)) => Ok(Some(contract::ResolvedId {
            id: resolved.id.to_string(),
            external: !matches!(resolved.external, ResolvedExternal::Bool(false)),
        })),
    }
}

/// `ctx.emit({ type, … })` — an entry or an asset, added to a running build.
pub fn ctx_emit(ctx: &HookCtx, emit: contract::Emit) -> Result<String, String> {
    let plugin = ctx.plugin();
    match emit {
        contract::Emit::Chunk {
            id,
            name,
            file_name,
        } => plugin
            .emit_chunk(rolldown_common::EmittedChunk {
                name: name.map(Into::into),
                file_name: file_name.map(Into::into),
                id,
                importer: None,
                preserve_entry_signatures: None,
            })
            .map(|reference| reference.to_string())
            .map_err(|e| e.to_string()),
        contract::Emit::Asset {
            name,
            file_name,
            source,
        } => plugin
            .emit_file(
                rolldown_common::EmittedAsset {
                    name,
                    original_file_name: None,
                    file_name: file_name.map(Into::into),
                    source: match source {
                        contract::Source::Text(text) => rolldown_common::StrOrBytes::Str(text),
                        contract::Source::Bytes(bytes) => rolldown_common::StrOrBytes::Bytes(bytes),
                    },
                },
                None,
                None,
            )
            .map(|reference| reference.to_string())
            .map_err(|e| e.to_string()),
    }
}

/// `ctx.warn()` / `info()` / `debug()`.
pub fn ctx_log(ctx: &HookCtx, level: &str, message: String) {
    let log = rolldown::plugin::LogWithoutPlugin {
        message,
        ..Default::default()
    };
    match level {
        "info" => ctx.plugin().info(log),
        "debug" => ctx.plugin().debug(log),
        _ => ctx.plugin().warn(log),
    }
}
