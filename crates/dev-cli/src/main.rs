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
mod declarations;
mod inspect;
mod test;
mod trace;
mod transform;
mod types;
mod watch;
use build::BuildConfig;
use inspect::InspectConfig;
use test::TestConfig;
use trace::PermissionTrace;
use transform::TypeStripper;
use watch::WatchConfig;

/// What the command line asked for.
enum Command {
    /// Run a module. `Config` is large, so it is boxed rather than making the
    /// build variant carry its weight. The debugger endpoint travels beside it
    /// rather than in it: parsing decides *what* was asked for, and binding a
    /// port is something `main` does once, after the whole command line has been
    /// found to make sense.
    Run(Box<Config>, Option<InspectConfig>),
    /// Bundle a module and its dependencies.
    Build(BuildConfig),
    /// Run a module, restarting it when its source changes.
    Watch(WatchConfig),
    /// Discover and run test files.
    Test(TestConfig),
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
    esdev --inspect <file>      Run it with a debugger attached
    esdev --trace-permissions <file>
                                Run it, then print the permissions it used
    esdev --install-types       Add the runtime: TypeScript definitions to this
                                project and wire up tsconfig.json
    esdev test [filter...]      Run the test files (`esdev test --help`)
    esdev build <entry>         Bundle an entry into one deployable ES module
    esdev build --lib <srcdir>  Build a publishable library instead: a module
                                tree and its .d.ts, dependencies left external
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

DEBUGGER:
    --inspect[=<addr>]          Serve the Chrome DevTools Protocol, default
                                127.0.0.1:9229. Attach with chrome://inspect,
                                VS Code, or any CDP client
    --inspect-brk[=<addr>]      ...and stop before the first statement, so a
                                program that ends quickly can still be debugged

    <addr> is a port (9229), an address (127.0.0.1) or both. Binding anywhere
    but loopback is allowed and warned about: a debugger port is a way to run
    code in this process regardless of what it was denied.

    This is why there is a second binary at all. esrun has no --inspect and no
    code that could serve one, and esdev only has it when the build asked:

        ES_RUNTIME_INSPECTOR=1 cargo build --release -p es-runtime-dev-cli

    A build without it accepts the flag and fails with that line, rather than
    listening on nothing.

PERMISSIONS:
    --trace-permissions         Watch every capability the run reaches for, and
                                print the esrun line that grants exactly those:

                                  esrun --deny-all --allow-read --allow-net app.js

    What it records is the check itself, so it reports what the program *used*
    rather than what it was given — including the ones it asked for and was
    refused, which are listed and deliberately left out of the line. Workers are
    traced into the same report; their grants are set at the spawn, which is
    where they are hardest to get right.

    Scopes are not traced: the line grants each capability unnarrowed. Narrow it
    by hand (--allow-read=./data) once the trace has told you which you need.

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

    --install-types adds @opentf/esrun-types (the runtime: definitions, on npm)
    as a dev dependency with the package manager your lockfile names, and adds
    it to compilerOptions.types so an editor resolves `import … from
    \"runtime:fs\"`. Types are for your editor and `tsc --noEmit`; esdev never
    checks them.

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

const TEST_USAGE: &str = "\
esdev test — run the test files

USAGE:
    esdev test [filter...]      Run every *.test.{js,mjs,ts,tsx,jsx} found
    esdev test <filter>         ...whose path contains <filter>
    esdev test --file=<path>    Run exactly one file
    esdev test -h, --help       Show this help

Each file runs in its own process, so one that wedges, exhausts its heap or
calls exit() cannot decide the fate of the others, and a global left behind by
one file is not visible to the next.

The file itself is the entry — it keeps its own path, its module resolution and
its TypeScript — and arrives with the globals already defined:

