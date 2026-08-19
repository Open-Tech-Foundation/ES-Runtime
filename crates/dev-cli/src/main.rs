//! `esdev` — the local development binary for the ES-Runtime.
//!
//! `esrun` is the production server runtime: it runs a service and does nothing
//! else, and that narrowness is deliberate — no inspector port, no file
//! watcher, no test discovery, nothing that could weaken the capability model it
//! exists to enforce. The cost of that lands entirely on the developer's inner
//! loop, and `esdev` is the binary that pays it.
//!
//! **It never changes what the JS sees.** Same prelude, same snapshot, same
//! providers, same capability *enforcement* — all of it shared with `esrun`
//! through `es-runtime-cli-common`, so a program cannot behave one way here and
//! another in production. What `esdev` changes is everything *around* a run:
//! watching, restarting, attaching, discovering, reporting, building.
//!
//! **One exception, and it is deliberate (D65): the default grant.** `esdev`
//! starts from every capability, `esrun` from none. Enforcement is the same code
//! either way — what differs is only where a command line with no permission
//! flags starts, because an inner loop that dies on an unnamed capability at
//! every save is the cost D59 put on this binary to avoid. The gap is what
//! `--trace-permissions` closes: it prints the `esrun` line that grants exactly
//! what the run reached for. `esdev start` is narrower still — it spawns the
//! child under `esdev.json`'s `permissions`, so the dev loop runs under the
//! production grant.
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
use es_runtime_cli_common::permissions::{Baseline, Permissions};
use es_runtime_cli_common::{Config, Source};

mod adapter;
mod build;
mod bundler;
mod config;
mod contract;
mod create;
mod css;
mod cssmodules;
mod declarations;
mod devserver;
mod dts;
mod guest;
mod html;
mod inspect;
mod install;
mod plugins;
mod prompt;
mod resolve;
mod staging;
mod start;
mod style;
mod test;
mod trace;
mod transform;
mod types;
mod watch;
use build::{BuildConfig, BuildRequest, ProjectBuild};
use create::{CreateConfig, DEFAULT_TEMPLATE};
use inspect::InspectConfig;
use start::StartConfig;
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
    /// Bundle a module and its dependencies, or the targets a project describes.
    Build(BuildRequest),
    /// Run a module, restarting it when its source changes.
    Watch(WatchConfig),
    /// Discover and run test files.
    Test(TestConfig),
    /// Build the project, run it, and keep both current.
    Start(Box<StartConfig>),
    /// Write a new project from a template.
    Create(CreateConfig),
}

const USAGE: &str = "\
esdev — the local development binary for the ES-Runtime

Runs your program the way esrun will, with the tooling to get it there. Every
flag is `--flag` or `--flag=value`; a value is never a separate argument.

USAGE:
    esdev [options] <file>      Run a module — .js, .mjs, .ts, .tsx, .jsx
    esdev -e=<code>             Run an inline module snippet
    esdev <command> [...]       One of the commands below

COMMANDS:
    create <dir>                Write a new project that already works
    start                       Build, run, and keep both current
    build [entry]               Bundle to deploy, or --lib to publish
    test [filter...]            Run the test files
    upgrade                     Update esdev to the latest release

    Each takes --help: `esdev build --help`.

OPTIONS:
    --watch                     Rerun the program when its source changes
    --inspect[=<addr>]          Serve the Chrome DevTools Protocol (127.0.0.1:9229)
    --inspect-brk[=<addr>]      ...and stop before the first statement
    --trace-permissions         Run it, then print the esrun line it needs
    --install-types             Add the runtime: TypeScript definitions to this
                                project and wire up tsconfig.json
    -h, --help                  Show this help
    -v, --version               Show the version

