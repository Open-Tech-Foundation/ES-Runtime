//! The bundler adapter: [our contract](crate::contract) on one side, rolldown
//! on the other.
//!
//! **Everything that names rolldown in the plugin path lives here.** That is
//! not tidiness — it is what makes the contract a contract. A guest-visible API
//! defined by a third party's trait moves when that trait moves, and the
//! `runtime:` namespace is a versioned promise. Behind this file the bundler is
//! replaceable; in front of it, nothing changes if it is replaced.
//!
//! [`Adapter`] takes any [`Pass`](crate::contract::Pass) and gives back
//! something the bundler will call. It does not care which kind it has: a
//! plugin declared in guest JavaScript and this toolchain's own CSS Modules
//! pass arrive here identically, which is what makes them one list, in one
//! order, under one set of filter rules — and what makes "we could swap the
//! bundler" cover our own passes rather than only other people's.
//!
//! What it translates, in both directions:
//!
//! * **Filters**, matched here rather than inside the pass. For a pass in the
//!   isolate a hook that declines costs a round trip into V8, so an unfiltered
//!   `transform` would be one crossing per module in the graph.
//! * **Order**, from the contract's `pre`/`post` to the backend's.
//! * **Virtual ids.** Rolldown, like every bundler descended from rollup, marks
//!   "there is no file behind this" with a leading NUL byte in the id. The
//!   contract says `virtual: true`; the byte is put on and taken off here,
//!   which is where a backend's private notation belongs.
//! * **Declared dependencies**, which the contract returns and the backend
//!   takes one at a time through the context.
//! * **The context itself** ([`HookCtx`]): `resolve()`, `emit()`, `log()` and
//!   `dependsOn` in the contract's vocabulary, over rolldown's three flavours
//!   of plugin context.

use std::sync::Arc;

use anyhow::anyhow;
use rolldown::plugin::{
    HookBuildEndArgs, HookBuildStartArgs, HookGenerateBundleArgs, HookLoadArgs, HookLoadOutput,
    HookLoadReturn,
    HookNoopReturn, HookResolveIdArgs, HookResolveIdOutput, HookResolveIdReturn, HookTransformArgs,
    HookTransformOutput, HookTransformOutputMap, HookTransformReturn, HookUsage, Plugin,
    PluginContext, PluginHookMeta, PluginOrder, SharedLoadPluginContext,
    SharedTransformPluginContext,
};
use rolldown_common::{ModuleType, ResolvedExternal};

use crate::contract::{self, Hook, Order};

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
}

/// The context ops, from the backend's side.
///
/// This impl is what `ctx.resolve()`, `ctx.emit()` and `ctx.warn()` reach —
/// whether the pass that called them is in the isolate or in this binary. It
/// lives here rather than beside the ops for the reason the whole file exists:
/// this is where the backend is named, and an op that constructed a
/// `rolldown_common::EmittedAsset` would put it back in front of the contract.
impl contract::Context for HookCtx {
    /// The bundler's own resolver, mid-hook.
    fn resolve<'a>(
        &'a self,
        specifier: &'a str,
        importer: Option<&'a str>,
        skip_self: bool,
    ) -> contract::Answer<'a, Option<contract::ResolvedId>> {
        Box::pin(async move {
            let options = rolldown::plugin::PluginContextResolveOptions {
                skip_self,
                ..rolldown::plugin::PluginContextResolveOptions::default()
            };
            match self
                .plugin()
                .resolve(specifier, importer, Some(options))
                .await
            {
                Err(err) => Err(err.to_string()),
                // Unresolvable is `null`, not a throw — the answer a pass
                // branches on, rather than an exception it has to catch to ask
                // a question.
                Ok(Err(_)) => Ok(None),
                Ok(Ok(resolved)) => Ok(Some(contract::ResolvedId {
                    id: resolved.id.to_string(),
                    external: !matches!(resolved.external, ResolvedExternal::Bool(false)),
                })),
            }
        })
    }

    /// An entry or an asset, added to a running build.
    fn emit(&self, emit: contract::Emit) -> Result<String, String> {
        let plugin = self.plugin();
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
                            contract::Source::Bytes(bytes) => {
                                rolldown_common::StrOrBytes::Bytes(bytes)
                            }
                        },
                    },
                    None,
                    None,
                )
                .map(|reference| reference.to_string())
                .map_err(|e| e.to_string()),
        }
    }

    fn log(&self, level: &str, message: String) {
        let log = rolldown::plugin::LogWithoutPlugin {
            message,
            ..Default::default()
        };
        match level {
            "info" => self.plugin().info(log),
            "debug" => self.plugin().debug(log),
            _ => self.plugin().warn(log),
        }
    }

    /// Routed through the *load* context when there is one, because that
    /// variant records the file against the module as well as globally — which
    /// is the difference between "this build read `_meta.js`" and "this module
    /// must be rebuilt when `_meta.js` changes", and fine-grained invalidation
    /// needs the second.
    fn depends_on(&self, file: &str) {
        match self {
            HookCtx::Load(ctx) => ctx.add_watch_file(file),
            other => other.plugin().add_watch_file(file),
        }
    }

    fn cwd(&self) -> std::path::PathBuf {
        self.plugin().cwd().clone()
    }
}

