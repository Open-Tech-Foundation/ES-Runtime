//! `runtime:build` — the bundler, callable from guest JavaScript.
//!
//! **`esdev` only.** rolldown is already linked into this binary for the
//! `esdev build` subcommand; what was missing is a way for a *program* to reach
//! it. Without that, a framework's dev server has to `import rolldown from
//! "rolldown"` — a napi addon, which this runtime does not load and will not —
//! and so has to be a Node program, which is the thing the framework was trying
//! to stop being.
//!
//! `esrun` does not carry this module. A production binary that could bundle
//! would have to contain a bundler, and a deployment has nothing to bundle.
//!
//! # What is here that a subprocess could not do
//!
//! It would be cheaper to shell out to a bundler and pipe source through it,
//! and that was considered. It cannot work, because `transform` is only one of
//! the hooks real plugins use:
//!
//! * `resolveId` + `load` serve **virtual modules** — a specifier that exists on
//!   no disk, whose content a plugin makes up. A pipe has nothing to pipe.
//! * `this.addWatchFile()` declares a dependency the graph **cannot discover**:
//!   a module built from frontmatter depends on files it never imports, and
//!   without saying so the consumer's invalidation is wrong in the direction
//!   that serves stale output.
//! * `this.resolve()` asks the **bundler's own resolver** a question in the
//!   middle of a hook.
//! * `this.emitFile()` adds an entry to a build that is already running.
//!
//! All four need the plugin and the bundler in the same conversation, which is
//! what [`plugin`] is: real hooks, taking real functions, dispatched into the
//! isolate.
//!
//! # The shape of the API
//!
//! Deliberately rollup's, because that is what every plugin ever written
//! expects and there is nothing to gain from a fourth spelling:
//!
//! ```js
//! const bundle = await build({ input: "app/main.js", plugins: [mdx()] });
//! const { output, watchFiles } = await bundle.generate({ format: "esm" });
//! ```
//!
//! # Capabilities
//!
//! `FileRead` to build, `FileWrite` as well to `write()`. The `cwd` a build
//! runs in is resolved through the run's own filesystem view, so a build cannot
//! be *started* somewhere `--allow-read` does not reach. What it reads from
//! there is read by rolldown itself, with this process's authority rather than
//! through the jail — a module graph's extent is not knowable up front, and
//! pretending otherwise would be a check that looks like a boundary without
//! being one. Stated rather than implied.

pub mod plugin;
pub mod server;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use es_runtime_cli_common::{
    AsyncOp, ExtensionContext, FileSystem, HostExtension, HostModule, OpDecl, OpError, Value,
};
use es_runtime_common::{Capability, ExceptionClass, IntoException};
use rolldown::plugin::HookUsage;
use tokio::sync::mpsc;

use plugin::{Bridge, HookCall, HookReply};
use server::{BuildServer, External, Options, OutputOptions, PluginSpec};

/// The `runtime:build` extension.
pub struct BuildExtension;

const MODULES: &[HostModule] = &[HostModule {
    specifier: "runtime:build",
    source: include_str!("build.js"),
}];

/// Everything the ops share for one run.
struct BuildState {
    bridge: Arc<Bridge>,
    /// The queue of hook calls waiting for the guest's pump. Polled in place
    /// rather than taken, so an abandoned `build_hook()` promise cannot lose it.
    hooks: Rc<RefCell<mpsc::UnboundedReceiver<HookCall>>>,
    /// Started on the first `build()`, then kept: a dev server builds
    /// continuously, and a thread and a runtime per rebuild is a cost paid
    /// forty times a minute for nothing.
    server: Rc<RefCell<Option<Arc<BuildServer>>>>,
    file_system: Arc<dyn FileSystem>,
    base_dir: std::path::PathBuf,
}

impl HostExtension for BuildExtension {
    fn modules(&self) -> &[HostModule] {
        MODULES
    }

