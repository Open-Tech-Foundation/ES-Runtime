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

mod build;
mod transform;
mod watch;
use build::BuildConfig;
use transform::TypeStripper;
use watch::WatchConfig;

/// What the command line asked for.
enum Command {
    /// Run a module. `Config` is large, so it is boxed rather than making the
    /// build variant carry its weight.
    Run(Box<Config>),
    /// Bundle a module and its dependencies.
    Build(BuildConfig),
    /// Run a module, restarting it when its source changes.
    Watch(WatchConfig),
}

const USAGE: &str = "\
esdev — the local development binary for the ES-Runtime

Runs your service the way esrun will, with the tooling you need to get it there.
Every flag is either `--flag` or `--flag=value`. A value is never a separate
argument: `--timeout=500`, not `--timeout 500`.

USAGE:
    esdev <file>                Run a module file — .js, .mjs, or .ts/.tsx/.jsx
    esdev -e=<code>             Run an inline module snippet (JavaScript)
    esdev --watch <file>        Run it, and rerun it when its source changes
    esdev build <entry>         Bundle an entry into one deployable ES module
                                (`esdev build --help` for its options)
    esdev -h, --help            Show this help
    esdev -v, --version         Show the version

WATCH:
    --watch reruns the program in a fresh process on every change, so nothing
    leaks between runs. A restart is a SIGTERM, which is the same graceful stop
    production gets: a server stops accepting, answers the requests already in
    flight, and only then exits — so a save while a request is open does not
    drop it. --shutdown-grace bounds that wait, after which the process is
    killed.

    Watched: the project root (nearest package.json) or the entry's directory,
    minus node_modules, .git, dist, target and .cache, and only for source
    extensions. A program that exits leaves the watcher up, waiting for the
    next change.

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

const BUILD_USAGE: &str = "\
esdev build — bundle a server entry and its dependencies into one ES module

USAGE:
    esdev build <entry> [options]

OPTIONS:
    --out=<file>                Where to write it (default: dist/<entry>.js)
    --minify                    Minify the output
    --conditions=<list>         Extra `exports` conditions, comma-separated.
                                These add to the defaults (import, default,
                                worker)
    --define=<name>=<value>     Replace <name> with <value> at build time.
                                process.env.NODE_ENV defaults to \"production\"
    -h, --help                  Show this help

The bundle is ES modules, `runtime:*` imports are left for the runtime to
serve, and CommonJS dependencies are converted on the way in — which is how a
package that ships CJS becomes runnable without esrun learning `require`.

It also shortens what production must be granted. An unbundled program needs
--allow-imports so the loader can walk node_modules; a bundle has no imports
left to resolve:

    esrun --deny-all --allow-imports --allow-listen=8080 app.js   # unbundled
    esrun --deny-all --allow-listen=8080 dist/app.js              # bundled
";