/// One [`Pass`], wearing rolldown's trait.
///
/// The translation is the whole of this type: filters applied, order declared,
/// the backend's private notation put on and taken off, contract-shaped answers
/// turned into rolldown's. It does not care which kind of pass it is carrying —
/// a plugin in the isolate and this toolchain's own CSS pass arrive here
/// identically, which is what makes them one list in one order.
#[derive(Debug)]
pub struct Adapter {
    pass: Arc<dyn contract::Pass>,
    usage: HookUsage,
}

impl Adapter {
    pub fn new(pass: Arc<dyn contract::Pass>) -> Adapter {
        // Declaring the usage is not bookkeeping: a hook rolldown does not know
        // this pass has is a hook it never calls, and one it thinks it has is a
        // crossing per module. Derived from the declaration rather than
        // trusted.
        let hooks = pass.hooks();
        let mut usage = HookUsage::empty();
        if hooks.start.is_some() {
            usage |= HookUsage::BuildStart;
        }
        if hooks.resolve.is_some() {
            usage |= HookUsage::ResolveId;
        }
        if hooks.load.is_some() {
            usage |= HookUsage::Load;
        }
        if hooks.transform.is_some() {
            usage |= HookUsage::Transform;
        }
        if hooks.end.is_some() {
            usage |= HookUsage::BuildEnd;
        }
        if hooks.bundle.is_some() {
            usage |= HookUsage::GenerateBundle;
        }
        Adapter { pass, usage }
    }

    /// Whether this hook wants this module, decided **here** — before anything
    /// crosses into an isolate.
    fn admits(&self, hook: Hook, id: &str, code: Option<&str>) -> bool {
        self.pass
            .hooks()
            .get(hook)
            .is_some_and(|spec| spec.filter.admits(id, code))
    }

    fn order(&self, hook: Hook) -> Option<PluginHookMeta> {
        let order = match self.pass.hooks().get(hook)?.order {
            Order::Pre => PluginOrder::Pre,
            Order::Post => PluginOrder::Post,
            Order::Normal => return None,
        };
        Some(PluginHookMeta { order: Some(order) })
    }
}

impl Plugin for Adapter {
    fn name(&self) -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Owned(self.pass.name().to_string())
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

    fn generate_bundle_meta(&self) -> Option<PluginHookMeta> {
        self.order(Hook::Bundle)
    }

