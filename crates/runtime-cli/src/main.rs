//! `esrun` — a standalone CLI that runs JavaScript on the ES-Runtime.
//!
//! This is the thin executable wrapper around the embeddable `runtime` library:
//! it wires the default tokio providers (system clock, OS entropy, reqwest
//! networking, a stdout/stderr console), constructs a [`Runtime`], loads the
//! given source as an **ES module**, and drives it to completion on the
//! [`Driver`]. The runtime itself owns no loop and no I/O — everything
//! host-facing is injected here, so this file *is* the standalone embedding
//! (SPEC.md §8).
//!
//! Every input runs as an ES module: `import`/`export` and top-level `await`
//! work. Imports resolve via [`NodeModuleLoader`]: relative/absolute paths and
//! `file:` URLs as local files, and bare specifiers through `node_modules`
//! (ES module packages only — CommonJS packages and `node:` builtins are
//! rejected; nothing is installed).
//!
//! ```text
//! esrun script.mjs           # run a module file
//! esrun -e "console.log(1)"  # run an inline module snippet
//! esrun --version | --help
//! ```

// A CLI's whole job is to talk to the terminal.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use es_runtime::{HostProviders, ModuleEvalState, ModuleLoader, Process, Runtime};
use es_runtime_common::CapabilitySet;
use es_runtime_default_providers::Driver;
use es_runtime_default_providers::{
    NodeModuleLoader, OsEntropy, ReqwestTransport, SystemClock, SystemProcess, TokioTimers, path,
};
use es_runtime_providers::{Console, ConsoleLevel};
use url::Url;

const USAGE: &str = "\
esrun — run JavaScript (ES modules) on the ES-Runtime

USAGE:
    esrun <file>             Run a JavaScript module file
    esrun -e <code>          Run an inline module snippet
    esrun -t, --timeout <ms> Stop execution after <ms> ms (watchdog, SPEC §4)
    esrun --help             Show this help
    esrun --version          Show the version

Inputs run as ES modules: import/export and top-level await work. Imports
resolve as local files (relative/absolute paths or file: URLs) and as bare
specifiers through node_modules (ES module packages only — CommonJS packages
and node: builtins are rejected; nothing is installed). Static and dynamic
import() both work; import attributes and remote modules are not supported yet.
The full WinterTC surface is available (console, URL, fetch, crypto, streams,
encoding, timers, events).
All host capabilities are granted.";

/// The V8 startup snapshot with the prelude baked in, built by build.rs.
static SNAPSHOT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/prelude.snapshot.bin"));

/// A console that prints to the process's stdout/stderr, like Node/Deno.
struct StdoutConsole;

impl Console for StdoutConsole {
    fn write(&self, level: ConsoleLevel, message: &str) {
        match level {
            ConsoleLevel::Warn | ConsoleLevel::Error => eprintln!("{message}"),
            _ => println!("{message}"),
        }
    }
}

/// What to run, parsed from argv.
enum Source {
    File(String),
    Inline(String),
}

/// Parsed command line.
struct Config {
    source: Source,
    timeout: Option<Duration>,
    /// User arguments after the script/`-e` code, exposed as `runtime:process`
    /// `args` (the runtime binary and the script/code are excluded).
    args: Vec<String>,
}

