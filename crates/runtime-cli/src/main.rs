//! `esrun` — a standalone CLI that runs JavaScript on the ES-Runtime.
//!
//! This is the thin executable wrapper around the embeddable `runtime` library.
//! The wiring itself — the default tokio providers, the [`Runtime`], the module
//! load and the drive loop — lives in `es-runtime-cli-common` and is shared with
//! `esdev`, so a program behaves identically under either binary (SPEC.md §8).
//! What remains here is `esrun`'s own command line: its flags. Nothing here is
//! for development — the TypeScript definitions (D59) and the self-upgrade
//! (D204) used to live in this binary and now belong to `esdev`, which is the
//! one a developer runs.
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
    esrun -e=<code>, --eval=<code>
                                Run an inline module snippet
    esrun -h, --help            Show this help
    esrun -v, -V, --version     Show the version

PERMISSIONS:
    --allow-<name>              Grant one capability; repeatable. <name> is one
                                of: read, write, imports, net, listen, env, run,
                                signals, workers, diagnostics, diagnostics-detail
    --allow-<name>=<list>       ...narrowed to a comma-separated list, for read,
                                write, net, listen, env, run and signals
    -A, --allow-all             Grant every capability (unsandboxed)
    --deny-<name>               Take one back; requires --allow-all; repeatable
    --deny-all                  Grant nothing — the default, said outright

OPTIONS:
    -t, --timeout=<ms>          Stop execution after <ms> and exit 124
    --max-heap=<mb>             Heap ceiling in megabytes, for this agent and as
                                the ceiling its workers inherit. Default: sized
                                from the container's memory limit, or the host's
    --env-file=<path>           Load env vars from a .env file
    --env-override              ...and let them override the OS environment
    --shutdown-grace=<ms>       How long in-flight HTTP requests may finish
                                after ^C/SIGTERM (default 10000)
    --tls-ca=<file|bundled>     Trust exactly these CAs for every TLS client
                                (repeatable; `bundled` is the public roots).
                                Default: the bundled Mozilla roots
    --tls-crl=<file>            Check servers against these CRLs (repeatable)
    --proxy=<url>               Send fetch, WebSocket and --otel through this
                                http:// or https:// proxy (user:pass@ for
                                credentials). No environment variable is read
    --no-proxy=<list>           Targets that go direct: domains, IPs, CIDR
                                blocks, host:port (repeatable). Loopback
                                always does
    --otel[=<url>]              Export OpenTelemetry to a collector (bare:
                                http://localhost:4318). Needs no permission
    --otel-service=<name>       service.name on what is exported (default: the
                                entry file's name)
    --otel-min-duration=<ms>    Drop spans shorter than this before export
    --otel-sample=<0..1>        Fraction of traces to export (default 1)

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
project root — the project you run esrun in, which is also the filesystem jail
and which nothing on the command line can move. Remote (https://) modules are deliberately
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
        // Only a flag written bare: `--allow-net=db:5432 app.js` already has
        // its value, so the word after it is not a misplaced one.
        if flag.starts_with('-') && flag.len() > 1 && value.is_none() {
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
            // Moved to `esdev upgrade` (D204), for the reason `types` moved: a
            // deployment pins its runtime rather than letting it replace
            // itself. Named here so the old command gets an answer rather
            // than "cannot read upgrade".
            "upgrade" => {
                return Err(
                    "esrun no longer upgrades itself: a deployment pins its esrun in its \
                     image or install step.\n\n\
                     On a development machine, `esdev upgrade` updates esrun and esdev \
                     together (`esdev upgrade --only=esrun` for esrun alone).\n\
                     Elsewhere, re-run the installer, pinning the version:\n  \
                     curl -fsSL https://esrun.opentechf.org/install.sh | ESRUN_VERSION=<version> bash -s -- --only=esrun\n\
                     On Windows (PowerShell):\n  \
                     $env:ESRUN_VERSION = '<version>'; $env:ES_RUNTIME_ONLY = 'esrun'; irm https://esrun.opentechf.org/install.ps1 | iex"
                        .to_string(),
                );
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
                    permissive: permissions.permissive(),
                    // A bare grant is the project; further is a typed path (D233).
                    paths_everywhere: false,
                    linked_packages: Vec::new(),
                    options,
                    // esrun runs JavaScript. Turning a `.ts` into that is
                    // `esdev`'s job, on a developer's machine.
                    transform: None,
                    // Strict: a deployment runs a bundle, and a specifier that
                    // names no file is a mistake rather than something to
                    // search for.
                    bundler_style_resolution: false,
                    package_converter: None,
                    specifier_alias: None,
                    module_hooks: None,
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
                    console: None,
                    on_start: None,
                });
            }
            // `--` before any script: it separates a script's own arguments
            // from esrun's flags, so there is nothing yet for it to separate.
            "--" => {
                return Err(
                    "`--` comes after the script, to pass the script arguments that \
                     look like esrun flags: `esrun app.js -- --timeout=5`. There is no script \
                     before it here."
                        .to_string(),
                );
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
                // Only for a flag that takes a value at all: suggesting
                // `--allow-all=app.js` names a flag this parser refuses (D244).
                if let Some(flag) = preceding_flag
                    && takes_value(&flag)
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
                    permissive: permissions.permissive(),
                    // A bare grant is the project; further is a typed path (D233).
                    paths_everywhere: false,
                    linked_packages: Vec::new(),
                    options,
                    // esrun runs JavaScript. Turning a `.ts` into that is
                    // `esdev`'s job, on a developer's machine.
                    transform: None,
                    // Strict: a deployment runs a bundle, and a specifier that
                    // names no file is a mistake rather than something to
                    // search for.
                    bundler_style_resolution: false,
                    package_converter: None,
                    specifier_alias: None,
                    module_hooks: None,
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
                    console: None,
                    on_start: None,
                });
            }
        }
    }
    Err(format!("missing script argument\n\n{USAGE}"))
}