    async fn build_start(
        &self,
        ctx: &PluginContext,
        _args: &HookBuildStartArgs<'_>,
    ) -> HookNoopReturn {
        let ctx: Arc<dyn contract::Context> = Arc::new(HookCtx::Plain(ctx.clone()));
        let files = self.pass.start(&ctx).await.map_err(|e| anyhow!(e))?;
        declare(&ctx, &files);
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
        let ctx: Arc<dyn contract::Context> = Arc::new(HookCtx::Plain(ctx.clone()));
        let out = self
            .pass
            .resolve(args.specifier, args.importer, args.is_entry, &ctx)
            .await
            .map_err(|e| anyhow!(e))?;
        Ok(match out {
            contract::Resolved::Pass => None,
            contract::Resolved::To {
                id,
                external,
                virtual_module,
            } => Some(HookResolveIdOutput {
                // A module the pass invented: rolldown's way of saying "no file
                // behind this" is the `\0` prefix every bundler inherited from
                // rollup. Applied here rather than asked of the plugin author,
                // and stripped again before any id reaches them.
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
        // The id the pass knows it by. A virtual module carries the backend's
        // NUL prefix internally, and a filter written against `"@app/nav"` has
        // to match the module the pass itself named.
        let id = guest_id(args.id);
        if !self.admits(Hook::Load, id, None) {
            return Ok(None);
        }
        let ctx: Arc<dyn contract::Context> = Arc::new(HookCtx::Load(ctx));
        let Some(result) = self.pass.load(id, &ctx).await.map_err(|e| anyhow!(e))? else {
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
        let ctx: Arc<dyn contract::Context> = Arc::new(HookCtx::Transform(ctx));
        let Some(result) = self
            .pass
            .transform(args.code, id, &ctx)
            .await
            .map_err(|e| anyhow!(e))?
        else {
            return Ok(None);
        };
        let result = convert(result, Hook::Transform)?;
        declare(&ctx, &result.depends_on);
        Ok(Some(HookTransformOutput {
            code: Some(result.code),
            map: match result.map {
                Some(map) => HookTransformOutputMap::Sourcemap(Box::new(map)),
                // `Omitted`, not `Null`: the pass said nothing about the map,
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
        // The failure, when there was one: a pass that started something in
        // `start` has to be told the build died rather than finished.
        let error = args.map(|args| {
            args.errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        });
        let ctx: Arc<dyn contract::Context> = Arc::new(HookCtx::Plain(ctx.clone()));
        self.pass
            .end(error.as_deref(), &ctx)
            .await
            .map_err(|e| anyhow!(e))?;
        Ok(())
    }

    async fn generate_bundle(
        &self,
        ctx: &PluginContext,
        args: &mut HookGenerateBundleArgs<'_>,
    ) -> HookNoopReturn {
        let output = produced(args.bundle);
        let ctx: Arc<dyn contract::Context> = Arc::new(HookCtx::Plain(ctx.clone()));
        self.pass
            .bundle(&output, &ctx)
            .await
            .map_err(|e| anyhow!(e))?;
        Ok(())
    }
}

/// What the build produced, in the contract's vocabulary.
///
/// Read from the backend's listing and copied, rather than handed over: the
/// contract's [`bundle`](contract::Pass::bundle) is read-only, and a `&mut Vec`
/// crossing into an isolate could not be anything else anyway.
fn produced(bundle: &[rolldown_common::Output]) -> Vec<contract::Output> {
    bundle
        .iter()
        .map(|output| match output {
            rolldown_common::Output::Chunk(chunk) => contract::Output::Chunk {
                file_name: chunk.filename.to_string(),
                name: chunk.name.to_string(),
                is_entry: chunk.is_entry,
                is_dynamic_entry: chunk.is_dynamic_entry,
                facade_module_id: chunk
                    .facade_module_id
                    .as_ref()
                    .map(|id| guest_id(&id.to_string()).to_string()),
                module_ids: chunk
                    .module_ids
                    .iter()
                    .map(|id| guest_id(&id.to_string()).to_string())
                    .collect(),
                imports: chunk.imports.iter().map(ToString::to_string).collect(),
                dynamic_imports: chunk
                    .dynamic_imports
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
            },
            rolldown_common::Output::Asset(asset) => contract::Output::Asset {
                file_name: asset.filename.to_string(),
            },
        })
        .collect()
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
fn declare(ctx: &Arc<dyn contract::Context>, files: &[String]) {
    if files.is_empty() {
        return;
    }
    let cwd = ctx.cwd();
    for file in files {
        let path = std::path::Path::new(file);
        if path.is_absolute() {
            ctx.depends_on(file);
        } else {
            ctx.depends_on(&cwd.join(path).to_string_lossy());
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
pub fn guest_id(id: &str) -> &str {
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