    fn ops(&self, ctx: &ExtensionContext<'_>) -> Vec<OpDecl> {
        let (bridge, hooks) = Bridge::new();
        let state = Rc::new(BuildState {
            bridge,
            hooks: Rc::new(RefCell::new(hooks)),
            server: Rc::new(RefCell::new(None)),
            file_system: ctx.file_system.clone(),
            base_dir: ctx.base_dir.to_path_buf(),
        });

        let mut ops = Vec::new();

        // create(options) -> id
        let this = state.clone();
        ops.push(
            OpDecl::r#async("build_create", move |args| -> AsyncOp {
                let this = this.clone();
                let options = args.into_iter().next().unwrap_or(Value::Undefined);
                Box::pin(async move {
                    let options = parse_options(&this, options).await?;
                    let server = this.server()?;
                    server
                        .create(options)
                        .await
                        .map(id_value)
                        .map_err(build_error)
                })
            })
            .requires(Capability::FileRead),
        );

        // generate(id, output) -> { output, watchFiles, warnings }
        let this = state.clone();
        ops.push(
            OpDecl::r#async("build_generate", move |args| -> AsyncOp {
                let this = this.clone();
                let id = arg_id(&args, 0);
                let output = output_options(args.get(1));
                Box::pin(async move {
                    let server = this.server()?;
                    let built = server
                        .generate(id, false, output)
                        .await
                        .map_err(build_error)?;
                    Ok(built_value(built))
                })
            })
            .requires(Capability::FileRead),
        );

        // write(id, output) — the same build, landed on disk.
        let this = state.clone();
        ops.push(
            OpDecl::r#async("build_write", move |args| -> AsyncOp {
                let this = this.clone();
                let id = arg_id(&args, 0);
                let output = output_options(args.get(1));
                Box::pin(async move {
                    let server = this.server()?;
                    let built = server
                        .generate(id, true, output)
                        .await
                        .map_err(build_error)?;
                    Ok(built_value(built))
                })
            })
            .requires(Capability::FileRead)
            .requires(Capability::FileWrite),
        );

        // close(id)
        let this = state.clone();
        ops.push(OpDecl::sync("build_close", move |args| {
            let id = arg_id(&args, 0);
            if let Some(server) = this.server.borrow().as_ref() {
                server.close(id);
            }
            Ok(Value::Undefined)
        }));

        // The pump: the next hook call for the guest to run.
        //
        // `unref`, because a pending one is not a reason to keep the program
        // alive: only a build this agent started can ever produce a hook call,
        // so with the loop otherwise idle there is provably nothing left to
        // answer. Without it, a program that bundled once would never exit.
        let this = state.clone();
        ops.push(
            OpDecl::r#async("build_hook", move |_args| -> AsyncOp {
                let hooks = this.hooks.clone();
                Box::pin(async move {
                    let call = NextHook { hooks }.await;
                    Ok(match call {
                        Some(call) => Value::Object(vec![
                            ("id".to_string(), Value::Number(call.id as f64)),
                            ("plugin".to_string(), Value::Number(call.plugin)),
                            ("hook".to_string(), Value::String(call.hook.to_string())),
                            ("args".to_string(), Value::Array(call.args)),
                        ]),
                        None => Value::Null,
                    })
                })
            })
            .unref(),
        );

        // reply(callId, ok, value)
        let this = state.clone();
        ops.push(OpDecl::sync("build_hook_reply", move |args| {
            let id = arg_id(&args, 0);
            let ok = matches!(args.get(1), Some(Value::Bool(true)));
            let value = args.into_iter().nth(2).unwrap_or(Value::Undefined);
            this.bridge.reply(
                id,
                if ok {
                    HookReply::Returned(value)
                } else {
                    HookReply::Threw(match value {
                        Value::String(message) => message,
                        other => format!("{other:?}"),
                    })
                },
            );
            Ok(Value::Undefined)
        }));

        // this.resolve(specifier, importer, options)
        let this = state.clone();
        ops.push(OpDecl::r#async("build_resolve", move |args| -> AsyncOp {
            let this = this.clone();
            let call = arg_id(&args, 0);
            let specifier = arg_str(&args, 1);
            let importer = args.get(2).and_then(Value::as_str).map(str::to_string);
            let skip_self = matches!(
                args.get(3).and_then(|v| field(v, "skipSelf")),
                Some(Value::Bool(true))
            );
            Box::pin(async move {
                let Some(ctx) = this.bridge.context(call) else {
                    return Err(context_expired());
                };
                let options = rolldown::plugin::PluginContextResolveOptions {
                    skip_self,
                    ..rolldown::plugin::PluginContextResolveOptions::default()
                };
                match ctx
                    .plugin()
                    .resolve(&specifier, importer.as_deref(), Some(options))
                    .await
                {
                    Err(err) => Err(build_error(err.to_string())),
                    // Unresolvable is `null`, not a throw — the same answer
                    // rollup gives, and the one a plugin branches on.
                    Ok(Err(_)) => Ok(Value::Null),
                    Ok(Ok(resolved)) => Ok(Value::Object(vec![
                        ("id".to_string(), Value::String(resolved.id.to_string())),
                        (
                            "external".to_string(),
                            Value::Bool(!matches!(
                                resolved.external,
                                rolldown_common::ResolvedExternal::Bool(false)
                            )),
                        ),
                    ])),
                }
            })
        }));

        // this.addWatchFile(file)
        let this = state.clone();
        ops.push(OpDecl::sync("build_watch_file", move |args| {
            let call = arg_id(&args, 0);
            let file = arg_str(&args, 1);
            let Some(ctx) = this.bridge.context(call) else {
                return Err(context_expired());
            };
            ctx.add_watch_file(&file);
            Ok(Value::Undefined)
        }));

        // this.emitFile({ type, name, fileName, source | id })
        let this = state.clone();
        ops.push(OpDecl::sync("build_emit", move |args| {
            let call = arg_id(&args, 0);
            let file = args.get(1).cloned().unwrap_or(Value::Undefined);
            let Some(ctx) = this.bridge.context(call) else {
                return Err(context_expired());
            };
            emit(ctx.plugin(), &file).map(Value::String)
        }));

        // this.warn(message) / this.info(message) / this.debug(message)
        let this = state;
        ops.push(OpDecl::sync("build_log", move |args| {
            let call = arg_id(&args, 0);
            let level = arg_str(&args, 1);
            let message = arg_str(&args, 2);
            let Some(ctx) = this.bridge.context(call) else {
                return Err(context_expired());
            };
            let log = rolldown::plugin::LogWithoutPlugin {
                message,
                ..Default::default()
            };
            match level.as_str() {
                "info" => ctx.plugin().info(log),
                "debug" => ctx.plugin().debug(log),
                _ => ctx.plugin().warn(log),
            }
            Ok(Value::Undefined)
        }));

        ops
    }
}

