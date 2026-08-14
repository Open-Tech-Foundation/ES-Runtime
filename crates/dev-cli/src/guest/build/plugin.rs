//! The bridge: a rolldown plugin whose hooks are JavaScript functions in the
//! guest isolate.
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
//! # `this`
//!
//! A hook's context is the other half of the API, and the half that cannot be
//! a message: `this.resolve()` asks the *bundler's own resolver* a question
//! mid-hook, and `this.addWatchFile()` declares a dependency the graph could
//! not have discovered. Both are why a subprocess protocol was never an option
//! here. [`PluginContext`] is `Arc`-backed and `Send`, so each in-flight call
//! parks its context beside its reply channel, and the context ops reach it by
//! call id — for exactly as long as that hook is running.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::anyhow;
use es_runtime_cli_common::Value;
use rolldown::plugin::{
    HookBuildEndArgs, HookBuildStartArgs, HookLoadArgs, HookLoadReturn, HookNoopReturn,
    HookResolveIdArgs, HookResolveIdOutput, HookResolveIdReturn, HookTransformArgs,
    HookTransformOutput, HookTransformReturn, HookUsage, Plugin, PluginContext,
    SharedLoadPluginContext, SharedTransformPluginContext,
};
use rolldown_common::{ModuleType, ResolvedExternal};
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
    /// Its arguments, in the order the guest's function takes them.
    pub args: Vec<Value>,
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

/// One JS plugin, as rolldown sees it.
#[derive(Debug)]
pub struct JsPlugin {
    bridge: Arc<Bridge>,
    /// The guest's own handle for the plugin object.
    id: f64,
    name: String,
    /// Which hooks the guest actually implements. Declaring this is not an
    /// optimisation detail: a plugin that lists `transform` it does not have
    /// puts every module in the graph through a round trip to the isolate.
    usage: HookUsage,
}

impl JsPlugin {
    pub fn new(bridge: Arc<Bridge>, id: f64, name: String, usage: HookUsage) -> Self {
        JsPlugin {
            bridge,
            id,
            name,
            usage,
        }
    }
}

impl Plugin for JsPlugin {
    fn name(&self) -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Owned(self.name.clone())
    }

    fn register_hook_usage(&self) -> HookUsage {
        self.usage
    }

    async fn build_start(
        &self,
        ctx: &PluginContext,
        _args: &HookBuildStartArgs<'_>,
    ) -> HookNoopReturn {
        self.bridge
            .call(
                self.id,
                "buildStart",
                Vec::new(),
                Some(HookCtx::Plain(ctx.clone())),
            )
            .await?;
        Ok(())
    }

    async fn resolve_id(
        &self,
        ctx: &PluginContext,
        args: &HookResolveIdArgs<'_>,
    ) -> HookResolveIdReturn {
        let out = self
            .bridge
            .call(
                self.id,
                "resolveId",
                vec![
                    Value::String(args.specifier.to_string()),
                    match args.importer {
                        Some(importer) => Value::String(importer.to_string()),
                        None => Value::Null,
                    },
                    Value::Object(vec![("isEntry".to_string(), Value::Bool(args.is_entry))]),
                ],
                Some(HookCtx::Plain(ctx.clone())),
            )
            .await?;
        Ok(resolved(out))
    }

    async fn load(&self, ctx: SharedLoadPluginContext, args: &HookLoadArgs<'_>) -> HookLoadReturn {
        let out = self
            .bridge
            .call(
                self.id,
                "load",
                vec![Value::String(args.id.to_string())],
                Some(HookCtx::Load(ctx)),
            )
            .await?;
        let Some((code, map, module_type)) = code_and_map(out, "load")? else {
            return Ok(None);
        };
        Ok(Some(rolldown::plugin::HookLoadOutput {
            code: code.into(),
            map,
            module_type,
            ..rolldown::plugin::HookLoadOutput::default()
        }))
    }

    async fn transform(
        &self,
        ctx: SharedTransformPluginContext,
        args: &HookTransformArgs<'_>,
    ) -> HookTransformReturn {
        let out = self
            .bridge
            .call(
                self.id,
                "transform",
                vec![
                    Value::String(args.code.to_string()),
                    Value::String(args.id.to_string()),
                ],
                Some(HookCtx::Transform(ctx)),
            )
            .await?;
        let Some((code, map, module_type)) = code_and_map(out, "transform")? else {
            return Ok(None);
        };
        Ok(Some(HookTransformOutput {
            code: Some(code),
            map: match map {
                Some(map) => rolldown::plugin::HookTransformOutputMap::Sourcemap(Box::new(map)),
                // `Omitted`, not `Null`: the plugin said nothing about the map,
                // which leaves the chain alone. `Null` would *break* it.
                None => rolldown::plugin::HookTransformOutputMap::Omitted,
            },
            module_type,
            ..HookTransformOutput::default()
        }))
    }

    async fn build_end(
        &self,
        ctx: &PluginContext,
        args: Option<&HookBuildEndArgs<'_>>,
    ) -> HookNoopReturn {
        // The failure, when there was one: a plugin that started something in
        // `buildStart` has to be told the build died rather than finished.
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
        self.bridge
            .call(
                self.id,
                "buildEnd",
                vec![error],
                Some(HookCtx::Plain(ctx.clone())),
            )
            .await?;
        Ok(())
    }
}