    test(name, fn)              fn may be async; failures are collected
    assert(cond, msg?)
    assertEquals(actual, expected, msg?)
    assertThrows(fn, msg?)
    assertRejects(fn, msg?)

The same vocabulary the runtime's own conformance suite uses. Exits non-zero if
any file fails.
";

const BUILD_USAGE: &str = "\
esdev build — build an application to deploy, or a library to publish

USAGE:
    esdev build <entry> [options]        One deployable ES module
    esdev build --lib <srcdir> [options] A publishable library

OPTIONS:
    --lib                       Build a library: keep the module structure,
                                leave dependencies external, emit .d.ts
    --no-types                  --lib only: skip the .d.ts files
    --out=<path>                Where to write it. A file for an application
                                (default dist/<entry>.js), a directory for --lib
                                (default dist)
    --minify                    Minify the output
    --conditions=<list>         Extra `exports` conditions, comma-separated.
                                These add to the defaults (import, default,
                                worker — none of which --lib asserts)
    --define=<name>=<value>     Replace <name> with <value> at build time.
                                process.env.NODE_ENV defaults to \"production\"
                                for an application, and to nothing for --lib
    -h, --help                  Show this help

APPLICATION (the default)
    The bundle is ES modules, `runtime:*` imports are left for the runtime to
    serve, and CommonJS dependencies are converted on the way in — which is how
    a package that ships CJS becomes runnable without esrun learning `require`.

    It also shortens what production must be granted. An unbundled program needs
    --allow-imports so the loader can walk node_modules; a bundle has no imports
    left to resolve:

        esrun --deny-all --allow-imports --allow-listen=8080 app.js   # unbundled
        esrun --deny-all --allow-listen=8080 dist/app.js              # bundled

LIBRARY (--lib)
    A library is not the end of the line — it is an input to somebody else's
    build, so the four decisions above are theirs to make and --lib makes none
    of them:

        esdev build --lib src            # src/** → dist/**.js + dist/**.d.ts

    * A directory, not an entry. Every module under it is built, the way tsc
      builds a rootDir — because which modules a consumer may import is
      decided by your `exports` map, not by what an entry happens to reach.
      Nothing is tree-shaken away: an export no current caller uses is not
      dead code here, it is the API. Skipped: *.test.* and .d.ts files.
    * Dependencies stay external, so a consumer can still dedupe, override or
      patch one. Only relative and absolute imports are emitted.
    * Module structure is preserved, file for file, so a subpath in your
      `exports` map is a real file and a stack trace names a module.
    * Nothing is defined and no condition asserted: NODE_ENV and `worker`
      belong to the build that consumes this, not to this one.
    * A .d.ts is emitted beside each module, derived from the annotations the
      source already carries — never inferred, the same contract type-stripping
      has. An exported signature that does not state its type fails the build
      with the list, rather than getting a guessed declaration nobody can see is
      wrong. --no-types opts out.
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
    if let Some(first) = argv.next() {
        if first == "build" {
            return parse_build(argv).map(Command::Build);
        }
        if first == "test" {
            return parse_test(argv).map(Command::Test);
        }
    }

    let mut options = RunOptions::default();
    let mut permissions = Permissions::default();
    let mut watching = false;
    let mut inspect: Option<InspectConfig> = None;
    let mut tracing_permissions = false;
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
            "--install-types" => {
                reject_value(flag, value)?;
                let outcome = types::install()?;
                print!("{}", outcome.report);
                // Non-zero when the package did not get installed, even though
                // the tsconfig half did: a setup script that carried on from
                // here would be building against types that are not there.
                std::process::exit(i32::from(!outcome.installed));
            }
            "--trace-permissions" => {
                reject_value(flag, value)?;
                tracing_permissions = true;
            }
            "--inspect" | "--inspect-brk" => {
                inspect = Some(InspectConfig {
                    address: inspect::parse_address(value)?,
                    wait: flag == "--inspect-brk",
                });
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
                return Ok(Command::Run(
                    Box::new(Config {
                        source: Source::Inline(code),
                        args: rest,
                        capabilities: permissions.resolve()?,
                        scopes: permissions.scopes()?,
                        options,
                        transform: Some(std::sync::Arc::new(TypeStripper)),
                        // The deploy line is printed with the entry as it was
                        // named, so it is one a reader can copy. For `-e` there
                        // is nothing to name, and the placeholder says so.
                        observer: permission_trace(tracing_permissions, "-e=<code>"),
                        inspector: None,
                    }),
                    inspect,
                ));
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
                        // described, under the same grants. `--inspect` travels
                        // with it and is served by the child, which is why the
                        // supervisor drops what it parsed: the debugger belongs
                        // to the process being debugged, and its port is bound
                        // and released with each run.
                        child_args: std::env::args()
                            .skip(1)
                            .filter(|a| a != "--watch")
                            .collect(),
                        entry: std::path::PathBuf::from(path),
                        grace: options.shutdown_grace,
                    }));
                }
                return Ok(Command::Run(
                    Box::new(Config {
                        source: Source::File(path.to_string()),
                        args: rest,
                        capabilities: permissions.resolve()?,
                        scopes: permissions.scopes()?,
                        options,
                        transform: Some(std::sync::Arc::new(TypeStripper)),
                        observer: permission_trace(tracing_permissions, path),
                        inspector: None,
                    }),
                    inspect,
                ));
            }
        }
    }
    Err(format!("missing script argument\n\n{USAGE}"))
}

