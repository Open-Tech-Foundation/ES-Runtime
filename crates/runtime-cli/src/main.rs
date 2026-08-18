//! `esrun` — a standalone CLI that runs JavaScript on the ES-Runtime.
//!
//! This is the thin executable wrapper around the embeddable `runtime` library.
//! The wiring itself — the default tokio providers, the [`Runtime`], the module
//! load and the drive loop — lives in `es-runtime-cli-common` and is shared with
//! `esdev`, so a program behaves identically under either binary (SPEC.md §8).
//! What remains here is `esrun`'s own command line: its flags, and the
//! `upgrade` subcommand it shares with `esdev` (`cli_common::upgrade`). Nothing
//! here is for development — the TypeScript definitions used to be installed
//! from this binary and now belong to `esdev`, which is the one a developer
//! runs (D59).
//!
//! Every input runs as an ES module: `import`/`export` and top-level `await`
//! work. Imports resolve via `NodeModuleLoader`: relative/absolute paths and
//! `file:` URLs as local files, and bare specifiers through `node_modules`
//! (ES module packages only — CommonJS packages and `node:` builtins are
//! rejected; nothing is installed).
//!
//! **Nothing is granted by default** (D65): a run reaches what `--allow-<name>`
//! named on the line that started it, and nothing else. `esdev` is the opposite,
//! because a developer's inner loop is not a deployment.
//!
//! Argument grammar: every flag is `--flag` or `--flag=value` — a value is never
//! a separate argument — and esrun's flags come **before** the script, since
//! everything after it belongs to the script.
//!
//! ```text
//! esrun script.mjs            # run a module file, granted nothing
//! esrun -e='console.log(1)'   # run an inline module snippet
//! esrun --allow-net app.js    # ...and let it reach the network
//! esrun --timeout=500 app.js  # values attach with '='
//! esrun --version | --help
//! ```

// A CLI's whole job is to talk to the terminal.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::process::ExitCode;

use es_runtime_cli_common::args::{
    RunOptions, reject_value, require_value, split_flag_value, try_permission_flag,
};
use es_runtime_cli_common::diagnostics::print_error;
use es_runtime_cli_common::permissions::{Baseline, Permissions};
use es_runtime_cli_common::{Config, Source};

const USAGE: &str = "\
esrun — run JavaScript (ES modules) on the ES-Runtime

Every flag is `--flag` or `--flag=value`. A value is never a separate argument:
`--timeout=500`, not `--timeout 500`. Flags come before the file; everything
after it belongs to the script, readable as `args` from runtime:process.

USAGE:
    esrun [options] <file> [args...]
                                Run a JavaScript module file
    esrun -e=<code>             Run an inline module snippet
    esrun upgrade               Update esrun to the latest release
    esrun -h, --help            Show this help
    esrun -v, --version         Show the version

PERMISSIONS:
    --allow-<name>[=<list>]     Grant one capability, optionally narrowed to a
                                comma-separated list; repeatable. <name> is one
                                of: read, write, imports, net, listen, env, run,
                                signals, workers
    -A, --allow-all             Grant every capability (unsandboxed)
    --deny-<name>               Take one back; requires --allow-all; repeatable
    --deny-all                  Grant nothing — the default, said outright
    --import-policy=<file>      JSON policy for what may be *loaded*, which is a
                                separate question from what running code reaches

OPTIONS:
    --root=<dir>                The project root: where module resolution walks
                                up to and the filesystem is jailed. Default: the
                                nearest package.json/node_modules above the entry
    -t, --timeout=<ms>          Stop execution after <ms> (watchdog, SPEC §4)
    --max-heap=<mb>             Heap ceiling in megabytes, for this agent and as
                                the ceiling its workers inherit. Default: sized
                                from the container's memory limit, or the host's
    --env-file=<path>           Load env vars from a .env file
    --env-override              ...and let them override the OS environment
    --shutdown-grace=<ms>       How long in-flight HTTP requests may finish
                                after ^C/SIGTERM (default 10000)

