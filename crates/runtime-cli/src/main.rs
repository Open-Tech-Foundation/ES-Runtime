//! `esrun` — a standalone CLI that runs JavaScript on the ES-Runtime.
//!
//! This is the thin executable wrapper around the embeddable `runtime` library.
//! The wiring itself — the default tokio providers, the [`Runtime`], the module
//! load and the drive loop — lives in `es-runtime-cli-common` and is shared with
//! `esdev`, so a program behaves identically under either binary (SPEC.md §8).
//! What remains here is `esrun`'s own command line: its flags and its `upgrade`
//! subcommand. Nothing here is for development — the TypeScript definitions used
//! to be installed from this binary and now belong to `esdev`, which is the one
//! a developer runs (D59).
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

Every flag is either `--flag` or `--flag=value`. A value is never a separate
argument: `--timeout=500`, not `--timeout 500`.

USAGE:
    esrun <file>                Run a JavaScript module file
    esrun -e=<code>             Run an inline module snippet
    esrun --allow-<name>        Grant one capability; repeatable
                                <name> is one of: read, write, imports, net,
                                listen, env, run, signals, workers
    esrun --allow-all, -A       Grant every capability (unsandboxed)
    esrun --deny-<name>         Take one back; requires --allow-all; repeatable
    esrun --deny-all            Grant nothing — the default, said outright
    esrun --allow-<name>=<list> Grant it narrowed to a comma-separated list:
                                read/write (paths), net/listen (addresses),
                                run (programs), env (variable names),
                                signals (signal names). imports and workers
                                take no list — a worker's own grant is set at
                                the spawn, `new Worker(url, { permissions })`
    esrun --import-policy=<file>
                                JSON policy for what may be loaded (allow/deny
                                lists of packages and paths)
    esrun -t=<ms>, --timeout=<ms>
                                Stop execution after <ms> ms (watchdog, SPEC §4)
    esrun --max-heap=<mb>       Heap ceiling in megabytes, for this agent and as
                                the ceiling its workers inherit. Default: sized
                                from the container's memory limit, or the host's
                                memory when there is none
    esrun --env-file=<path>     Load env vars from a .env file
    esrun --env-override        Let --env-file values override the OS environment
    esrun --shutdown-grace=<ms> How long in-flight HTTP requests may finish after
                                ^C/SIGTERM (default 10000)
    esrun upgrade               Update esrun to the latest release
    esrun -h, --help            Show this help
    esrun -v, --version         Show the version

Inputs run as ES modules: import/export and top-level await work. Imports
resolve as local files (relative/absolute paths or file: URLs) and as bare
specifiers through node_modules (ES module packages only — CommonJS packages
and node: builtins are rejected; nothing is installed). Static and dynamic
import() both work; import attributes (`with { type: \"json\" }`) are supported.
Remote (`https://`) modules are explicitly unsupported to enforce a local-only security model.
The full WinterTC surface is available (console, URL, fetch, crypto, streams,
encoding, timers, events).

Nothing is granted by default: a run reaches what it was named on the command
line that started it, and nothing else. Widen it in one of two ways, never both —
each has a single direction, so no flag ever overrides another:

    esrun --allow-net --allow-read app.js  # nothing, plus these
    esrun --allow-net=api.example.com app.js   # ...and only there
    esrun --allow-all --deny-run app.js    # everything, minus these

--deny-<name> requires --allow-all (with nothing granted, there is nothing for
it to take away). A denied operation throws NotAllowedError; importing a
runtime: module always works. With no flags at all a run is a single file: it
can compute, but cannot read, write, import another file, reach the network,
read the environment, or spawn anything — a multi-file program needs at least
--allow-imports. Ask from JS with `permissions.has(name)` from runtime:process.

