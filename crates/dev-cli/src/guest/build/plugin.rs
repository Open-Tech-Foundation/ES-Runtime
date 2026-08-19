//! The isolate boundary: a [`Pass`](crate::contract::Pass) whose hooks run in
//! guest JavaScript.
//!
//! [`GuestPass`] is one of the two implementations of the contract, and the
//! hard one. The other — this toolchain's CSS Modules pass — answers a hook by
//! returning; this one answers by posting a message to another thread and
//! waiting. Everything downstream, [`Adapter`](crate::adapter::Adapter)
//! included, is unable to tell which it has.
//!
//! No rolldown *type* appears here — the backend is named only in
//! [`crate::adapter`](crate::adapter), which is where the contract stops. What
//! it does to the design is described below, because the shape of this file is
//! a consequence of it.
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
//! [`Filter`](crate::contract::Filter) is evaluated on *this* side, before the
//! call is posted, which is why it is declarative: a predicate the guest owns
//! could only be consulted by crossing.
//!
//! # `this`
//!
//! A hook's context is the other half of the API, and the half that cannot be
//! a message: `ctx.resolve()` asks the *bundler's own resolver* a question
//! mid-hook. So each in-flight call parks its
//! [`Context`](crate::contract::Context) beside its reply channel, and the
//! context ops reach it by call id — for exactly as long as that hook is
//! running. The bridge holds it as a contract object rather than a backend one,
//! which is what keeps this file free of the backend.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::contract::{self, Hook};
use anyhow::anyhow;
use es_runtime_cli_common::Value;
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

/// The queue between rolldown's threads and the isolate's.
pub struct Bridge {
    calls: mpsc::UnboundedSender<HookCall>,
    state: std::sync::Mutex<State>,
    next_id: AtomicU64,
}

#[derive(Default)]
struct State {
    waiting: HashMap<u64, oneshot::Sender<HookReply>>,
    contexts: HashMap<u64, Arc<dyn contract::Context>>,
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
        ctx: Option<Arc<dyn contract::Context>>,
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
    pub fn context(&self, id: u64) -> Option<Arc<dyn contract::Context>> {
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

/// A plugin declared in guest JavaScript, as a [`Pass`].
///
/// Every hook here is the same three steps: contract-shaped arguments across
/// the bridge, wait, contract-shaped answer back. Nothing rolldown-shaped
/// appears — the ids arriving have already had the backend's notation stripped
/// by the [`Adapter`], and the answers go back in the contract's own types.
#[derive(Debug)]
pub struct GuestPass {
    bridge: Arc<Bridge>,
    /// Shared rather than cloned: a filter is a compiled regular expression,
    /// and a dev server builds forty times a minute.
    plugin: Arc<contract::Plugin>,
}

impl GuestPass {
    pub fn new(bridge: Arc<Bridge>, plugin: Arc<contract::Plugin>) -> GuestPass {
        GuestPass { bridge, plugin }
    }

    async fn call(
        &self,
        hook: Hook,
        args: Vec<Value>,
        meta: Vec<(String, Value)>,
        ctx: Arc<dyn contract::Context>,
    ) -> Result<Value, String> {
        self.bridge
            .call(self.plugin.id, hook.name(), args, meta, Some(ctx))
            .await
            .map_err(|e| e.to_string())
    }
}

impl contract::Pass for GuestPass {
    fn name(&self) -> &str {
        &self.plugin.name
    }

    fn hooks(&self) -> &contract::Hooks {
        &self.plugin.hooks
    }

    fn start<'a>(
        &'a self,
        ctx: &'a Arc<dyn contract::Context>,
    ) -> contract::Answer<'a, Vec<String>> {
        let ctx = ctx.clone();
        Box::pin(async move {
            let out = self.call(Hook::Start, Vec::new(), Vec::new(), ctx).await?;
            // A build-wide input: a config file or a manifest the whole build
            // was read from, which no module imports.
            Ok(contract::depends_on(&out))
        })
    }