RUN OPTIONS (esrun's, with one deliberate difference):
    esdev grants every capability by default; esrun grants none. The vocabulary
    and the rules are identical — only the starting point differs, so the inner
    loop needs no flags and a deployment states what it may reach.
    --trace-permissions turns one into the other.

    -A, --allow-all             Grant everything — the default, said outright
    --deny-all                  Run with no host access at all, as esrun does
    --deny-<name>               Deny one capability; repeatable
    --allow-<name>[=<list>]     Grant one back, optionally narrowed; requires
                                --deny-all. <name> is one of: read, write,
                                imports, net, listen, env, run, signals, workers
    --import-policy=<file>      JSON policy for what may be loaded
    -t, --timeout=<ms>          Stop execution after <ms>
    --max-heap=<mb>             Heap ceiling in megabytes
    --env-file=<path>           Load env vars from a .env file
    --env-override              ...and let them override the OS environment
    --shutdown-grace=<ms>       Drain time for in-flight requests on ^C (10000)

TypeScript and JSX are stripped as they load — types erased, never checked. An
import specifier must name the file that exists (`./app.ts`), because
resolution here is esrun's.

esdev is for your machine. It is not a deployment target: ship the artifact and
run it under esrun, which has no development surface to attack.

    Everything esdev does:  https://esrun.opentechf.org/docs/esdev
    Capabilities:           https://esrun.opentechf.org/docs/security
    The debugger:           https://esrun.opentechf.org/docs/esdev/debugging
    TypeScript:             https://esrun.opentechf.org/docs/esdev/typescript
";

const TEST_USAGE: &str = "\
esdev test — run the test files

USAGE:
    esdev test [filter...]      Run every *.test.{js,mjs,ts,tsx,jsx} whose path
                                contains a filter — or all of them, given none
    esdev test --file=<path>    Run exactly one file
    esdev test -h, --help       Show this help

Each file runs in its own process, so one that wedges, exhausts its heap or
calls exit() cannot decide the fate of the others. The file itself is the
entry — it keeps its own path, its module resolution and its TypeScript — and
imports what it uses from runtime:test:

    import { test, assert, assertEquals, assertThrows, assertRejects } from \"runtime:test\";

    test(\"it adds\", () => {
      assertEquals(1 + 1, 2);
    });

Nothing is ambient: there is no global `test`, and a file that calls one fails
with a ReferenceError. Types come from @opentf/esrun-types
(`esdev --install-types`). Exits non-zero if any file fails.

    The API:  https://esrun.opentechf.org/api/test
    The how:  https://esrun.opentechf.org/docs/esdev/test
";

const CREATE_USAGE: &str = "\
esdev create — a project that already works

USAGE:
    esdev create <dir> [options]
                                Write a new project into <dir>
    esdev create --list         List the templates and their modes
    esdev create -h, --help     Show this help

OPTIONS:
    --template=<name>           react (default), api, vanilla or lib
    --mode=<name>               Which shape of it, where it has more than one:
                                react is static (default) or fullstack
    --install[=<manager>]       Install after writing: npm, bun, pnpm or yarn
    --no-install                Write the files and stop
    -y, --yes                   Take every default; never ask
    --force                     Write into a directory that already holds
                                something. It still never replaces a file

What you get is a project that runs and one page — its name, what it was built
with, and the file to edit — with its esdev.json written, its entry named by
the script tag in its index.html, and a permission line that is narrow from the
first run. The templates are baked into this binary, so create works offline
and always writes a project this esdev can build.

On a terminal it asks which template, which mode where there is a choice, and
whether to install. Anywhere else — a pipe, a CI job — it takes the defaults,
installs nothing and says nothing, because a prompt in a script is a script
that hangs. Every question has a flag:

    esdev create my-app --template=api --install=bun
    esdev create my-app --yes

    The templates:  https://esrun.opentechf.org/docs/esdev/create
";

const START_USAGE: &str = "\
esdev start — the dev loop: build, run, rebuild, reload

USAGE:
    esdev start [options]       Build what esdev.json describes, run it, and
                                keep both current