A scope list narrows a grant. --allow-env=HOME,PATH hides every other variable;
--allow-run=git,ls refuses to spawn anything else; --allow-net=api.example.com
refuses every other host, on every redirect hop as well as the first request;
--allow-listen=127.0.0.1:8080 refuses every other bind; --allow-read=./data
refuses every other path; --allow-signals=SIGTERM refuses to watch anything
else.

An address is a host, a host:port, or a bare port (any interface); [::1]:8080
for IPv6. A path is absolute or relative to the working directory and covers its
subtree. Matching is exact — example.com does not admit api.example.com, ./app
does not admit ./app-secrets, and there are no wildcards. Paths are checked
after canonicalization, so a symlink cannot walk out of a list, and a path list
narrows the root jail; a path outside it adds that subtree, which is how a run
reaches a certificate or a CA bundle the project does not contain.

Entries are comma-separated and trimmed (`--allow-env=\"A, B\"` ≡
`--allow-env=A,B`); an empty entry is an error. Denials take no value at all: a
scope narrows a grant, so it is written --allow-<name>=<list>.

What may be *loaded* is a separate question from what running code may reach, so
it is a separate mechanism: --import-policy=<file> takes JSON with \"allow\"
and/or \"deny\" lists of package names and paths. Deny wins; omitting \"allow\"
permits everything not denied; paths resolve relative to the policy file. The
imports capability decides whether the loader runs at all, the policy decides
what it may resolve — so a policy is not a way around --deny-imports.

Permission flags must come before the script; after it they would be the
script's own arguments.

Everything after <file> (or the -e code) belongs to the script, readable as
`args` from runtime:process.";

/// `esrun upgrade` — find the latest GitHub release for this target, download +
/// extract it, and replace the running binary in place (the same outcome as
/// re-running install.sh / install.ps1, but built in). HTTPS via rustls.
// Returns a `String` error (not a boxed `dyn Error`) so the result is `Send` and
// can cross the OS-thread boundary this runs on (see the `"upgrade"` dispatch).
fn upgrade() -> Result<String, String> {
    // Release assets are named `esrun-<os>-<arch>.{tar.gz,zip}` by the
    // otf-release tool (see .github/workflows/release.yml), e.g.
    // `esrun-linux-x86-64.tar.gz`. self_update selects the asset whose name
    // contains its configured `target`, so build that `<os>-<arch>` token for
    // the running platform rather than using the default Rust target triple.
    let os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86-64"
    };
    let target = format!("{os}-{arch}");

    let status = self_update::backends::github::Update::configure()
        .repo_owner("Open-Tech-Foundation")
        .repo_name("ES-Runtime")
        .bin_name("esrun")
        .target(&target)
        // The archive holds the binary at its root, so `{{ bin }}` alone (which
        // self_update fills with the bin name plus the platform `.exe` suffix on
        // Windows) is the in-archive path.
        .bin_path_in_archive("{{ bin }}")
        // Disambiguate the archive from any same-target sidecar by extension.
        .asset_identifier(if cfg!(windows) { ".zip" } else { ".tar.gz" })
        .current_version(env!("CARGO_PKG_VERSION"))
        .show_download_progress(true)
        .build()
        .map_err(|e| e.to_string())?
        .update()
        .map_err(|e| e.to_string())?;
    Ok(if status.is_updated() {
        format!("Upgraded esrun to {}.", status.version())
    } else {
        format!("esrun is already up to date ({}).", status.version())
    })
}

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
                // `self_update` drives its own blocking HTTP runtime; running it
                // inside this `#[tokio::main]` context would drop that runtime
                // from within an async context and panic. Run it on a dedicated
                // OS thread, off the tokio runtime.
                let result = std::thread::spawn(upgrade)
                    .join()
                    .unwrap_or_else(|_| Err("the upgrade thread panicked".to_string()));
                match result {
                    Ok(msg) => println!("{msg}"),
                    Err(e) => {
                        eprintln!("error: upgrade failed: {e}");
                        std::process::exit(1);
                    }
                }
                std::process::exit(0);
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