/// Whether `flag` takes a value, so that a bare word after it may have been
/// meant as one. Asked of the parsers that own each flag rather than listed.
fn takes_value(flag: &str) -> bool {
    if RunOptions::is_shared_flag(flag) {
        return RunOptions::accepts_value(flag);
    }
    matches!(flag, "-e" | "--eval") || Permissions::accepts_value(flag)
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

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("error: cannot start the async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    let code = runtime.block_on(run());
    // Dropping a runtime waits, without limit, for its blocking work (D241).
    // A run that ended normally has none — the drive returned because nothing
    // was pending — so there is nothing to wait for; one that was stopped (a
    // timeout, an uncaught failure) may still have a PBKDF2 or a key generation
    // running there whose result nobody will read. Waiting for it turned a
    // 300 ms `--timeout` into the derive's whole duration. The process leaves
    // now, as `exit()` and a signal already make it.
    runtime.shutdown_background();
    code
}

async fn run() -> ExitCode {
    // Before anything that could log. Installing a subscriber is a
    // process-global act, so a library crate must not do it. Quiet by default
    // (`warn`); `RUST_LOG` opens it up, e.g. `RUST_LOG=runtime::http=debug`.
    es_runtime_common::telemetry::init_tracing();
    let result = match parse_args() {
        Ok(config) => es_runtime_cli_common::run("esrun", config).await,
        Err(err) => Err(err.into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            print_error(err.message());
            // `1` for a failure, `124` for a run `--timeout` stopped.
            ExitCode::from(err.exit_code())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D244: the "attach it with '='" hint is for a flag that takes a value.
    /// It was offered after any flag, so `esrun --allow-all app.js` with a
    /// missing file suggested `--allow-all=app.js`, which is refused.
    #[test]
    fn only_a_flag_with_a_value_form_takes_a_value() {
        for flag in [
            "--timeout",
            "--max-heap",
            "--env-file",
            "--otel",
            "--allow-net",
            "--allow-read",
            "--allow-signals",
            "-e",
        ] {
            assert!(takes_value(flag), "{flag}");
        }
        for flag in [
            "--allow-all",
            "-A",
            "--deny-all",
            "--deny-net",
            "--allow-imports",
            "--allow-workers",
            "--allow-diagnostics",
            "--env-override",
            "--help",
        ] {
            assert!(!takes_value(flag), "{flag}");
        }
    }

    /// D244: an `--otel*` flag after the script is refused like every other
    /// esrun flag there, instead of becoming the program's argument.
    #[test]
    fn an_otel_flag_after_the_script_is_refused() {
        for flag in [
            "--otel=http://x:4318",
            "--otel-service=api",
            "--otel-sample=0.5",
        ] {
            let err = reject_esrun_flags_after_source(&[flag.to_string()], "app.js").unwrap_err();
            assert!(err.contains("appears after app.js"), "{flag}: {err}");
        }
        let after_separator = ["--".to_string(), "--otel=http://x:4318".to_string()];
        assert!(reject_esrun_flags_after_source(&after_separator, "app.js").is_ok());
    }

    /// The help names every flag the shared parser takes: `--otel*` and
    /// `--eval` were missing from it, and so from the page copied from it.
    #[test]
    fn the_usage_names_every_shared_flag() {
        for flag in [
            "--timeout",
            "--max-heap",
            "--env-file",
            "--env-override",
            "--shutdown-grace",
            "--tls-ca",
            "--tls-crl",
            "--proxy",
            "--no-proxy",
            "--otel",
            "--otel-service",
            "--otel-min-duration",
            "--otel-sample",
        ] {
            assert!(RunOptions::is_shared_flag(flag), "{flag} is not shared");
            assert!(USAGE.contains(flag), "--help does not name {flag}");
        }
        assert!(USAGE.contains("--eval"), "--help does not name --eval");
    }

    /// The help names every permission, from the vocabulary it is part of.
    #[test]
    fn the_usage_names_every_permission() {
        let usage = USAGE.split_whitespace().collect::<Vec<_>>().join(" ");
        for name in es_runtime_common::Capability::HOST_FACING
            .into_iter()
            .filter_map(es_runtime_common::Capability::flag_name)
        {
            assert!(
                usage.contains(&format!(" {name},"))
                    || usage.contains(&format!(" {name}\n"))
                    || usage.contains(&format!(" {name} ")),
                "--help does not name {name}"
            );
        }
    }
}
