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
//! # Three layers, and the middle one is the point
//!
//! ```text
//!   build.js + this file  the API and the ops
//!   contract              what a plugin is — ours, versioned with runtime:
//!   plugin.rs, server.rs  the adapter, and the only place rolldown is named
//! ```
//!
//! The bundler is an *implementation* of the contract, not the definition of
//! it. A guest-visible API defined by a third party's trait moves when that
//! trait moves — and a hook renamed in a bundler's patch release would be a
//! breaking change in this runtime's standard library. [`contract`] states what
//! a backend must be able to do; [`plugin`] is what makes rolldown do it.
//!
//! # What a subprocess could not do
//!
//! It would be cheaper to shell out to a bundler and pipe source through it,
//! and that was considered. It cannot work, because `transform` is only one of
//! the hooks real plugins use:
//!
//! * `resolve` + `load` serve **virtual modules** — a specifier that exists on
//!   no disk, whose content a plugin makes up. A pipe has nothing to pipe.
//! * A module built from frontmatter **depends on files it never imports**, and
//!   without saying so the consumer's invalidation is wrong in the direction
//!   that serves stale output.
//! * `ctx.resolve()` asks the **bundler's own resolver** a question in the
//!   middle of a hook.
//! * `ctx.emit()` adds an entry to a build that is already running.
//!
//! All four need the plugin and the bundler in the same conversation.
//!
//! # Capabilities
//!
//! `FileRead` to build, `FileWrite` as well to `write()`. The `cwd` a build
//! runs in is resolved through the run's own filesystem view, so a build cannot
//! be *started* somewhere `--allow-read` does not reach. What it reads from
//! there is read by the bundler itself, with this process's authority rather
//! than through the jail — a module graph's extent is not knowable up front,
//! and pretending otherwise would be a check that looks like a boundary without
//! being one. Stated rather than implied.
//!
//! A **plugin**, on the other hand, is guest code: it runs in this isolate and
//! reaches the host through the same gated ops as any other program. A plugin
//! that reads a file needs `FileRead` like anything else does.

pub mod contract;
pub mod plugin;
pub mod server;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use es_runtime_cli_common::{
    AsyncOp, ExtensionContext, FileSystem, HostExtension, HostModule, OpDecl, OpError, Value,
};
use es_runtime_common::{Capability, ExceptionClass, IntoException};
use tokio::sync::mpsc;

use plugin::{Bridge, HookCall, HookReply};
use server::{BuildServer, External, Options, OutputOptions};

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
                            ("meta".to_string(), Value::Object(call.meta)),
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
                let found = plugin::ctx_resolve(&ctx, &specifier, importer.as_deref(), skip_self)
                    .await
                    .map_err(build_error)?;
                Ok(match found {
                    None => Value::Null,
                    Some(resolved) => Value::Object(vec![
                        ("id".to_string(), Value::String(resolved.id)),
                        ("external".to_string(), Value::Bool(resolved.external)),
                    ]),
                })
            })
        }));

        // this.emitFile({ type, name, fileName, source | id })
        let this = state.clone();
        ops.push(OpDecl::sync("build_emit", move |args| {
            let call = arg_id(&args, 0);
            let file = args.get(1).cloned().unwrap_or(Value::Undefined);
            let Some(ctx) = this.bridge.context(call) else {
                return Err(context_expired());
            };
            let request = contract::emit(&file).map_err(OpError::type_error)?;
            plugin::ctx_emit(&ctx, request)
                .map(Value::String)
                .map_err(build_error)
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
            plugin::ctx_log(&ctx, &level, message);
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
        // A function, crossed as the handle the guest registered it under, and
        // bound here to the same bridge a plugin hook rides. Every specifier
        // the bundler meets becomes a question for the isolate; the answers are
        // not cached, because the predicate is the guest's and a bundler that
        // remembered its answers would be deciding when it stopped asking.
        Some(Value::Number(id)) => {
            let bridge = state.bridge.clone();
            Some(External::Predicate(std::sync::Arc::new(
                move |specifier: &str, importer: Option<&str>, resolved: bool| {
                    let bridge = bridge.clone();
                    let args = vec![
                        Value::String(specifier.to_string()),
                        match importer {
                            Some(importer) => Value::String(importer.to_string()),
                            None => Value::Null,
                        },
                        Value::Bool(resolved),
                    ];
                    Box::pin(async move {
                        let answer = bridge.call(id, "external", args, Vec::new(), None).await?;
                        Ok(matches!(answer, Value::Bool(true)))
                    })
                },
            )))
        }
        _ => None,
    };

    let resolve = get("resolve");
    let resolve_field = |name: &str| resolve.as_ref().and_then(|r| field(r, name)).cloned();

    Ok(Options {
        bundler: crate::bundler::Options {
            cwd: Some(cwd),
            input,
            external,
            // `neutral` is this runtime, and the default for the same reason it
            // is the subcommand's: a program bundling here is bundling for here
            // unless it says otherwise.
            platform: match get("platform").as_ref().and_then(Value::as_str) {
                Some("browser") => crate::resolve::Target::Browser,
                Some("node") => crate::resolve::Target::Node,
                _ => crate::resolve::Target::Server,
            },
            conditions: string_list(resolve_field("conditionNames").as_ref()),
            main_fields: string_list(resolve_field("mainFields").as_ref()),
            alias: alias_list(resolve_field("alias").as_ref()),
            extensions: string_list(resolve_field("extensions").as_ref()),
            define: pairs(get("define").as_ref()),
            minify: matches!(get("minify"), Some(Value::Bool(true))),
            treeshake: match get("treeshake") {
                Some(Value::Bool(on)) => Some(on),
                _ => None,
            },
            preserve_modules: None,
            preserve_modules_root: None,
            output: output_options(Some(&options)),
        },
        plugins: plugins(get("plugins").as_ref())?,
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

/// Reads the plugin list.
///
/// Each entry is the handle the guest registered the object under, its name,
/// and what it declared — parsed by the [contract](contract), which is also
/// where a malformed declaration is refused. A plugin that cannot be read is a
/// build that cannot start, rather than a plugin that quietly does nothing.
fn plugins(value: Option<&Value>) -> Result<Vec<Arc<contract::Plugin>>, OpError> {
    let Some(Value::Array(items)) = value else {
        return Ok(Vec::new());
    };
    items
        .iter()
        .map(|item| contract::plugin(item).map(Arc::new))
        .collect()
}

/// A finished build, as the guest reads it.
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