OPTIONS:
    --port=<n>                  The port you open, and it gets that one or
                                fails. Without it: your `listen` grant's port,
                                or 5173 for a frontend project — and any free
                                port if that is taken, printed when it moves
    --no-hot                    Reload the page on a change instead of patching
                                the changed module into it
    --config=<path>             Read this instead of ./esdev.json
    --shutdown-grace=<ms>       How long the server may drain on a restart
    -h, --help                  Show this help

It is `esdev build` on a loop. A dev build differs from a release build in
exactly two ways — process.env.NODE_ENV is \"development\", and nothing is
content-hashed. A build that fails leaves everything running.

The server is yours: `\"start\": { \"run\": \"server\" }` names the target whose
output esdev runs as a child process, under the config's `permissions`, and
restarts with a SIGTERM — the same graceful stop production gets. It is the
same file production runs; nothing wraps it. A project with no server of its
own is served from its output directory instead, with an index.html fallback.

    The dev loop:  https://esrun.opentechf.org/docs/esdev/start
";

const BUILD_USAGE: &str = "\
esdev build — build an application to deploy, or a library to publish

USAGE:
    esdev build                          Every target in esdev.json
    esdev build <entry> [options]        One deployable ES module
    esdev build --lib <srcdir> [options] A publishable library

OPTIONS:
    --config=<path>             Read this instead of ./esdev.json
    --target=<name>             Build one target from the file, not all of them
    --out=<path>                Where to write it. A file for an application
                                (default dist/<entry>.js), a directory for --lib
    --minify                    Minify the output
    --define=<name>=<value>     Replace <name> with <value> at build time.
                                process.env.NODE_ENV defaults to \"production\"
                                for an application, and to nothing for --lib
    --conditions=<list>         Extra `exports` conditions, comma-separated
    --lib                       Build a library: keep the module structure,
                                leave dependencies external, emit .d.ts
    --no-types                  --lib only: skip the .d.ts files
    --dts-bundle[=<entry>]      --lib only: link every declaration into one
                                .d.ts (default entry: <srcdir>/index.ts)
    -h, --help                  Show this help

A PROJECT (esdev.json)
    What a project builds is a property of the project, so it lives in the
    project — one entry per target, because an app that renders on the server
    and hydrates in the browser is two bundles a command line cannot describe:

        {
          \"targets\": {
            \"server\": { \"entry\": \"src/server.ts\", \"out\": \"dist/server.js\" },
            \"web\":    { \"entry\": \"index.html\", \"outdir\": \"dist\" }
          }
        }

    An .html entry is a different kind of build: the tags in the document are
    the inputs, and what is written out is the same document pointing at the
    hashed results. A flag beats the file; naming an entry ignores it entirely.
    esrun never reads esdev.json — the grant a service runs under belongs on
    the command that deployed it.

AN APPLICATION vs A LIBRARY
    A bundle has no imports left to resolve, so production needs no
    --allow-imports. A library is an input to somebody else's build, so --lib
    makes none of that build's decisions: module structure is kept file for
    file, dependencies stay external, nothing is defined, no condition is
    asserted, and a .d.ts is emitted from the annotations the source carries —
    derived, never inferred, so an unannotated export fails the build.

    Targets, an HTML entry, --lib and --dts-bundle in full:
        https://esrun.opentechf.org/docs/esdev/build
    The plugin API (runtime:build):
        https://esrun.opentechf.org/api/build
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
        if first == "start" {
            return parse_start(argv).map(|config| Command::Start(Box::new(config)));
        }
        if first == "create" {
            return parse_create(argv).map(Command::Create);
        }
        if first == "upgrade" {
            if let Some(extra) = argv.next() {
                return Err(format!(
                    "esdev upgrade takes no arguments; got {extra}.\n\n\
                     It replaces this binary with the newest esdev release."
                ));
            }
            // The same machinery `esrun upgrade` runs, on a thread of its own —
            // self_update drives a blocking HTTP runtime, and dropping that from
            // inside this `#[tokio::main]` context panics.
            es_runtime_cli_common::upgrade::run_and_exit("esdev", env!("CARGO_PKG_VERSION"));
        }
    }

    let mut options = RunOptions::default();
    let mut permissions = Permissions::new(Baseline::Everything);
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
                        extensions: guest::extensions(),
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
                        extensions: guest::extensions(),
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

