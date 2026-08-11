//! `esdev` — the local development binary for the ES-Runtime.
//!
//! `esrun` is the production server runtime: it runs a service and does nothing
//! else, and that narrowness is deliberate — no inspector port, no file
//! watcher, no test discovery, nothing that could weaken the capability model it
//! exists to enforce. The cost of that lands entirely on the developer's inner
//! loop, and `esdev` is the binary that pays it.
//!
//! **It never changes what the JS sees.** Same prelude, same snapshot, same
//! providers, same capability enforcement — all of it shared with `esrun`
//! through `es-runtime-cli-common`, so a program cannot behave one way here and
//! another in production. What `esdev` changes is everything *around* a run:
//! watching, restarting, attaching, discovering, reporting, building.
//!
//! Argument grammar is `esrun`'s, unchanged: every flag is `--flag` or
//! `--flag=value` — a value is never a separate argument — and esdev's flags
//! come **before** the script, since everything after it belongs to the script.

// A CLI's whole job is to talk to the terminal.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::process::ExitCode;

use es_runtime_cli_common::args::{
    RunOptions, reject_value, require_value, split_flag_value, try_permission_flag,
};
use es_runtime_cli_common::diagnostics::print_error;
use es_runtime_cli_common::permissions::Permissions;
use es_runtime_cli_common::{Config, Source};

mod transform;
use transform::TypeStripper;

const USAGE: &str = "\
esdev — the local development binary for the ES-Runtime

Runs your service the way esrun will, with the tooling you need to get it there.
Every flag is either `--flag` or `--flag=value`. A value is never a separate
argument: `--timeout=500`, not `--timeout 500`.

USAGE:
    esdev <file>                Run a module file — .js, .mjs, or .ts/.tsx/.jsx
    esdev -e=<code>             Run an inline module snippet (JavaScript)
    esdev -h, --help            Show this help
    esdev -v, --version         Show the version

TYPESCRIPT & JSX:
    .ts, .tsx, .mts, .cts and .jsx files are stripped to JavaScript as they
    load — types erased, never checked (that is your editor's job, and
    `tsc --noEmit`'s). A .js file is passed through untouched.

    Import specifiers are left exactly as written, so a specifier must name the
    file that exists: `import './app.ts'`, not './app.js'. Resolution is the
    same as esrun's, because it is esrun's.

    JSX compiles to the automatic runtime, `react/jsx-runtime` by default.
    Point it elsewhere per file with a pragma:

        /** @jsxImportSource remix/ui */

RUN OPTIONS (identical to esrun — a program behaves the same under both):
    --deny-all                  Run with no host access at all
    --deny-<name>               Deny one capability; repeatable
    --allow-<name>[=<list>]     Grant one back, optionally narrowed to a list;
                                requires --deny-all. <name> is one of: read,
                                write, imports, net, listen, env, run, signals,
                                workers
    --import-policy=<file>      JSON policy for what may be loaded
    -t=<ms>, --timeout=<ms>     Stop execution after <ms> ms
    --max-heap=<mb>             Heap ceiling in megabytes
    --env-file=<path>           Load env vars from a .env file
    --env-override              Let --env-file values override the OS environment
    --shutdown-grace=<ms>       How long in-flight HTTP requests may finish after
                                ^C/SIGTERM (default 10000)

esdev is for your machine. It is not a deployment target: ship the artifact and
run it under esrun, which has no development surface to attack.
";

/// Parses `esdev`'s command line.
///
/// The shared flags go to `cli-common` — the same code `esrun` parses them with,
/// so the two cannot drift on what `--allow-net=…` or `--max-heap=…` means.
/// Matched below is what only `esdev` has.
fn parse_args() -> Result<Config, String> {
    let mut options = RunOptions::default();
    let mut permissions = Permissions::default();
    // The flag the previous argument was, so a bare word following it can be
    // diagnosed as an attempted value rather than silently becoming the script.
    let mut previous_flag: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let preceding_flag = previous_flag.take();
        let (flag, value) = split_flag_value(&arg);
        if flag.starts_with('-') && flag.len() > 1 {
            previous_flag = Some(flag.to_string());
        }
        if options.try_flag(flag, value)? || try_permission_flag(&mut permissions, flag, value)? {
            continue;
        }
        match flag {
            "-h" | "--help" => {
                reject_value(flag, value)?;
                println!("{USAGE}");
                std::process::exit(0);
            }
            "-v" | "-V" | "--version" => {
                reject_value(flag, value)?;
                println!("esdev {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-e" | "--eval" => {
                let code = require_value(flag, value)?.to_string();
                let rest: Vec<String> = args.collect();
                reject_esdev_flags_after_source(&rest, "the -e code")?;
                return Ok(Config {
                    source: Source::Inline(code),
                    args: rest,
                    capabilities: permissions.resolve()?,
                    scopes: permissions.scopes()?,
                    options,
                    transform: Some(std::sync::Arc::new(TypeStripper)),
                });
            }
            flag if flag.starts_with('-') && flag.len() > 1 => {
                return Err(format!("unknown option: {flag}\n\n{USAGE}"));
            }
            path => {
                // A bare word is the script. But `--deny-net example.com app.js`
                // still *reads* like a value to whoever typed it, and would
                // otherwise run `example.com` as the script — a "cannot read"
                // three steps from the cause. Say what happened.
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
                reject_esdev_flags_after_source(&rest, path)?;
                return Ok(Config {
                    source: Source::File(path.to_string()),
                    args: rest,
                    capabilities: permissions.resolve()?,
                    scopes: permissions.scopes()?,
                    options,
                    transform: Some(std::sync::Arc::new(TypeStripper)),
                });
            }
        }
    }
    Err(format!("missing script argument\n\n{USAGE}"))
}

/// Whether `flag` is one esdev itself understands.
fn is_esdev_flag(flag: &str) -> bool {
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

/// Rejects an esdev flag that appears *after* the script, where it is the
/// script's own argument and does nothing to the run.
///
/// Same rule as `esrun`, and for the same reason: order is part of the grammar,
/// so a misplaced flag silently does nothing — which for `--deny-net` is a
/// security failure and for the rest is a confusing no-op. `--` suppresses the
/// check for a script that genuinely wants such an argument.
fn reject_esdev_flags_after_source(args: &[String], source: &str) -> Result<(), String> {
    for arg in args {
        // Everything past `--` is the script's, verbatim and unexamined.
        if arg == "--" {
            return Ok(());
        }
        let (flag, _) = split_flag_value(arg);
        if is_esdev_flag(flag) {
            return Err(format!(
                "{arg} appears after {source}, where it is the script's own argument and \
                 does nothing to the run.\n\n\
                 esdev's flags come before the script: `esdev {arg} {source} ...`. \
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
        Ok(config) => es_runtime_cli_common::run("esdev", config).await,
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