fn parse_args() -> Result<Config, String> {
    let mut timeout = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("esrun {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-t" | "--timeout" => {
                let ms = args
                    .next()
                    .ok_or_else(|| "-t/--timeout requires a millisecond value".to_string())?;
                let ms: u64 = ms.parse().map_err(|_| {
                    format!("invalid --timeout value: {ms} (expected milliseconds)")
                })?;
                timeout = Some(Duration::from_millis(ms));
            }
            "-e" | "--eval" => {
                let code = args
                    .next()
                    .ok_or_else(|| "-e/--eval requires a code argument".to_string())?;
                return Ok(Config {
                    source: Source::Inline(code),
                    timeout,
                    args: args.collect(),
                });
            }
            flag if flag.starts_with('-') => {
                return Err(format!("unknown option: {flag}\n\n{USAGE}"));
            }
            path => {
                return Ok(Config {
                    source: Source::File(path.to_string()),
                    timeout,
                    args: args.collect(),
                });
            }
        }
    }
    Err(format!("missing script argument\n\n{USAGE}"))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("esrun: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let config = parse_args()?;
    // The module's canonical specifier (a file: URL — also import.meta.url and
    // the referrer its imports resolve against), its source, and a short label
    // for diagnostics.
    // Returns the module's canonical specifier (a file: URL — also
    // import.meta.url and the referrer its imports resolve against), its source,
    // a short diagnostic label, and the **base directory** (the entry's own
    // directory, or cwd for `-e`) from which the loader detects the sandbox root.
    let (specifier, source, label, base_dir) = match config.source {
        Source::File(path) => {
            let code =
                std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
            // Canonicalize the entry path (resolving relative components and
            // symlinks, and normalizing the Windows verbatim prefix) into a
            // file: URL via the shared cross-OS path layer (D25). This is a
            // filesystem path, not a module specifier, so it bypasses the
            // loader's specifier rules.
            let abs =
                path::canonicalize(&path).map_err(|e| format!("cannot resolve {path}: {e}"))?;
            let dir = abs
                .parent()
                .map(std::path::Path::to_path_buf)
                .ok_or_else(|| format!("entry path has no parent directory: {path}"))?;
            let url = path::to_file_url(&abs).map_err(|e| e.to_string())?;
            (url, code, path, dir)
        }
        Source::Inline(code) => {
            // A synthetic file: id in the working directory, so the snippet's
            // relative imports resolve against the cwd.
            let cwd = std::env::current_dir()
                .map_err(|e| format!("cannot read working directory: {e}"))?;
            let base = Url::from_directory_path(&cwd)
                .map_err(|()| "working directory is not absolute".to_string())?;
            let url = base
                .join("[eval]")
                .map_err(|e| format!("cannot derive eval specifier: {e}"))?;
            (url.to_string(), code, "<eval>".to_string(), cwd)
        }
    };

    // Default providers — the standalone embedding's host surface.
    let clock = Arc::new(SystemClock::new());
    let timers = Arc::new(TokioTimers);
    let net = Arc::new(ReqwestTransport::new().map_err(|e| format!("http transport: {e}"))?);
    // Host process view for runtime:process (env/cwd/platform from the OS; args
    // are the user's, after the script/-e). A concrete handle is kept to read
    // the exit code a guest `process.exit()` may request.
    let process = Arc::new(SystemProcess::new(config.args));
    let providers = HostProviders::new(
        clock.clone(),
        Arc::new(StdoutConsole),
        net,
        Arc::new(OsEntropy),
    )
    .with_process(process.clone());
    // Module loader: relative/absolute/file: specifiers resolve as local files,
    // bare specifiers through node_modules (ESM packages only). Based at the
    // entry's directory, from which it detects the sandbox root (the project
    // root containing node_modules/package.json) — resolution is jailed under it
    // by default (D25). Held behind an Arc so dynamic import() can reach it.
    let loader: Arc<dyn ModuleLoader> = Arc::new(
        NodeModuleLoader::with_base_dir(&base_dir).map_err(|e| format!("module loader: {e}"))?,
    );

    // Restore the prelude from the snapshot baked in at build time (build.rs)
    // instead of compiling + evaluating it — the bulk of construction cost.
    let mut runtime =
        Runtime::with_snapshot(SNAPSHOT.to_vec(), providers).map_err(|e| e.to_string())?;
    // A trusted local script: grant the full capability set (incl. FileSystem,
    // which module loading requires).
    runtime.set_capabilities(CapabilitySet::all());

    // Execution-time watchdog (SPEC §4): a separate thread terminates the engine
    // after the deadline. Cross-thread V8 termination means even a synchronous
    // infinite loop in a module's top level is stopped. `timed_out` lets us
    // report a timeout distinctly from an ordinary error.
    let timed_out = Arc::new(AtomicBool::new(false));
    if let Some(deadline) = config.timeout {
        let handle = runtime.interrupt_handle();
        let flag = timed_out.clone();
        std::thread::spawn(move || {
            std::thread::sleep(deadline);
            flag.store(true, Ordering::SeqCst);
            handle.terminate();
        });
    }

    // Load the module graph (resolving + reading any imports) and begin
    // evaluating it. Top-level await is native to modules, so no wrapper is
    // needed. A compile/instantiation error or a missing import surfaces here;
    // a top-level throw rejects the evaluation, observed after the drive below.
    let load = runtime.load_module_source(&specifier, &source, loader);
    let loaded = match config.timeout {
        Some(deadline) => match tokio::time::timeout(deadline, load).await {
            Ok(result) => result,
            Err(_) => {
                runtime.interrupt_handle().terminate();
                return Err(timeout_message(config.timeout));
            }
        },
        None => load.await,
    };
    // A guest `process.exit(code)` during the synchronous top level halts the
    // load via the interrupt; exit with that code (not as an error).
    if let Some(code) = process.requested_exit_code() {
        std::process::exit(code);
    }
    if let Err(err) = loaded {
        if timed_out.load(Ordering::SeqCst) {
            return Err(timeout_message(config.timeout));
        }
        return Err(format!("{label}: {err}"));
    }

    // Drive async work (top-level await, fetch, setTimeout, promise reactions)
    // to quiescence. The timeout is a backstop for runaways that live in async
    // callbacks, which yield to the executor (where a blocking watchdog can't
    // preempt them).
    let driver = Driver::new(clock, timers);
    let drive = driver.run_to_completion(&mut runtime);
    let rejections = match config.timeout {
        Some(deadline) => match tokio::time::timeout(deadline, drive).await {
            Ok(rejections) => rejections,
            Err(_) => {
                runtime.interrupt_handle().terminate();
                return Err(timeout_message(config.timeout));
            }
        },
        None => drive.await,
    };

    // A guest `process.exit(code)` from async code halts the drive via the
    // interrupt; exit with that code rather than reporting the termination.
    if let Some(code) = process.requested_exit_code() {
        std::process::exit(code);
    }

    // A top-level throw (or a rejected top-level await) fails the module's
    // evaluation. Report it as the primary error — its rejection also shows up
    // in `rejections`, so it is the one uncaught-rejection we don't re-report.
    if let ModuleEvalState::Failed(message) = runtime.module_eval_state() {
        eprintln!("Uncaught: {message}");
        return Err(format!("{label}: module evaluation failed"));
    }

    if !rejections.is_empty() {
        for message in &rejections {
            eprintln!("Uncaught (in promise): {message}");
        }
        return Err(format!(
            "{} unhandled promise rejection(s)",
            rejections.len()
        ));
    }
    Ok(())
}

fn timeout_message(timeout: Option<Duration>) -> String {
    match timeout {
        Some(d) => format!("execution timed out after {} ms", d.as_millis()),
        None => "execution timed out".to_string(),
    }
}