/// Parses `esdev build [entry] [options]`.
fn parse_build(args: impl Iterator<Item = String>) -> Result<BuildRequest, String> {
    let mut sources: Vec<String> = Vec::new();
    let mut out = None;
    let mut minify = false;
    let mut lib = false;
    let mut no_types = false;
    let mut dts_bundle: Option<Option<String>> = None;
    let mut conditions = Vec::new();
    let mut defines = Vec::new();
    let mut config_path: Option<String> = None;
    let mut target: Option<String> = None;
    for arg in args {
        let (flag, value) = split_flag_value(&arg);
        match flag {
            "-h" | "--help" => {
                reject_value(flag, value)?;
                println!("{BUILD_USAGE}");
                std::process::exit(0);
            }
            "--config" => config_path = Some(require_value(flag, value)?.to_string()),
            "--target" => target = Some(require_value(flag, value)?.to_string()),
            "--out" => out = Some(require_value(flag, value)?.to_string()),
            "--lib" => {
                reject_value(flag, value)?;
                lib = true;
            }
            "--no-types" => {
                reject_value(flag, value)?;
                no_types = true;
            }
            // The value is optional: with none, the entry is `index` in the
            // source directory, which is where a package's `.` export points
            // in almost every library that has one.
            "--dts-bundle" => dts_bundle = Some(value.map(str::to_string)),
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
    // A project build and a command-line build are the same build with its
    // settings in different places, so asking for both is ambiguous rather than
    // additive: which of the two named the entry?
    if !sources.is_empty() {
        if let Some(name) = &target {
            return Err(format!(
                "--target={name} selects a target from {}, and {} was named on the \
                 command line.\n\n\
                 Build the one: `esdev build --target={name}`, or `esdev build {}`.",
                config::FILE_NAME,
                sources[0],
                sources[0]
            ));
        }
        if let Some(path) = &config_path {
            return Err(format!(
                "--config={path} describes what to build, and {} was named on the \
                 command line as well.\n\n\
                 Drop one: `esdev build --config={path}` builds the targets in the \
                 file, `esdev build {}` builds that entry.",
                sources[0], sources[0]
            ));
        }
    }
    if lib && (config_path.is_some() || target.is_some()) {
        return Err(format!(
            "--lib builds a source directory named on the command line; {} \
             describes applications.\n\n\
             A library's shape is its source tree, and the four decisions --lib \
             makes are the ones a consumer's build makes for it.",
            config::FILE_NAME
        ));
    }

    if sources.is_empty() && !lib {
        // The config is only *looked* for when there is nothing to build
        // otherwise, so a project that has one can still build a scratch entry
        // by naming it.
        if let Some(project) = config::load(config_path.as_deref())? {
            if let Some(path) = &out {
                return Err(format!(
                    "--out={path} names one file, and a project build writes what each \
                     of its targets says.\n\n\
                     Where a target's output goes is `out` or `outdir` in {}.",
                    config::FILE_NAME
                ));
            }
            return Ok(BuildRequest::Project(Box::new(ProjectBuild {
                project: std::sync::Arc::new(project),
                targets: target.map(|name| vec![name]),
                minify,
                defines,
                conditions,
                dev: None,
            })));
        }
        if let Some(name) = target {
            return Err(format!(
                "--target={name} needs a {0}, and there is none here.\n\n\
                 A target is one thing the project builds; {0} is where they are \
                 described.",
                config::FILE_NAME
            ));
        }
    }
    if sources.is_empty() {
        return Err(format!(
            "missing {} argument\n\n{BUILD_USAGE}",
            if lib {
                "source directory"
            } else {
                "entry (or an esdev.json describing what this project builds)"
            }
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
    if dts_bundle.is_some() && !lib {
        return Err("--dts-bundle only means something with --lib.\n\n\
             An application build emits no declarations to link: a bundle is deployed \
             and run, not imported and type-checked."
            .to_string());
    }
    if dts_bundle.is_some() && no_types {
        return Err("--dts-bundle and --no-types ask for opposite things.\n\n\
             One links every declaration into a file; the other emits none."
            .to_string());
    }
    // Resolved here rather than in the build, so a default that is not there is
    // an argument error naming both what was looked for and the way to say it.
    let dts_bundle = match dts_bundle {
        None => None,
        Some(Some(entry)) => Some(entry),
        Some(None) => {
            let found = ["ts", "tsx", "mts", "cts"]
                .iter()
                .map(|extension| std::path::Path::new(&source).join(format!("index.{extension}")))
                .find(|candidate| candidate.is_file());
            match found {
                Some(entry) => Some(entry.display().to_string()),
                None => {
                    return Err(format!(
                        "--dts-bundle found no index.ts in {source}.\n\n\
                         One declaration file is built from one entry. Name it: \
                         --dts-bundle={source}/main.ts."
                    ));
                }
            }
        }
    };
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
    Ok(BuildRequest::Single(Box::new(BuildConfig {
        source,
        out,
        out_dir: None,
        dev: false,
        platform: config::Platform::Server,
        assets: Vec::new(),
        root: None,
        minify,
        conditions,
        defines,
        lib,
        types: !no_types,
        dts_bundle,
        // A command line names an entry, not a project, so there is no
        // esdev.json to have declared any.
        plugins: Vec::new(),
    })))
}

/// Parses `esdev create <dir> [options]`.
fn parse_create(args: impl Iterator<Item = String>) -> Result<CreateConfig, String> {
    let mut dirs: Vec<String> = Vec::new();
    // `None` means "not said", which on a terminal becomes a question and
    // away from one becomes the default. A flag is always an answer.
    let mut template: Option<String> = None;
    let mut mode: Option<String> = None;
    let mut install: Option<Option<String>> = None;
    let mut force = false;
    for arg in args {
        let (flag, value) = split_flag_value(&arg);
        match flag {
            "-h" | "--help" => {
                reject_value(flag, value)?;
                println!("{CREATE_USAGE}");
                std::process::exit(0);
            }
            "--list" => {
                reject_value(flag, value)?;
                print!("{}", create::list());
                std::process::exit(0);
            }
            "--template" => template = Some(require_value(flag, value)?.to_string()),
            "--mode" => mode = Some(require_value(flag, value)?.to_string()),
            // `--install` alone means "with npm"; `--install=bun` names one.
            "--install" => {
                install = Some(Some(value.unwrap_or(create::DEFAULT_MANAGER).to_string()));
            }
            "--no-install" => {
                reject_value(flag, value)?;
                install = Some(None);
            }
            // The conventional spelling of "do not ask me anything": take every
            // default rather than prompting, even on a terminal.
            "-y" | "--yes" => {
                reject_value(flag, value)?;
                template.get_or_insert_with(|| DEFAULT_TEMPLATE.to_string());
                // Left as `None` on purpose: the default *mode* depends on
                // which template this turned out to be, and only `create` knows
                // that. What `--yes` promises is that nothing is asked, and an
                // unsaid mode away from a prompt is already the default.
                install.get_or_insert(None);
            }
            "--force" => {
                reject_value(flag, value)?;
                force = true;
            }
            flag if flag.starts_with('-') && flag.len() > 1 => {
                return Err(format!("unknown option: {flag}\n\n{CREATE_USAGE}"));
            }
            dir => dirs.push(dir.to_string()),
        }
    }
    if dirs.is_empty() {
        return Err(format!("missing directory argument\n\n{CREATE_USAGE}"));
    }
    if dirs.len() > 1 {
        return Err(format!(
            "esdev create writes one project; got {} directories.\n\n{CREATE_USAGE}",
            dirs.len()
        ));
    }
    Ok(CreateConfig {
        dir: dirs.remove(0),
        template,
        mode,
        force,
        install,
    })
}

/// Parses `esdev start [options]`.
fn parse_start(args: impl Iterator<Item = String>) -> Result<StartConfig, String> {
    let mut config_path: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut hot = true;
    let mut options = RunOptions::default();
    for arg in args {
        let (flag, value) = split_flag_value(&arg);
        // One shared flag applies here, and it is the one a restart uses. The
        // rest shape a *run*, and `start` does not run your program — it runs
        // the target's output as a child, under what esdev.json grants. Taking
        // them and dropping them would be a flag somebody keeps passing and
        // keeps believing, so they fall through to the error below.
        if flag == "--shutdown-grace" {
            options.try_flag(flag, value)?;
            continue;
        }
        match flag {
            "-h" | "--help" => {
                reject_value(flag, value)?;
                println!("{START_USAGE}");
                std::process::exit(0);
            }
            "--no-hot" => {
                reject_value(flag, value)?;
                hot = false;
            }
            "--config" => config_path = Some(require_value(flag, value)?.to_string()),
            "--port" => {
                let given = require_value(flag, value)?;
                port =
                    Some(given.parse::<u16>().map_err(|_| {
                        format!("--port={given} is not a port number (1 to 65535).")
                    })?);
            }
            flag if RunOptions::is_shared_flag(flag) => {
                return Err(format!(
                    "{flag} shapes a run, and `esdev start` does not run your program — it \
                     builds what {} describes and runs the output as a child process, under \
                     that file's `permissions`.\n\n\
                     `esdev <file> {flag}=…` takes it, and what the child may reach is \
                     `permissions` in that file.",
                    config::FILE_NAME
                ));
            }
            flag => return Err(format!("unknown option: {flag}\n\n{START_USAGE}")),
        }
    }
    let mut project = config::load(config_path.as_deref())?.ok_or_else(|| {
        format!(
            "esdev start needs a {0}, and there is none here.\n\n\
             It describes what this project builds and what to run:\n\n  \
             {{ \"targets\": {{ \"server\": {{ \"entry\": \"src/server.ts\", \"out\": \"dist/server.js\" }} }},\n    \
             \"start\": {{ \"run\": \"server\" }} }}\n\n\
             See `esdev build --help` for the rest of {0}.",
            config::FILE_NAME
        )
    })?;
    // A flag beats the file, the same way it does for a build.
    if let Some(port) = port {
        project.start.port = Some(port);
    }
    Ok(StartConfig {
        project,
        hot,
        grace: options.shutdown_grace,
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
        // Nothing is added to the file. It is an ordinary run of an ordinary
        // module — the same transform any `.ts` gets — and the test API comes
        // from the `runtime:test` the file imported. What makes this a *test*
        // run is what `finish()` finds afterwards, not anything done to the
        // source.
        let run = Config {
            source: Source::File(file.clone()),
            args: Vec::new(),
            capabilities: es_runtime_common::CapabilitySet::all(),
            scopes: std::collections::HashMap::new(),
            options: RunOptions::default(),
            transform: Some(std::sync::Arc::new(TypeStripper)),
            extensions: guest::extensions(),
            observer: None,
            inspector: None,
        };
        return match es_runtime_cli_common::run("esdev", run).await {
            Ok(()) => guest::test::finish(),
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
        Ok(Command::Build(request)) => build::run(request).await,
        Ok(Command::Start(config)) => start::start(*config).await,
        Ok(Command::Create(config)) => match create::create(&config) {
            Ok(report) => {
                print!("{report}");
                Ok(())
            }
            Err(err) => Err(err),
        },
        Err(err) => Err(err),
    };
    match result {
        // Whatever the command line called this run, a program that imported
        // `runtime:test` ran tests, and their tally decides the exit code. One
        // that did not prints nothing and succeeds — which is every other run.
        Ok(()) => guest::test::finish(),
        Err(err) => {
            print_error(&err);
            ExitCode::FAILURE
        }
    }
}