/// A `resolveId` return: `null`, a string, or `{ id, external }`.
fn resolved(out: Value) -> Option<HookResolveIdOutput> {
    match out {
        Value::String(id) => Some(HookResolveIdOutput::from_id(id)),
        Value::Object(fields) => {
            let id = fields
                .iter()
                .find(|(k, _)| k == "id")
                .and_then(|(_, v)| v.as_str())?;
            let external = match fields.iter().find(|(k, _)| k == "external") {
                Some((_, Value::Bool(true))) => Some(ResolvedExternal::Bool(true)),
                Some((_, Value::String(kind))) if kind == "absolute" => {
                    Some(ResolvedExternal::Absolute)
                }
                Some((_, Value::String(kind))) if kind == "relative" => {
                    Some(ResolvedExternal::Relative)
                }
                _ => None,
            };
            Some(HookResolveIdOutput {
                id: id.into(),
                external,
                ..HookResolveIdOutput::from_id(id)
            })
        }
        // `null`, `undefined`, and anything else mean "not mine" — the same
        // answer rollup gives them.
        _ => None,
    }
}

/// A `load`/`transform` return: `null`, a string of code, or
/// `{ code, map, moduleType }`.
type CodeAndMap = Option<(
    String,
    Option<rolldown_sourcemap::SourceMap>,
    Option<ModuleType>,
)>;

fn code_and_map(out: Value, hook: &str) -> anyhow::Result<CodeAndMap> {
    match out {
        Value::String(code) => Ok(Some((code, None, None))),
        Value::Object(fields) => {
            let get = |name: &str| fields.iter().find(|(k, _)| k == name).map(|(_, v)| v);
            let Some(code) = get("code").and_then(Value::as_str) else {
                return Ok(None);
            };
            // A map crosses as JSON, which is what every JS tool that makes one
            // already has: `JSON.stringify(map)` on the way out, parsed here.
            // Converting object-by-object would be the same bytes with more
            // ways to be wrong.
            let map = match get("map").and_then(Value::as_str) {
                Some(json) if !json.is_empty() => Some(
                    rolldown_sourcemap::OwnedSourceMap::from_json_string(json)
                        .map(rolldown_sourcemap::SourceMap::from)
                        .map_err(|e| {
                            anyhow!("{hook} returned a source map that is not valid: {e}")
                        })?,
                ),
                _ => None,
            };
            let module_type = match get("moduleType").and_then(Value::as_str) {
                Some(name) => Some(
                    ModuleType::from_known_str(name)
                        .map_err(|_| anyhow!("{hook} returned an unknown moduleType {name:?}"))?,
                ),
                None => None,
            };
            Ok(Some((code.to_string(), map, module_type)))
        }
        _ => Ok(None),
    }
}