Nothing is granted by default: a run reaches what the command line that started
it named, and nothing else. Widen it in one of two directions, never both, so
no flag ever overrides another:

    esrun app.js                              # granted nothing
    esrun --allow-net --allow-read app.js     # nothing, plus these
    esrun --allow-net=api.example.com app.js  # ...and only there
    esrun --allow-all --deny-run app.js       # everything, minus these

A scope list narrows a grant: hosts for net/listen, paths for read/write,
programs for run, variable names for env, signal names for signals. Matching is
exact, after canonicalization — example.com does not admit api.example.com, and
there are no wildcards. A denied operation throws NotAllowedError; importing a
runtime: module always works. Ask from JS with `permissions.has(name)` from
runtime:process.

Inputs run as ES modules: import/export, top-level await and import attributes
work. Imports resolve as local files and as bare specifiers through
node_modules (ES module packages only — CommonJS and node: builtins are
rejected, and nothing is installed), from the entry's directory up to the
project root — `--root=<dir>` when that is somewhere else, such as the top of a
workspace. Remote (https://) modules are deliberately
unsupported. The WinterTC surface is there: console, URL, fetch, crypto,
streams, encoding, timers, events.

    Every flag in full:  https://esrun.opentechf.org/api/cli
    The capabilities:    https://esrun.opentechf.org/docs/security
    The modules:         https://esrun.opentechf.org/api
";

/// Parses `esrun`'s command line.
///
/// The shared flags (`--timeout`, `--env-file`, `--max-heap`, the permission
/// vocabulary, …) are handed to `cli-common` so that they mean the same thing
/// here as they do in `esdev`; what is matched below is what only `esrun` has.
fn parse_args() -> Result<Config, String> {
    let mut options = RunOptions::default();
    let mut permissions = Permissions::new(Baseline::Nothing);
    // The flag the previous argument was, so a bare word following it can be
    // diagnosed as an attempted value rather than silently becoming the script.
    let mut previous_flag: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let preceding_flag = previous_flag.take();
        // One grammar for every flag: `--flag` or `--flag=value`. Splitting here,
        // once, is what makes that true — no arm reaches for the next argument.
        let (flag, value) = split_flag_value(&arg);
        if flag.starts_with('-') && flag.len() > 1 {
            previous_flag = Some(flag.to_string());
        }
        // Shared first, so esrun can never disagree with esdev about what a
        // common flag means.
        if options.try_flag(flag, value)? || try_permission_flag(&mut permissions, flag, value)? {
            continue;
        }
        match flag {
            "-h" | "--help" => {
                reject_value(flag, value)?;
                println!("{USAGE}");
                std::process::exit(0);
            }
            // A shipped command that moved. Without this the word is taken for a
            // script path and the answer is "cannot read types", which explains
            // nothing to someone with the old command in their fingers.
            "types" => {
                return Err(
                    "esrun no longer installs TypeScript definitions: they are on npm as \
                     @opentf/esrun-types, and wiring them into a project is development \
                     tooling.\n\n\
                     Use `esdev --install-types`, or add the package yourself:\n  \
                     npm install --save-dev @opentf/esrun-types"
                        .to_string(),
                );
            }
            "upgrade" => {
                reject_value(flag, value)?;
                es_runtime_cli_common::upgrade::run_and_exit("esrun", env!("CARGO_PKG_VERSION"));
            }
            "-v" | "-V" | "--version" => {
                reject_value(flag, value)?;
                println!("esrun {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-e" | "--eval" => {
                let code = require_value(flag, value)?.to_string();
                let rest: Vec<String> = args.collect();
                reject_esrun_flags_after_source(&rest, "the -e code")?;
                return Ok(Config {
                    source: Source::Inline(code),
                    args: rest,
                    capabilities: permissions.resolve()?,
                    scopes: permissions.scopes()?,
                    options,
                    // esrun runs JavaScript. Turning a `.ts` into that is
                    // `esdev`'s job, on a developer's machine.
                    transform: None,
                    // Nothing is added to the `runtime:` namespace here. A
                    // production binary offers the standard modules and only
                    // those, so `runtime:build` and `runtime:watch` — `esdev`'s
                    // — are not merely unwired but absent.
                    extensions: Vec::new(),
                    observer: None,
                    // esrun has no inspector and no flag that could ask for
                    // one: a debugger port would undo every --deny-* the
                    // deployment was started with (D59).
                    inspector: None,
                });
            }
            flag if flag.starts_with('-') && flag.len() > 1 => {
                return Err(format!("unknown option: {flag}\n\n{USAGE}"));
            }
            path => {
                // With one grammar there is nothing to guess: a bare word is the
                // script. But `--deny-net example.com app.js` still *reads* like
                // a value to whoever typed it, and would otherwise run
                // `example.com` as the script with `app.js` as its argument — a
                // "cannot read" three steps from the cause. Say what happened.
                if let Some(flag) = preceding_flag
                    && !std::path::Path::new(path).exists()
                {
                    return Err(format!(
                        "cannot read {path}, and it follows {flag}.\n\n\
                         If {path} was meant as {flag}'s value, attach it with '=' \
                         ({flag}={path}) — this parser never reads a value from the next \
                         argument. Every flag is either `--flag` or `--flag=value`."
                    ));
                }
                let rest: Vec<String> = args.collect();
                reject_esrun_flags_after_source(&rest, path)?;
                return Ok(Config {
                    source: Source::File(path.to_string()),
                    args: rest,
                    capabilities: permissions.resolve()?,
                    scopes: permissions.scopes()?,
                    options,
                    // esrun runs JavaScript. Turning a `.ts` into that is
                    // `esdev`'s job, on a developer's machine.
                    transform: None,
                    // Nothing is added to the `runtime:` namespace here. A
                    // production binary offers the standard modules and only
                    // those, so `runtime:build` and `runtime:watch` — `esdev`'s
                    // — are not merely unwired but absent.
                    extensions: Vec::new(),
                    observer: None,
                    // esrun has no inspector and no flag that could ask for
                    // one: a debugger port would undo every --deny-* the
                    // deployment was started with (D59).
                    inspector: None,
                });
            }
        }
    }
    Err(format!("missing script argument\n\n{USAGE}"))
}

/// Whether `flag` is one esrun itself understands.
fn is_esrun_flag(flag: &str) -> bool {
    if RunOptions::is_shared_flag(flag) {
        return true;
    }
    if matches!(
        flag,
        "-h" | "--help"
            | "-v"
            | "-V"
            | "--version"
            | "-e"
            | "--eval"
            | "--deny-all"
            | "--allow-all"
            | "-A"
    ) {
        return true;
    }
    Permissions::is_permission_flag(flag)
}

/// Rejects an esrun flag that appears *after* the script, where it is the
/// script's own argument and does nothing to the run.
///
/// **Order is part of the grammar:** esrun's flags come before the script, and
/// everything after it belongs to the script. That split is what lets a script
/// have flags of its own without colliding with the runtime's — but it means a
/// misplaced flag silently does nothing, which for `--deny-net` is a security
/// failure and for the rest is a confusing no-op. `--` suppresses the check for
/// a script that genuinely wants such an argument.
fn reject_esrun_flags_after_source(args: &[String], source: &str) -> Result<(), String> {
    for arg in args {
        // Everything past `--` is the script's, verbatim and unexamined.
        if arg == "--" {
            return Ok(());
        }
        let (flag, _) = split_flag_value(arg);
        if is_esrun_flag(flag) {
            return Err(format!(
                "{arg} appears after {source}, where it is the script's own argument and \
                 does nothing to the run.\n\n\
                 esrun's flags come before the script: `esrun {arg} {source} ...`. \
                 If the script really wants this argument, separate it with `--`."
            ));
        }
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    // Before anything that could log. Installing a subscriber is a
    // process-global act, so a library crate must not do it. Quiet by default
    // (`warn`); `RUST_LOG` opens it up, e.g. `RUST_LOG=runtime::http=debug`.
    es_runtime_common::telemetry::init_tracing();
    let result = match parse_args() {
        Ok(config) => es_runtime_cli_common::run("esrun", config).await,
        Err(err) => Err(err),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            print_error(&err);
            ExitCode::FAILURE
        }
    }
}