    fn resolve<'a>(
        &'a self,
        specifier: &'a str,
        importer: Option<&'a str>,
        is_entry: bool,
        ctx: &'a Arc<dyn contract::Context>,
    ) -> contract::Answer<'a, contract::Resolved> {
        let ctx = ctx.clone();
        Box::pin(async move {
            let out = self
                .call(
                    Hook::Resolve,
                    vec![
                        Value::String(specifier.to_string()),
                        match importer {
                            Some(importer) => Value::String(importer.to_string()),
                            None => Value::Null,
                        },
                    ],
                    // On the context rather than a third positional argument,
                    // so `(source, importer, ctx)` is the signature and stays
                    // the signature if anything else is ever added.
                    vec![("isEntry".to_string(), Value::Bool(is_entry))],
                    ctx,
                )
                .await?;
            Ok(contract::resolved(&out))
        })
    }

    fn load<'a>(
        &'a self,
        id: &'a str,
        ctx: &'a Arc<dyn contract::Context>,
    ) -> contract::Answer<'a, Option<contract::ModuleResult>> {
        let ctx = ctx.clone();
        Box::pin(async move {
            let out = self
                .call(
                    Hook::Load,
                    vec![Value::String(id.to_string())],
                    Vec::new(),
                    ctx,
                )
                .await?;
            contract::module_result(&out, Hook::Load)
        })
    }

    fn transform<'a>(
        &'a self,
        code: &'a str,
        id: &'a str,
        ctx: &'a Arc<dyn contract::Context>,
    ) -> contract::Answer<'a, Option<contract::ModuleResult>> {
        let ctx = ctx.clone();
        Box::pin(async move {
            let out = self
                .call(
                    Hook::Transform,
                    vec![
                        Value::String(code.to_string()),
                        Value::String(id.to_string()),
                    ],
                    Vec::new(),
                    ctx,
                )
                .await?;
            contract::module_result(&out, Hook::Transform)
        })
    }

    fn bundle<'a>(
        &'a self,
        output: &'a [contract::Output],
        ctx: &'a Arc<dyn contract::Context>,
    ) -> contract::Answer<'a, ()> {
        let ctx = ctx.clone();
        Box::pin(async move {
            let listing = Value::Array(output.iter().map(output_value).collect());
            self.call(Hook::Bundle, vec![listing], Vec::new(), ctx)
                .await?;
            Ok(())
        })
    }

    fn end<'a>(
        &'a self,
        error: Option<&'a str>,
        ctx: &'a Arc<dyn contract::Context>,
    ) -> contract::Answer<'a, ()> {
        let ctx = ctx.clone();
        Box::pin(async move {
            let error = match error {
                Some(error) => Value::String(error.to_string()),
                None => Value::Null,
            };
            self.call(Hook::End, vec![error], Vec::new(), ctx).await?;
            Ok(())
        })
    }
}

/// One produced file, as the guest reads it.
///
/// The same field names the finished build uses, so a `bundle` hook and a
/// consumer of `generate()` are reading one shape rather than two — minus
/// `code`, which this hook is deliberately not given.
fn output_value(output: &contract::Output) -> Value {
    let strings =
        |items: &[String]| Value::Array(items.iter().map(|s| Value::String(s.clone())).collect());
    match output {
        contract::Output::Chunk {
            file_name,
            name,
            is_entry,
            is_dynamic_entry,
            facade_module_id,
            module_ids,
            imports,
            dynamic_imports,
        } => Value::Object(vec![
            ("type".to_string(), Value::String("chunk".to_string())),
            ("fileName".to_string(), Value::String(file_name.clone())),
            ("name".to_string(), Value::String(name.clone())),
            ("isEntry".to_string(), Value::Bool(*is_entry)),
            ("isDynamicEntry".to_string(), Value::Bool(*is_dynamic_entry)),
            (
                "facadeModuleId".to_string(),
                facade_module_id.clone().map_or(Value::Null, Value::String),
            ),
            ("moduleIds".to_string(), strings(module_ids)),
            ("imports".to_string(), strings(imports)),
            ("dynamicImports".to_string(), strings(dynamic_imports)),
        ]),
        contract::Output::Asset { file_name } => Value::Object(vec![
            ("type".to_string(), Value::String("asset".to_string())),
            ("fileName".to_string(), Value::String(file_name.clone())),
        ]),
    }
}