impl BuildState {
    /// The bundler thread, started on first use.
    fn server(&self) -> Result<Arc<BuildServer>, OpError> {
        if let Some(server) = self.server.borrow().as_ref() {
            return Ok(server.clone());
        }
        let server = Arc::new(BuildServer::start(self.bridge.clone()).map_err(build_error)?);
        *self.server.borrow_mut() = Some(server.clone());
        Ok(server)
    }
}

/// The pending half of the pump.
struct NextHook {
    hooks: Rc<RefCell<mpsc::UnboundedReceiver<HookCall>>>,
}

impl std::future::Future for NextHook {
    type Output = Option<HookCall>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.hooks.borrow_mut().poll_recv(cx)
    }
}

/// Reads the guest's `build()` options.
async fn parse_options(state: &BuildState, options: Value) -> Result<Options, OpError> {
    let get = |name: &str| field(&options, name).cloned();

    // The one path the run's own filesystem view judges: where the build
    // starts. Defaults to the entry module's directory, so a relative `input`
    // means what it means everywhere else in a run.
    let cwd = match get("cwd").as_ref().and_then(Value::as_str) {
        Some(cwd) => state
            .file_system
            .real_path(cwd.to_string())
            .await
            .map_err(|e| {
                OpError::new(e.exception_class(), e.exception_message()).with_code_opt(e.code())
            })?
            .into(),
        None => state.base_dir.clone(),
    };

    let input = match get("input") {
        Some(Value::String(one)) => vec![(None, one)],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(import) => Some((None, import.clone())),
                Value::Object(_) => {
                    let import = field(item, "import")?.as_str()?.to_string();
                    let name = field(item, "name")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    Some((name, import))
                }
                _ => None,
            })
            .collect(),
        Some(Value::Object(entries)) => entries
            .iter()
            .filter_map(|(name, import)| Some((Some(name.clone()), import.as_str()?.to_string())))
            .collect(),
        _ => Vec::new(),
    };
    if input.is_empty() {
        return Err(OpError::type_error("build: input is required"));
    }

    let external = match get("external") {
        Some(Value::Array(items)) => Some(External::List(
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
        )),
        Some(Value::String(one)) => Some(External::List(vec![one])),
        // A function crossed as the handle the guest registered it under.
        Some(Value::Number(id)) => Some(External::Predicate(id)),
        _ => None,
    };

    let resolve = get("resolve");
    let resolve_field = |name: &str| resolve.as_ref().and_then(|r| field(r, name)).cloned();

    Ok(Options {
        cwd: Some(cwd),
        input,
        external,
        platform: get("platform")
            .as_ref()
            .and_then(Value::as_str)
            .map(str::to_string),
        conditions: string_list(resolve_field("conditionNames").as_ref()),
        main_fields: string_list(resolve_field("mainFields").as_ref()),
        alias: alias_list(resolve_field("alias").as_ref()),
        extensions: string_list(resolve_field("extensions").as_ref()),
        define: pairs(get("define").as_ref()),
        plugins: plugin_specs(get("plugins").as_ref()),
        minify: matches!(get("minify"), Some(Value::Bool(true))),
        treeshake: match get("treeshake") {
            Some(Value::Bool(on)) => Some(on),
            _ => None,
        },
        output: output_options(Some(&options)),
    })
}