/// The capability observer for a run, when `--trace-permissions` asked for one.
fn permission_trace(tracing: bool, entry: &str) -> Option<es_runtime_cli_common::SharedObserver> {
    tracing.then(|| {
        std::sync::Arc::new(PermissionTrace::new(entry.to_string()))
            as es_runtime_cli_common::SharedObserver
    })
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
            | "--inspect"
            | "--inspect-brk"
            | "--trace-permissions"
            | "--install-types"
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
    let mut sources: Vec<String> = Vec::new();
    let mut out = None;
    let mut minify = false;
    let mut lib = false;
    let mut no_types = false;
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
            "--lib" => {
                reject_value(flag, value)?;
                lib = true;
            }
            "--no-types" => {
                reject_value(flag, value)?;
                no_types = true;
            }
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
            path => sources.push(path.to_string()),
        }
    }
    if sources.is_empty() {
        return Err(format!(
            "missing {} argument\n\n{BUILD_USAGE}",
            if lib { "source directory" } else { "entry" }
        ));
    }
    if sources.len() > 1 {
        return Err(format!(
            "esdev build takes one {}; got {}.\n\n{}",
            if lib { "source directory" } else { "entry" },
            sources.len(),
            if lib {
                "A library is built from its source tree, not from a list — every \
                 module under the directory becomes a file in the output."
            } else {
                "A bundle has one root — that is what makes it one file."
            }
        ));
    }
    let source = sources.remove(0);
    // The whole shape of a library build follows from its unit being a
    // directory, so a file here is not a small mistake to guess past: it would
    // silently produce a tree missing everything the named module happens not
    // to import.
    if lib && std::path::Path::new(&source).is_file() {
        let root = std::path::Path::new(&source)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| ".".to_string(), |p| p.display().to_string());
        return Err(format!(
            "--lib builds a source directory, and {source} is a file.\n\n\
             A library publishes its whole tree — which modules a consumer may \
             import is decided by the package's `exports` map, not by what this \
             entry happens to reach. Build the directory: \
             `esdev build --lib {root}`."
        ));
    }
    if no_types && !lib {
        return Err("--no-types only means something with --lib.\n\n\
             An application build emits no declarations to skip: a bundle is \
             deployed and run, not imported and type-checked."
            .to_string());
    }
    // `--out` changes shape between the two, and getting it wrong is otherwise
    // a directory literally named `app.js` full of modules.
    if lib
        && let Some(path) = &out
        && std::path::Path::new(path).extension().is_some()
    {
        return Err(format!(
            "--out={path} names a file, and --lib writes a directory of them.\n\n\
             A library keeps its module structure, so the output is a tree: \
             --out=dist, not --out=dist/index.js."
        ));
    }
    Ok(BuildConfig {
        source,
        out,
        minify,
        conditions,
        defines,
        lib,
        types: !no_types,
    })
}

/// Parses `esdev test [--file=<path>] [filter...]`.
fn parse_test(args: impl Iterator<Item = String>) -> Result<TestConfig, String> {
    let mut file = None;
    let mut filters = Vec::new();
    for arg in args {
        let (flag, value) = split_flag_value(&arg);
        match flag {
            "-h" | "--help" => {
                reject_value(flag, value)?;
                println!("{TEST_USAGE}");
                std::process::exit(0);
            }
            "--file" => file = Some(require_value(flag, value)?.to_string()),
            flag if flag.starts_with('-') && flag.len() > 1 => {
                return Err(format!("unknown option: {flag}\n\n{TEST_USAGE}"));
            }
            filter => filters.push(filter.to_string()),
        }
    }
    Ok(TestConfig { file, filters })
}