/// Parses `esdev`'s command line.
///
/// The shared flags go to `cli-common` — the same code `esrun` parses them with,
/// so the two cannot drift on what `--allow-net=…` or `--max-heap=…` means.
/// Matched below is what only `esdev` has.
fn parse_args() -> Result<Command, String> {
    // `build` is a subcommand, not a flag, and everything after it is its own.
    // Requiring it first keeps that unambiguous: there is no reading to be done
    // about whether `--deny-all` before it was meant to shape a bundle (it
    // cannot — a bundle does not run) or the run that is not happening.
    let mut argv = std::env::args().skip(1);
    if let Some(first) = argv.next()
        && first == "build"
    {
        return parse_build(argv).map(Command::Build);
    }

    let mut options = RunOptions::default();
    let mut permissions = Permissions::default();
    let mut watching = false;
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
            "--watch" => {
                reject_value(flag, value)?;
                watching = true;
            }
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
                if watching {
                    return Err("--watch needs a file to watch; -e code has none.\n\n\
                         Put the snippet in a file and watch that."
                        .to_string());
                }
                let rest: Vec<String> = args.collect();
                reject_esdev_flags_after_source(&rest, "the -e code")?;
                return Ok(Command::Run(Box::new(Config {
                    source: Source::Inline(code),
                    args: rest,
                    capabilities: permissions.resolve()?,
                    scopes: permissions.scopes()?,
                    options,
                    transform: Some(std::sync::Arc::new(TypeStripper)),
                })));
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
                if watching {
                    return Ok(Command::Watch(WatchConfig {
                        // The same command line, minus the flag that put us
                        // here — so the child runs exactly the program the user
                        // described, under the same grants.
                        child_args: std::env::args()
                            .skip(1)
                            .filter(|a| a != "--watch")
                            .collect(),
                        entry: std::path::PathBuf::from(path),
                        grace: options.shutdown_grace,
                    }));
                }
                return Ok(Command::Run(Box::new(Config {
                    source: Source::File(path.to_string()),
                    args: rest,
                    capabilities: permissions.resolve()?,
                    scopes: permissions.scopes()?,
                    options,
                    transform: Some(std::sync::Arc::new(TypeStripper)),
                })));
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
            | "--watch"
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

/// Parses `esdev build <entry> [options]`.
fn parse_build(args: impl Iterator<Item = String>) -> Result<BuildConfig, String> {
    let mut entry: Option<String> = None;
    let mut out = None;
    let mut minify = false;
    let mut conditions = Vec::new();
    let mut defines = Vec::new();
    for arg in args {
        let (flag, value) = split_flag_value(&arg);
        match flag {
            "-h" | "--help" => {
                reject_value(flag, value)?;
                println!("{BUILD_USAGE}");
                std::process::exit(0);
            }
            "--out" => out = Some(require_value(flag, value)?.to_string()),
            "--minify" => {
                reject_value(flag, value)?;
                minify = true;
            }
            "--conditions" => {
                for name in require_value(flag, value)?.split(',') {
                    let name = name.trim();
                    if name.is_empty() {
                        return Err(format!(
                            "{flag}={} has an empty entry — a stray comma is a typo, and a \
                             condition decides which code a package hands over.",
                            value.unwrap_or_default()
                        ));
                    }
                    conditions.push(name.to_string());
                }
            }
            "--define" => {
                let pair = require_value(flag, value)?;
                let (name, replacement) = pair.split_once('=').ok_or_else(|| {
                    format!(
                        "{flag}={pair} is not a replacement — write \
                         --define=<name>=<value>, e.g. \
                         --define=process.env.NODE_ENV=\\\"development\\\"."
                    )
                })?;
                defines.push((name.to_string(), replacement.to_string()));
            }
            flag if flag.starts_with('-') && flag.len() > 1 => {
                return Err(format!("unknown option: {flag}\n\n{BUILD_USAGE}"));
            }
            path => {
                if entry.is_some() {
                    return Err(format!(
                        "esdev build takes one entry; got a second ({path}).\n\n\
                         A bundle has one root — that is what makes it one file."
                    ));
                }
                entry = Some(path.to_string());
            }
        }
    }
    let entry = entry.ok_or_else(|| format!("missing entry argument\n\n{BUILD_USAGE}"))?;
    Ok(BuildConfig {
        entry,
        out,
        minify,
        conditions,
        defines,
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    // Before anything that could log. Installing a subscriber is a
    // process-global act, so a library crate must not do it. Quiet by default
    // (`warn`); `RUST_LOG` opens it up, e.g. `RUST_LOG=runtime::http=debug`.
    es_runtime_common::telemetry::init_tracing();
    let result = match parse_args() {
        Ok(Command::Run(config)) => es_runtime_cli_common::run("esdev", *config).await,
        Ok(Command::Watch(config)) => watch::supervise(config).await,
        Ok(Command::Build(config)) => match build::build(config).await {
            Ok(written) => {
                println!("bundled → {written}");
                Ok(())
            }
            Err(err) => Err(err),
        },
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