/// Reads the output half of the options, from `build()` or from
/// `generate()`/`write()`.
fn output_options(value: Option<&Value>) -> OutputOptions {
    let Some(value) = value else {
        return OutputOptions::default();
    };
    let text = |name: &str| {
        field(value, name)
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    OutputOptions {
        format: text("format"),
        dir: text("dir"),
        file: text("file"),
        entry_filenames: text("entryFileNames"),
        chunk_filenames: text("chunkFileNames"),
        asset_filenames: text("assetFileNames"),
        code_splitting: match field(value, "codeSplitting") {
            Some(Value::Bool(on)) => Some(*on),
            _ => None,
        },
        sourcemap: match field(value, "sourcemap") {
            Some(Value::Bool(true)) => Some("external".to_string()),
            Some(Value::String(kind)) => Some(kind.clone()),
            _ => None,
        },
        banner: text("banner"),
        footer: text("footer"),
    }
}

/// Reads the plugin list: each entry is the handle the guest registered the
/// object under, plus which hooks it actually has.
fn plugin_specs(value: Option<&Value>) -> Vec<PluginSpec> {
    let Some(Value::Array(items)) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let id = field(item, "id")?.as_number()?;
            let name = field(item, "name")
                .and_then(Value::as_str)
                .unwrap_or("plugin")
                .to_string();
            let mut usage = HookUsage::empty();
            for hook in string_list(field(item, "hooks")) {
                usage |= match hook.as_str() {
                    "buildStart" => HookUsage::BuildStart,
                    "resolveId" => HookUsage::ResolveId,
                    "load" => HookUsage::Load,
                    "transform" => HookUsage::Transform,
                    "buildEnd" => HookUsage::BuildEnd,
                    // A hook this bridge does not carry is ignored rather than
                    // refused: the plugin still works, minus that hook, and
                    // saying so belongs in the docs rather than in a throw at
                    // the start of every build.
                    _ => HookUsage::empty(),
                };
            }
            Some(PluginSpec { id, name, usage })
        })
        .collect()
}

/// `this.emitFile({ type, ... })`.
fn emit(ctx: &rolldown::plugin::PluginContext, file: &Value) -> Result<String, OpError> {
    let kind = field(file, "type")
        .and_then(Value::as_str)
        .unwrap_or("asset");
    let name = field(file, "name")
        .and_then(Value::as_str)
        .map(str::to_string);
    let file_name = field(file, "fileName")
        .and_then(Value::as_str)
        .map(str::to_string);
    match kind {
        "chunk" => {
            let id = field(file, "id")
                .and_then(Value::as_str)
                .ok_or_else(|| OpError::type_error("emitFile: a chunk needs an id"))?
                .to_string();
            ctx.emit_chunk(rolldown_common::EmittedChunk {
                name: name.map(Into::into),
                file_name: file_name.map(Into::into),
                id,
                importer: None,
                preserve_entry_signatures: None,
            })
            .map(|reference| reference.to_string())
            .map_err(|e| build_error(e.to_string()))
        }
        _ => {
            let source = match field(file, "source") {
                Some(Value::Bytes(bytes)) => rolldown_common::StrOrBytes::Bytes(bytes.clone()),
                Some(Value::String(text)) => rolldown_common::StrOrBytes::Str(text.clone()),
                _ => return Err(OpError::type_error("emitFile: an asset needs a source")),
            };
            ctx.emit_file(
                rolldown_common::EmittedAsset {
                    name,
                    original_file_name: None,
                    file_name: file_name.map(Into::into),
                    source,
                },
                None,
                None,
            )
            .map(|reference| reference.to_string())
            .map_err(|e| build_error(e.to_string()))
        }
    }
}