/// Runs one test file, or discovers and runs them all.
///
/// The parent spawns a child per file rather than looping in-process, so a file
/// that hangs or exits takes only itself down. `--file` is what a child is
/// invoked with, and is equally a supported way to run one file by hand.
async fn run_tests(config: TestConfig) -> ExitCode {
    if let Some(file) = config.file {
        let path = std::path::PathBuf::from(&file);
        let run = Config {
            source: Source::File(file.clone()),
            args: Vec::new(),
            capabilities: es_runtime_common::CapabilitySet::all(),
            scopes: std::collections::HashMap::new(),
            options: RunOptions::default(),
            transform: Some(std::sync::Arc::new(test::TestTransform::new(&path))),
            observer: None,
            inspector: None,
        };
        return match es_runtime_cli_common::run("esdev", run).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                print_error(&err);
                ExitCode::FAILURE
            }
        };
    }

    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let files = test::discover(&root, &config.filters);
    if files.is_empty() {
        eprintln!("no test files found (looked for *.test.js/.mjs/.ts/.tsx/.jsx)");
        return ExitCode::FAILURE;
    }

    let Ok(exe) = std::env::current_exe() else {
        eprintln!("error: cannot find the esdev binary");
        return ExitCode::FAILURE;
    };
    let mut failed = 0usize;
    for file in &files {
        println!("{}", file.strip_prefix(&root).unwrap_or(file).display());
        let status = tokio::process::Command::new(&exe)
            .arg("test")
            .arg(format!("--file={}", file.display()))
            .status()
            .await;
        match status {
            Ok(status) if status.success() => {}
            Ok(_) => failed += 1,
            Err(e) => {
                eprintln!("  cannot run it: {e}");
                failed += 1;
            }
        }
    }
    let total = files.len();
    if failed == 0 {
        println!("\n{total} file{} passed", if total == 1 { "" } else { "s" });
        ExitCode::SUCCESS
    } else {
        println!(
            "\n{failed} of {total} file{} failed",
            if total == 1 { "" } else { "s" }
        );
        ExitCode::FAILURE
    }
}

/// Starts the debugger endpoint asked for on the command line and puts it in the
/// run's config.
///
/// Bound here rather than during parsing, and before the program is loaded: a
/// port already taken should be an error the user sees instead of the program
/// starting and the debugger never arriving.
fn attach_debugger(config: &mut Config, inspect: Option<&InspectConfig>) -> Result<(), String> {
    let Some(inspect) = inspect else {
        return Ok(());
    };
    let entry = match &config.source {
        Source::File(path) => path.clone(),
        Source::Inline(_) => "[eval]".to_string(),
    };
    config.inspector = Some(es_runtime_cli_common::Inspector {
        transport: inspect::start(inspect, &entry)?,
        wait: inspect.wait,
    });
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    // Before anything that could log. Installing a subscriber is a
    // process-global act, so a library crate must not do it. Quiet by default
    // (`warn`); `RUST_LOG` opens it up, e.g. `RUST_LOG=runtime::http=debug`.
    es_runtime_common::telemetry::init_tracing();
    let result = match parse_args() {
        Ok(Command::Run(mut config, inspect)) => {
            match attach_debugger(&mut config, inspect.as_ref()) {
                Ok(()) => es_runtime_cli_common::run("esdev", *config).await,
                Err(err) => Err(err),
            }
        }
        Ok(Command::Watch(config)) => watch::supervise(config).await,
        Ok(Command::Test(config)) => return run_tests(config).await,
        Ok(Command::Build(config)) => {
            let verb = if config.lib { "built" } else { "bundled" };
            match build::build(config).await {
                Ok(written) => {
                    println!("{verb} → {written}");
                    Ok(())
                }
                Err(err) => Err(err),
            }
        }
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