/// A finished build, in the shape rollup's `generate()` returns.
fn built_value(built: server::Built) -> Value {
    let mut output = Vec::with_capacity(built.chunks.len() + built.assets.len());
    for chunk in built.chunks {
        output.push(Value::Object(vec![
            ("type".to_string(), Value::String("chunk".to_string())),
            ("fileName".to_string(), Value::String(chunk.file_name)),
            ("name".to_string(), Value::String(chunk.name)),
            ("code".to_string(), Value::String(chunk.code)),
            ("isEntry".to_string(), Value::Bool(chunk.is_entry)),
            (
                "isDynamicEntry".to_string(),
                Value::Bool(chunk.is_dynamic_entry),
            ),
            ("moduleIds".to_string(), strings(chunk.module_ids)),
            ("imports".to_string(), strings(chunk.imports)),
            ("dynamicImports".to_string(), strings(chunk.dynamic_imports)),
            (
                "map".to_string(),
                chunk.map.map_or(Value::Null, Value::String),
            ),
        ]));
    }
    for asset in built.assets {
        output.push(Value::Object(vec![
            ("type".to_string(), Value::String("asset".to_string())),
            ("fileName".to_string(), Value::String(asset.file_name)),
            (
                "source".to_string(),
                match asset.source {
                    server::AssetSource::Text(text) => Value::String(text),
                    server::AssetSource::Bytes(bytes) => Value::Bytes(bytes),
                },
            ),
        ]));
    }
    Value::Object(vec![
        ("output".to_string(), Value::Array(output)),
        ("watchFiles".to_string(), strings(built.watch_files)),
        ("warnings".to_string(), strings(built.warnings)),
    ])
}

fn strings(items: Vec<String>) -> Value {
    Value::Array(items.into_iter().map(Value::String).collect())
}

fn id_value(id: u64) -> Value {
    Value::Number(id as f64)
}

fn build_error(message: String) -> OpError {
    OpError::new(ExceptionClass::Error, message)
}

fn context_expired() -> OpError {
    OpError::new(
        ExceptionClass::Error,
        "this build hook has already returned — its context is only usable while it runs",
    )
}

// --- reading structured values from the guest -------------------------------

fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    match value {
        Value::Object(fields) => fields.iter().find(|(k, _)| k == name).map(|(_, v)| v),
        _ => None,
    }
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Some(Value::String(one)) => vec![one.clone()],
        _ => Vec::new(),
    }
}

/// An object of strings, as ordered pairs: `define`, and anything like it.
fn pairs(value: Option<&Value>) -> Vec<(String, String)> {
    match value {
        Some(Value::Object(fields)) => fields
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
            .collect(),
        _ => Vec::new(),
    }
}

/// `alias`, in either spelling: `{ find: replacement }` or
/// `[{ find, replacement }]` with a list of replacements to try in order.
fn alias_list(value: Option<&Value>) -> Vec<(String, Vec<String>)> {
    match value {
        Some(Value::Object(fields)) => fields
            .iter()
            .filter_map(|(find, to)| match to {
                Value::String(one) => Some((find.clone(), vec![one.clone()])),
                Value::Array(_) => Some((find.clone(), string_list(Some(to)))),
                _ => None,
            })
            .collect(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| {
                let find = field(item, "find")?.as_str()?.to_string();
                let to = field(item, "replacement")?;
                Some(match to {
                    Value::String(one) => (find, vec![one.clone()]),
                    _ => (find, string_list(Some(to))),
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn arg_str(args: &[Value], i: usize) -> String {
    args.get(i)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn arg_id(args: &[Value], i: usize) -> u64 {
    args.get(i).and_then(Value::as_number).unwrap_or(0.0) as u64
}
