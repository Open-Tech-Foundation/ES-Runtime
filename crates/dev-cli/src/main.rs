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
mod alias;
mod assets;
mod bidi;
mod browser;
mod browser_run;
mod build;
mod bundler;
mod check;
mod commonjs;
mod config;
mod contract;
mod coverage;
mod create;
mod css;
mod cssmodules;
mod declarations;
mod devserver;
mod dom;
mod dts;
mod global_setup;
mod guest;
mod html;
mod init;
mod inline_snapshot;
mod inspect;
mod install;
mod jsx;
mod module_mocks;
mod module_url;
mod plugins;
mod preview;
mod prompt;
mod related;
mod report;
mod resolve;
mod screenshot;
mod settings;
mod staging;
mod start;
mod style;
mod tags;
mod tailwind;
mod test;
mod trace;
mod transform;
mod types;
mod watch;
mod watch_keys;
use build::{BuildConfig, BuildRequest, ProjectBuild};
use config::TestIsolation;
use create::{CreateConfig, DEFAULT_TEMPLATE};
use init::InitConfig;
use inspect::InspectConfig;
use preview::PreviewConfig;
use start::StartConfig;
use test::TestConfig;
use trace::PermissionTrace;
use transform::TypeStripper;
use watch::WatchConfig;

/// What the command line asked for.
enum Command {
    /// Run a module. Boxed rather than making the build variant carry its
    /// weight. What the command line decided — what runs, under which grant —
    /// and not yet the project's settings, which `main` applies once the
    /// whole command line has been found to make sense. The debugger endpoint
    /// travels beside it for the same reason: binding a port is something
    /// `main` does, not the parser. Last, the `--config` naming the project.
    Run(Box<settings::Run>, Option<InspectConfig>, Option<String>),
    /// Bundle a module and its dependencies, or the targets a project describes.
    Build(BuildRequest),
    /// Run a module, restarting it when its source changes.
    Watch(WatchConfig),
    /// Discover and run test files. Boxed, like `Run`, for its size.
    Test(Box<TestConfig>),
    /// Build the project, run it, and keep both current.
    Start(Box<StartConfig>),
    /// Write a new project from a template.
    Create(CreateConfig),
    /// Start a bare project, or adopt an existing directory.
    Init(InitConfig),
    /// Serve what a build wrote, the way it will be served.
    Preview(PreviewConfig),
    /// Typecheck the project with its own TypeScript.
    Check(Vec<String>),
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
    init [dir]                  Start a bare project, or adopt this one
    start                       Build, run, and keep both current
    build [entry]               Bundle to deploy, or --lib to publish
    test [filter...]            Run the test files
    bench [filter...]           Run the benchmark files (*.bench.*)
    check [args...]             Typecheck the project with its own tsc
    preview                     Serve the built output before deploying it
    upgrade [--dry-run]         Update esdev to the latest release

    Each takes --help: `esdev build --help`.

OPTIONS:
    --watch                     Rerun the program when its source changes
    --inspect[=<addr>]          Serve the Chrome DevTools Protocol (127.0.0.1:9229)
    --inspect-brk[=<addr>]      ...and stop before the first statement
    --trace-permissions         Run it, then print the esrun line it needs
    --config=<path>             Read this instead of ./esdev.json
    --install-types             Add the runtime: TypeScript definitions to this
                                project and wire up tsconfig.json
    -h, --help                  Show this help
    -v, -V, --version         Show the version

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

TypeScript and JSX are stripped as they load — types erased, never checked.
Imports resolve the way `esdev build` resolves them: `./util` finds util.ts,
a directory finds its index, and `./util.js` finds util.ts. esrun resolves
only what the module spec says, so ship a build rather than the source.

esdev is for your machine. It is not a deployment target: ship the artifact and
run it under esrun, which has no development surface to attack.

    Everything esdev does:  https://esrun.opentechf.org/esdev
    Capabilities:           https://esrun.opentechf.org/docs/security
    The debugger:           https://esrun.opentechf.org/esdev/debugging
    TypeScript:             https://esrun.opentechf.org/esdev/typescript
";

const TEST_USAGE: &str = "\
esdev test — run the test files

USAGE:
    esdev test [filter...]      Run every *.test.* / *.spec.* whose path
                                contains a filter — or all of them, given none
    esdev test --file=<path>    Run exactly one file
    esdev bench [filter...]     Run the *.bench.* / *.benchmark.* files the same
                                way, one at a time, with `bench` in the context
    esdev test -h, --help       Show this help

OPTIONS:
    --config=<path>             Read this instead of ./esdev.json
    --jobs=<n>                  How many files run at once. The default is the
                                machine's parallelism, at most 8 — each file is
                                a process holding a V8 heap
    --isolation=<mode>          process (default), or none to run all selected
                                files serially in one process and retain caches
    --watch                     Run them again whenever a source file changes;
                                on a terminal, press h for the keys
    --dom                       Install esdev's test-only DOM globals
    --browser[=<name>]          Run the files in a real browser over WebDriver
                                BiDi: auto (the default) takes the first of
                                chrome, chromium, firefox, edge that can be
                                driven; all but firefox need their matching
                                driver on PATH. Nothing is downloaded.
                                --browser=firefox,chrome runs every file in
                                each, one browser after the other
    --headed                    Show the browser window instead of running it
                                headless, to watch a test run
    -t, --test-name-pattern=<re>
                                Run only the tests whose full name matches;
                                the rest are counted as skipped
    --test-skip-pattern=<re>    Skip the tests whose full name matches
    --tags-filter=<expr>        Run only the tests whose tags match: tag names
                                with and/&&, or/||, not/!, parentheses and *.
                                Repeatable; a test must match every one
    --list-tags[=json]          Print the tags test.tags defines, and exit
    --max-concurrency=<n>       How many test.concurrent cases in a file run at
                                once (5 by default)
    --typecheck                 Also run the project's tsc --noEmit, which is
                                where expectTypeOf and assertType fail
    --bail[=<n>]                Stop after <n> failed tests (1 by default); the
                                rest are counted as not run
    --inspect[=<addr>]          Serve a debugger for each file in turn, one file
                                at a time (default 127.0.0.1:9229)
    --inspect-brk[=<addr>]      ...and stop before each file's first statement
    --detect-async-leaks        Fail a file that leaves timers, servers or other
                                work pending after its tests, naming where each
                                was started
    --coverage                  Measure which statements, branches, functions
                                and lines the tests ran, and report it
    --changed[=<since>]         Run only the test files that reach what git
                                says changed: uncommitted, or since a commit
                                or branch
    --related <file>...         Run only the test files that import these
                                source files, directly or not
    --shard=<index>/<count>     Run one part of the files, to split a suite
                                across machines: --shard=1/3 is the first of
                                three
    --randomize                 Run files, and tests within their groups, in a
                                shuffled order; the seed is printed
    --seed=<n>                  Shuffle with this seed, to repeat an order
    --repeats=<n>               Run every test <n> more times; it fails if any
                                run fails
    --list                      Name the tests each file registers, running
                                none
    --setup=<path>              Import this before each test file. Repeatable
    --global-setup=<path>       Run this module's setup once before the files,
                                and its teardown after. Repeatable
    --timeout=<ms>              Stop a file that takes longer, and fail it
    --reporter=<fmt>            human (default), json (one object per line),
                                junit, tap or dots
    --reporter-outfile=<path>   Write that report to a file; the terminal keeps
                                the human one
    -u, --update-snapshots       Write new and changed snapshots
    --ci                         Require every snapshot to be pre-existing
    --full-diff                  Do not truncate a large snapshot diff
    --deny-all                  Rehearse the production grant: run each file
                                with no host access, granting back only what
                                --allow-<name>[=<list>] names
    --deny-<name>               Deny one capability for each file; repeatable
    --allow-<name>[=<list>]     Grant one back, optionally narrowed; requires
                                --deny-all. <name> is one of: read, write,
                                imports, net, listen, env, run, signals, workers

`setup`, `globalSetup`, `timeout`, `jobs`, `isolation`, `reporter`, `browser`,
`coverage`, `tags`, `strictTags` and `maxConcurrency` are also esdev.json keys,
under \"test\" — the rest (`--update-snapshots`, `--ci`, `--full-diff` and the
permission flags) are flags only: they decide a single run, not the project:

    { \"test\": { \"setup\": [\"./test/setup.ts\"], \"timeout\": 5000,
                \"jobs\": 4, \"isolation\": \"process\", \"reporter\": \"json\" } }

A flag beats the file.

By default each file runs in its own process, so one that wedges, exhausts its
heap or calls exit() cannot decide the fate of the others. Files run in parallel,
and each one's output is held and printed whole when it finishes — --jobs=1 runs
them one at a time and lets each write straight to the terminal. --isolation=none
runs all selected files serially in one process, retaining module caches but
sharing global state and failure fate. The file itself is the entry — it keeps
its own path, its module resolution and its TypeScript — and imports what it uses
from runtime:test:

    import { test, expect } from \"runtime:test\";

    test(\"it adds\", () => {
      expect(1 + 1).toBe(2);
    });

Also exported: describe (and it/suite), the before*/after* hooks,
assert/assertEquals/assertThrows/assertRejects, mock (mock.fn, mock.spyOn,
mock.when, mock.module) and clock (clock.freeze, clock.advance). test and
describe carry .skip, .only, .todo, .each(table), .skipIf(cond) and
.runIf(cond), and test.extend(...) adds fixtures a test names in its first
parameter.

Nothing is ambient: there is no global `test`, and a file that calls one fails
with a ReferenceError. Types come from @opentf/esrun-types
(`esdev --install-types`). Exits non-zero if any file fails.

    The API:  https://esrun.opentechf.org/api/test
    The how:  https://esrun.opentechf.org/esdev/test
";

const PREVIEW_USAGE: &str = "\
esdev preview — serve the built output, the way it will be served

USAGE:
    esdev preview [options]     Serve what `esdev build` wrote

OPTIONS:
    --dir=<path>                The directory to serve. Without it: the site
                                esdev.json describes
    --port=<n>                  The port to open (default 4173, or any free one
                                if that is taken)
    --config=<path>             Read this instead of ./esdev.json
    -h, --help                  Show this help

It serves; it does not build. The dev loop's build is not the one that ships —
NODE_ENV is \"development\" there and nothing is content-hashed — so this is where
a release build gets looked at before it is deployed. Missing paths that look
like routes fall back to index.html, as they must for a client-side router.

Loopback only, like every endpoint esdev opens. A project whose output is a
server bundle has nothing to serve: run it under esrun, which is what will run
it in production.

    Building:  https://esrun.opentechf.org/esdev/build
";

const CHECK_USAGE: &str = "\
esdev check — typecheck the project with its own TypeScript

USAGE:
    esdev check [args...]     Run tsc --noEmit through the project's package
                              manager, passing args through untouched
    esdev check -h, --help    Show this help

Finds the project's package manager the way --install-types does — the
packageManager field, then the lockfile, then what is installed — and runs
its tsc, so the version checked with is the version the project uses.
Output is tsc's own; a failure adds only the exit code.

    Type definitions:  https://esrun.opentechf.org/esdev/typescript
";

const UPGRADE_USAGE: &str = "\
esdev upgrade — update esdev to the latest release

USAGE:
    esdev upgrade               Replace this binary with the newest release
    esdev upgrade --dry-run     Say whether a newer release exists, and change
                                nothing
    esdev upgrade -h, --help    Show this help

It finds the latest release, downloads it, and replaces the running binary
in place — the same machinery `esrun upgrade` runs. There is nothing else
to configure, and no other argument to give it.
";

const INIT_USAGE: &str = "\
esdev init — start a bare project, or adopt this one

USAGE:
    esdev init [dir] [options]
                                Start a bare project in <dir> (. for here),
                                or adopt the project in it
    esdev init -h, --help       Show this help

OPTIONS:
    --name=<name>             The package name. Asked with the directory's
                              name as the default; new projects only
    --language=<name>         js or ts. Asked outright; new projects only
    --entry=<path>            The file to adopt. Asked with the detected
                              entry as the default; existing projects only
    --install[=<manager>]     Install after writing: npm, bun, pnpm or yarn.
                              New projects only
    --no-install              Write the files and stop
    -y, --yes                 Take every default; never ask
    --force                   Write a new project among what is there. It
                              still never replaces a file

An empty directory gets the bare minimal setup — a greeting server, built
and run by esdev. A directory with a project in it gets the one file it is
missing: a working esdev.json, plus its types installed. An esdev.json
already there is refused outright; so are the other flow's flags, which name
what to drop rather than being quietly ignored.

    The templates:  https://esrun.opentechf.org/esdev/create
";

const CREATE_USAGE: &str = "\
esdev create — a project that already works

USAGE:
    esdev create <dir> [options]
                                Write a new project into <dir>
    esdev create --list         List the templates and their modes
    esdev create -h, --help     Show this help

OPTIONS:
    --template=<name>           react (default), api, vanilla, micro-ui, lib,
                                spa, fullstack, docs or library
    --mode=<name>               Which shape of it, where it has more than one:
                                react is static (default) or fullstack
    --language=<name>           OTF templates only: js (default) or ts
    --styling=<name>            css or tailwind: react, vanilla and micro-ui
                                default to css, spa and fullstack to tailwind
    --blog, --no-blog           docs only: keep the demo blog (default) or not
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

On a terminal it asks which template, which mode where there is a choice, the
axes the template takes (language, styling, blog), and whether to install. Anywhere else — a pipe, a CI job — it takes the defaults,
installs nothing and says nothing, because a prompt in a script is a script
that hangs. Every question has a flag:

    esdev create my-app --template=api --install=bun
    esdev create my-app --yes

    The templates:  https://esrun.opentechf.org/esdev/create
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
    --allow-read=<paths>        Also watch explicitly granted read paths
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

    The dev loop:  https://esrun.opentechf.org/esdev/start
";

const BUILD_USAGE: &str = "\
esdev build — build an application to deploy, or a library to publish

USAGE:
    esdev build                          Every target in esdev.json
    esdev build <entry> [options]        One deployable .js/.mjs/.ts/.tsx/.jsx module
    esdev build <entry.css> [options]    One bundled stylesheet
    esdev build --lib <srcdir> [options] A publishable library

OPTIONS:
    --config=<path>             Read this instead of ./esdev.json
    --target=<name>             Build one target from the file, not all of them
    --out=<path>                Where to write it. A file for an application
                                (default dist/<entry>.js), a directory for --lib
    --minify                    Minify the output
    --sourcemap[=<kind>]        Write source maps: a .map beside the output, or
                                =inline in the file, or =hidden for neither
    --alias=<name>=<path>       Rewrite the start of a specifier. Repeatable
    --define=<name>=<value>     Replace <name> with <value> at build time.
                                process.env.NODE_ENV defaults to \"production\"
                                for an application, and to nothing for --lib
    --conditions=<list>         Extra `exports` conditions, comma-separated
    --lib                       Build a library: keep the module structure,
                                leave dependencies external, emit .d.ts
    --format=<list>             --lib only: the module systems to write.
                                esm (default), cjs, or esm,cjs
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

    Each of --lib, --format, --no-types and --dts-bundle is also a target key
    — \"lib\", \"format\", \"types\", \"dts-bundle\" — so a library is describable in
    esdev.json, where its \"assets\" (the README and LICENSE a package ships) can
    be named. A flag and a key are two spellings of one build.

    A library may also be published for consumers who are not on this runtime.
    --format=esm,cjs writes both trees into one directory — dist/**.js with a
    .d.ts, dist/**.cjs with a .d.cts — which a dual `exports` map names. The
    types go *inside* each condition; a \"types\" key beside them matches first
    and hands a require() the ES module's declarations:

        \"exports\": {
          \".\": {
            \"import\":  { \"types\": \"./dist/index.d.ts\",  \"default\": \"./dist/index.js\" },
            \"require\": { \"types\": \"./dist/index.d.cts\", \"default\": \"./dist/index.cjs\" }
          }
        }

    Targets, an HTML entry, --lib and --dts-bundle in full:
        https://esrun.opentechf.org/esdev/build
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
            return parse_test(argv).map(|config| Command::Test(Box::new(config)));
        }
        // The same runner over the benchmark files, one file at a time so
        // one's processes are not the noise in another's numbers.
        if first == "bench" {
            return parse_test(argv).map(|mut config| {
                config.run.bench = true;
                config.jobs = Some(1);
                Command::Test(Box::new(config))
            });
        }
        if first == "start" {
            return parse_start(argv).map(|config| Command::Start(Box::new(config)));
        }
        if first == "create" {
            return parse_create(argv).map(Command::Create);
        }
        if first == "init" {
            return parse_init(argv).map(Command::Init);
        }
        if first == "preview" {
            return parse_preview(argv).map(Command::Preview);
        }
        if first == "check" {
            let args: Vec<String> = argv.collect();
            if args.len() == 1 && (args[0] == "-h" || args[0] == "--help") {
                println!("{CHECK_USAGE}");
                std::process::exit(0);
            }
            return Ok(Command::Check(args));
        }
        if first == "upgrade" {
            if let Some(extra) = argv.next() {
                let (flag, value) = split_flag_value(&extra);
                if value.is_none() {
                    if flag == "-h" || flag == "--help" {
                        println!("{UPGRADE_USAGE}");
                        std::process::exit(0);
                    }
                    // The read half of the upgrade: the same release listing,
                    // stopping at the comparison instead of self-replacing.
                    if flag == "--dry-run" {
                        es_runtime_cli_common::upgrade::check_and_exit(
                            "esdev",
                            env!("CARGO_PKG_VERSION"),
                        );
                    }
                }
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
    let mut config_path: Option<String> = None;
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
            "--config" => config_path = Some(require_value(flag, value)?.to_string()),
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
                    Box::new(settings::Run {
                        source: Source::Inline(code),
                        args: rest,
                        capabilities: permissions.resolve()?,
                        scopes: permissions.scopes()?,
                        options,
                        stripper: TypeStripper::new(),
                        extensions: guest::extensions(),
                        // The deploy line is printed with the entry as it was
                        // named, so it is one a reader can copy. For `-e` there
                        // is nothing to name, and the placeholder says so.
                        observer: permission_trace(tracing_permissions, "-e=<code>"),
                    }),
                    inspect,
                    config_path,
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
                    Box::new(settings::Run {
                        source: Source::File(path.to_string()),
                        args: rest,
                        capabilities: permissions.resolve()?,
                        scopes: permissions.scopes()?,
                        options,
                        stripper: TypeStripper::new(),
                        extensions: guest::extensions(),
                        observer: permission_trace(tracing_permissions, path),
                    }),
                    inspect,
                    config_path,
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

/// Why the working directory looks like an OTF Web project, if it does.
///
/// `esdev create` scaffolds these alongside its own templates, and `esdev
/// build` / `esdev start` are meaningless there — so the missing-`esdev.json`
/// errors consult this first and name `otfw` instead of reading as a
/// misconfiguration. `None` anywhere else, including when the directory
/// cannot be read, so the ordinary errors stand.
fn otfw_project_reason() -> Option<String> {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| config::otfw_reason(&cwd))
}

/// Parses `esdev build [entry] [options]`.
fn parse_build(args: impl Iterator<Item = String>) -> Result<BuildRequest, String> {
    let mut sources: Vec<String> = Vec::new();
    let mut out = None;
    let mut minify = false;
    let mut lib = false;
    let mut formats: Vec<build::Format> = Vec::new();
    let mut no_types = false;
    let mut dts_bundle: Option<Option<String>> = None;
    let mut conditions = Vec::new();
    let mut defines = Vec::new();
    let mut alias: Vec<(String, String)> = Vec::new();
    let mut sourcemap: Option<String> = None;
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
            "--format" => {
                for name in require_value(flag, value)?.split(',') {
                    let name = name.trim();
                    let Some(format) = build::Format::parse(name) else {
                        return Err(format!(
                            "{flag}={} is not a module system this writes — esm or cjs.\n\n\
                             Both at once is a comma: --format=esm,cjs.",
                            value.unwrap_or_default()
                        ));
                    };
                    if !formats.contains(&format) {
                        formats.push(format);
                    }
                }
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
            "--sourcemap" => {
                let kind = value.unwrap_or("external");
                if !matches!(kind, "external" | "inline" | "hidden") {
                    return Err(format!(
                        "--sourcemap={kind} is not a shape of source map.\n\n  \
                         --sourcemap           a .map beside the output\n  \
                         --sourcemap=inline    a data URL in the file itself\n  \
                         --sourcemap=hidden    written, with nothing pointing at it"
                    ));
                }
                sourcemap = Some(kind.to_string());
            }
            "--alias" => {
                let pair = require_value(flag, value)?;
                let (find, to) = pair.split_once('=').ok_or_else(|| {
                    format!(
                        "{flag}={pair} is not a rewrite — write --alias=<name>=<path>, \
                         e.g. --alias=@=./src."
                    )
                })?;
                if find.trim().is_empty() {
                    return Err(format!(
                        "{flag}={pair} has no name to rewrite.\n\n\
                         An alias rewrites the start of a specifier: --alias=@=./src."
                    ));
                }
                // Absolute from here, against the working directory the path was
                // typed in — the same rule the config file follows against the
                // project directory, and for the same reason.
                let is_path = to.starts_with("./")
                    || to.starts_with("../")
                    || std::path::Path::new(to).is_absolute();
                let to = if is_path {
                    std::env::current_dir()
                        .map_err(|e| format!("cannot read working directory: {e}"))?
                        .join(to)
                        .to_string_lossy()
                        .into_owned()
                } else {
                    to.to_string()
                };
                alias.push((find.to_string(), to));
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

    // What the flags do to a target, and the flags only a library build reads
    // — which a project states per target, so on a project build they would
    // mean nothing. Refused by name rather than accepted and dropped.
    let target_flags = settings::TargetFlags {
        minify,
        sourcemap: sourcemap.clone(),
        defines: defines.clone(),
        conditions: conditions.clone(),
    };
    let library_flag = [
        (!formats.is_empty(), "--format"),
        (no_types, "--no-types"),
        (dts_bundle.is_some(), "--dts-bundle"),
    ]
    .into_iter()
    .find_map(|(given, flag)| given.then_some(flag));

    if sources.is_empty() && !lib {
        // The targets are only *built* when there is nothing named to build
        // otherwise, so a project that has them can still build a scratch
        // entry by naming it.
        let settings = settings::Settings::load(config_path.as_deref())?;
        if settings.has_project {
            if let Some(path) = &out {
                return Err(format!(
                    "--out={path} names one file, and a project build writes what each \
                     of its targets says.\n\n\
                     Where a target's output goes is `out` or `outdir` in {}.",
                    config::FILE_NAME
                ));
            }
            if let Some(flag) = library_flag {
                return Err(format!(
                    "{flag} shapes a library build, and a project build's targets say \
                     whether they are libraries in {}.\n\n\
                     Put it on the target: \"lib\": true, \"format\": [\"esm\", \"cjs\"], \
                     \"types\": false, \"dts-bundle\": \"src/index.ts\".",
                    config::FILE_NAME
                ));
            }
            return Ok(BuildRequest::Project(Box::new(ProjectBuild {
                settings: std::sync::Arc::new(
                    settings.with_alias(&alias).with_build(&target_flags),
                ),
                targets: target.map(|name| vec![name]),
                dev: None,
            })));
        }
        // No esdev.json — but an OTF Web project was never going to have one.
        // Refusing with the missing entry is a dead end there; name the
        // toolchain its scripts call instead.
        if let Some(reason) = otfw_project_reason() {
            return Err(format!(
                "this project builds with otfw, not `esdev build`.\n\n\
                 {reason}, and there is no esdev.json describing targets for this toolchain. \
                 Build it the project's way:\n\n  npm run build"
            ));
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
    if !alias.is_empty() && lib {
        return Err("--alias is a bundling rule, and --lib does not bundle.\n\n\
             A published module keeps the specifier its source wrote, and the \
             build that consumes it resolves that — so an alias applied here \
             would ship a package whose imports work under this toolchain and \
             nowhere else."
            .to_string());
    }
    if !formats.is_empty() && !lib {
        return Err("--format only means something with --lib.\n\n\
             An application build's output is loaded by esrun, which loads ES \
             modules and nothing else (D22). A library is an input to somebody \
             else's build, and that build may still be a CommonJS one."
            .to_string());
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
    // The project around the entry, when there is one: its `jsx`, `alias` and
    // `plugins` describe the source tree the entry is in. Its targets do not
    // apply — this build is the one the command line describes.
    let settings = settings::Settings::load(None)?.with_alias(&alias);
    Ok(BuildRequest::Single(Box::new(build::EntryBuild {
        config: BuildConfig {
            jsx: settings.source.jsx.clone(),
            tsconfig: settings.source.tsconfig.clone(),
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
            alias: settings.alias(lib),
            sourcemap,
            lib,
            formats,
            types: !no_types,
            dts_bundle,
            // Started with the build: see `build::run`.
            plugins: Vec::new(),
        },
        settings: std::sync::Arc::new(settings),
    })))
}

/// Parses `esdev preview [options]`.
fn parse_preview(args: impl Iterator<Item = String>) -> Result<PreviewConfig, String> {
    let mut dir = None;
    let mut port = None;
    let mut config = None;
    for arg in args {
        let (flag, value) = split_flag_value(&arg);
        match flag {
            "-h" | "--help" => {
                reject_value(flag, value)?;
                println!("{PREVIEW_USAGE}");
                std::process::exit(0);
            }
            "--dir" => dir = Some(require_value(flag, value)?.to_string()),
            "--config" => config = Some(require_value(flag, value)?.to_string()),
            "--port" => {
                let text = require_value(flag, value)?;
                port = Some(text.parse::<u16>().map_err(|_| {
                    format!("{flag}={text} is not a port — a number from 1 to 65535.")
                })?);
            }
            other => {
                return Err(format!("unknown option: {other}\n\n{PREVIEW_USAGE}"));
            }
        }
    }
    Ok(PreviewConfig { dir, port, config })
}

/// Parses `esdev create <dir> [options]`.
fn parse_create(args: impl Iterator<Item = String>) -> Result<CreateConfig, String> {
    let mut dirs: Vec<String> = Vec::new();
    // `None` means "not said", which on a terminal becomes a question and
    // away from one becomes the default. A flag is always an answer.
    let mut template: Option<String> = None;
    let mut mode: Option<String> = None;
    let mut language: Option<String> = None;
    let mut styling: Option<String> = None;
    let mut blog: Option<bool> = None;
    let mut install: Option<Option<String>> = None;
    let mut force = false;
    let mut yes = false;
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
            "--language" => language = Some(require_value(flag, value)?.to_string()),
            "--styling" => styling = Some(require_value(flag, value)?.to_string()),
            "--blog" => {
                reject_value(flag, value)?;
                blog = Some(true);
            }
            "--no-blog" => {
                reject_value(flag, value)?;
                blog = Some(false);
            }
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
                yes = true;
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
        language,
        styling,
        blog,
        force,
        install,
        yes,
    })
}

/// Parses `esdev init [dir] [options]`.
///
/// The directory is optional where `create` requires one: no argument means
/// this directory, which is what adopting means. Everything else mirrors
/// `create` — one directory at most, every question answered by a flag, `-y`
/// answering all of them.
fn parse_init(args: impl Iterator<Item = String>) -> Result<InitConfig, String> {
    let mut dirs: Vec<String> = Vec::new();
    let mut name: Option<String> = None;
    let mut language: Option<String> = None;
    let mut entry: Option<String> = None;
    let mut install: Option<Option<String>> = None;
    let mut force = false;
    let mut yes = false;
    for arg in args {
        let (flag, value) = split_flag_value(&arg);
        match flag {
            "-h" | "--help" => {
                reject_value(flag, value)?;
                println!("{INIT_USAGE}");
                std::process::exit(0);
            }
            "--name" => name = Some(require_value(flag, value)?.to_string()),
            "--language" => language = Some(require_value(flag, value)?.to_string()),
            "--entry" => entry = Some(require_value(flag, value)?.to_string()),
            "--install" => {
                install = Some(Some(
                    value.unwrap_or(crate::create::DEFAULT_MANAGER).to_string(),
                ));
            }
            "--no-install" => {
                reject_value(flag, value)?;
                install = Some(None);
            }
            "-y" | "--yes" => {
                reject_value(flag, value)?;
                yes = true;
            }
            "--force" => {
                reject_value(flag, value)?;
                force = true;
            }
            flag if flag.starts_with('-') && flag.len() > 1 => {
                return Err(format!("unknown option: {flag}\n\n{INIT_USAGE}"));
            }
            dir => dirs.push(dir.to_string()),
        }
    }
    if dirs.len() > 1 {
        return Err(format!(
            "esdev init writes one project; got {} directories.\n\n{INIT_USAGE}",
            dirs.len()
        ));
    }
    Ok(InitConfig {
        dir: dirs.into_iter().next().unwrap_or_else(|| ".".to_string()),
        name,
        language,
        entry,
        install,
        force,
        yes,
    })
}

/// Parses `esdev start [options]`.
fn parse_start(args: impl Iterator<Item = String>) -> Result<StartConfig, String> {
    let mut config_path: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut hot = true;
    let mut options = RunOptions::default();
    let mut permission_args = Vec::new();
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
            "--allow-read" => {
                let permission = match value {
                    Some(value) => format!("--allow-read={value}"),
                    None => "--allow-read".to_string(),
                };
                permission_args.push(permission);
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
    let settings = settings::Settings::load(config_path.as_deref())?;
    let project = match settings.has_project {
        true => settings.with_start(port, permission_args),
        // No esdev.json — but an OTF Web project was never going to have one.
        // Refusing with the missing file is a dead end there; name the
        // toolchain its scripts call instead.
        false => match otfw_project_reason() {
            Some(reason) => {
                return Err(format!(
                    "this project runs with otfw, not `esdev start`.\n\n\
                     {reason}, and there is no esdev.json describing what to run. \
                     Start it the project's way:\n\n  npm run dev"
                ));
            }
            None => {
                return Err(format!(
                    "esdev start needs a {0}, and there is none here.\n\n\
                     It describes what this project builds and what to run:\n\n  \
                     {{ \"targets\": {{ \"server\": {{ \"entry\": \"src/server.ts\", \"out\": \"dist/server.js\" }} }},\n    \
                     \"start\": {{ \"run\": \"server\" }} }}\n\n\
                     See `esdev build --help` for the rest of {0}.",
                    config::FILE_NAME
                ));
            }
        },
    };
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
    let mut config_path = None;
    let mut jobs = None;
    let mut isolation = None;
    let mut watch = false;
    let mut watch_keys = false;
    let mut setup: Vec<String> = Vec::new();
    let mut global_setup: Vec<String> = Vec::new();
    let mut provided = None;
    let mut global_setup_out = None;
    let mut coverage = None;
    let mut coverage_out = None;
    let mut inspect = None;
    let mut detect_leaks = false;
    let mut tags_filter = Vec::new();
    let mut typecheck = false;
    let mut max_concurrency = None;
    let mut bench = false;
    let mut list_tags = None;
    let mut timeout = None;
    let mut reporter = None;
    let mut update_snapshots = false;
    let mut ci = std::env::var_os("CI").is_some_and(|value| !value.is_empty());
    let mut full_diff = false;
    let mut snapshot_prune = None;
    let mut dom = false;
    let mut browser = None;
    let mut headed = false;
    let mut name_pattern = None;
    let mut skip_pattern = None;
    let mut bail = None;
    let mut summary = None;
    let mut settings_file = None;
    let mut shard = None;
    let mut changed = None;
    let mut related = false;
    let mut randomize = false;
    let mut seed = None;
    let mut repeats = None;
    let mut list = false;
    let mut reporter_outfile = None;
    let mut quiet = false;
    let mut permissions = Permissions::new(Baseline::Everything);
    let mut permission_args = Vec::new();
    for arg in args {
        let (flag, value) = split_flag_value(&arg);
        // A rehearsal shapes each test file's run, not the parent's
        // discovery: the raw flags travel to every child, which re-parses
        // them as its own run, and `--isolation=none` resolves them below.
        if try_permission_flag(&mut permissions, flag, value)? {
            permission_args.push(arg);
            continue;
        }
        match flag {
            "-h" | "--help" => {
                reject_value(flag, value)?;
                println!("{TEST_USAGE}");
                std::process::exit(0);
            }
            "--file" => file = Some(require_value(flag, value)?.to_string()),
            "--config" => config_path = Some(require_value(flag, value)?.to_string()),
            "--jobs" => {
                let text = require_value(flag, value)?;
                let count = text
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| {
                        format!(
                            "{flag}={text} is not a number of files to run at once.\n\n\
                         One or more; --jobs=1 runs them one at a time and lets each \
                         one write straight to the terminal."
                        )
                    })?;
                jobs = Some(count);
            }
            "--isolation" => {
                let mode = require_value(flag, value)?;
                isolation = Some(match mode {
                    "process" => TestIsolation::Process,
                    "none" => TestIsolation::None,
                    _ => {
                        return Err(format!(
                            "{flag}={mode} is not a test isolation mode.\n\n  \
                         process  — one process per file, the default\n  \
                         none     — all files share one process and module cache"
                        ));
                    }
                });
            }
            "--watch" => {
                reject_value(flag, value)?;
                watch = true;
            }
            "--_watch-keys" => {
                reject_value(flag, value)?;
                watch_keys = true;
            }
            "--dom" => {
                reject_value(flag, value)?;
                dom = true;
            }
            "--browser" => {
                browser = Some(match value {
                    None => browser::Choice::Auto,
                    Some(name) => {
                        browser::Choice::parse(name).map_err(|err| format!("{flag}: {err}"))?
                    }
                });
            }
            "--headed" => {
                reject_value(flag, value)?;
                headed = true;
            }
            "--setup" => setup.push(require_value(flag, value)?.to_string()),
            "--global-setup" => global_setup.push(require_value(flag, value)?.to_string()),
            "--_provided" => provided = Some(std::path::PathBuf::from(require_value(flag, value)?)),
            "--coverage" => {
                reject_value(flag, value)?;
                coverage = Some(crate::coverage::Settings::default());
            }
            "--inspect" | "--inspect-brk" => {
                inspect = Some(InspectConfig {
                    address: inspect::parse_address(value)?,
                    wait: flag == "--inspect-brk",
                });
            }
            "--_bench" => {
                reject_value(flag, value)?;
                bench = true;
            }
            "--max-concurrency" => {
                let text = require_value(flag, value)?;
                max_concurrency =
                    Some(text.parse::<u64>().ok().filter(|n| *n > 0).ok_or_else(|| {
                        format!(
                            "{flag}={text} is not a number of tests: a whole number above zero."
                        )
                    })?);
            }
            "--typecheck" => {
                reject_value(flag, value)?;
                typecheck = true;
            }
            "--tags-filter" => {
                let text = require_value(flag, value)?;
                // Parsed now, so a mistake is said once rather than per file.
                tags::parse(text)?;
                tags_filter.push(text.to_string());
            }
            "--list-tags" => {
                list_tags = Some(match value {
                    None => false,
                    Some("json") => true,
                    Some(other) => {
                        return Err(format!(
                            "--list-tags={other} is not a format: --list-tags prints them for \
                             a person, --list-tags=json for a program."
                        ));
                    }
                });
            }
            "--detect-async-leaks" => {
                reject_value(flag, value)?;
                detect_leaks = true;
            }
            "--_coverage" => {
                coverage_out = Some(std::path::PathBuf::from(require_value(flag, value)?));
            }
            "--_global-setup-out" => {
                global_setup_out = Some(std::path::PathBuf::from(require_value(flag, value)?));
            }
            "-t" | "--test-name-pattern" => {
                name_pattern = Some(require_value(flag, value)?.to_string());
            }
            "--test-skip-pattern" => {
                skip_pattern = Some(require_value(flag, value)?.to_string());
            }
            "--bail" => {
                bail = Some(match value {
                    None => 1,
                    Some(text) => text.parse::<usize>().ok().filter(|n| *n > 0).ok_or_else(|| {
                        format!(
                            "{flag}={text} is not a number of failed tests.\n\n\
                             One or more: --bail stops after the first failure, --bail=5 after five."
                        )
                    })?,
                });
            }
            "--shard" => {
                shard = Some(test::Shard::parse(require_value(flag, value)?)?);
            }
            "--changed" => changed = Some(value.map(str::to_string)),
            "--related" => {
                reject_value(flag, value)?;
                related = true;
            }
            "--randomize" => {
                reject_value(flag, value)?;
                randomize = true;
            }
            "--seed" => {
                let text = require_value(flag, value)?;
                seed = Some(text.parse::<u32>().map_err(|_| {
                    format!("{flag}={text} is not a seed: a whole number from 0 to 4294967295.")
                })?);
            }
            "--repeats" => {
                let text = require_value(flag, value)?;
                repeats = Some(text.parse::<u32>().map_err(|_| {
                    format!(
                        "{flag}={text} is not a number of repeats.\n\n\
                         How many more times every test runs after the first: --repeats=20 runs each 21 times."
                    )
                })?);
            }
            "--list" => {
                reject_value(flag, value)?;
                list = true;
            }
            "--_summary" => summary = Some(std::path::PathBuf::from(require_value(flag, value)?)),
            "--_settings" => {
                settings_file = Some(std::path::PathBuf::from(require_value(flag, value)?));
            }
            "--timeout" => {
                let text = require_value(flag, value)?;
                timeout = Some(
                    text.parse::<u64>()
                        .ok()
                        .filter(|ms| *ms > 0)
                        .ok_or_else(|| {
                            format!(
                                "{flag}={text} is not a number of milliseconds.\n\n\
                             How long one file may take before it is stopped and \
                             failed: --timeout=5000."
                            )
                        })?,
                );
            }
            "--reporter" => {
                let name = require_value(flag, value)?;
                if !report::REPORTERS.iter().any(|(known, _)| *known == name) {
                    let known: String = report::REPORTERS
                        .iter()
                        .map(|(known, what)| format!("\n  {known:<6} — {what}"))
                        .collect();
                    return Err(format!("{flag}={name} is not a reporter.\n{known}"));
                }
                reporter = Some(name.to_string());
            }
            "--reporter-outfile" => {
                reporter_outfile = Some(std::path::PathBuf::from(require_value(flag, value)?));
            }
            "--_quiet" => {
                reject_value(flag, value)?;
                quiet = true;
            }
            "-u" | "--update-snapshots" => {
                reject_value(flag, value)?;
                update_snapshots = true;
            }
            "--ci" => {
                reject_value(flag, value)?;
                ci = true;
            }
            "--full-diff" => {
                reject_value(flag, value)?;
                full_diff = true;
            }
            "--_snapshot-prune" => {
                snapshot_prune = Some(match require_value(flag, value)? {
                    "0" => false,
                    "1" => true,
                    _ => return Err("internal snapshot prune flag must be 0 or 1".to_string()),
                });
            }
            flag if flag.starts_with('-') && flag.len() > 1 => {
                return Err(format!("unknown option: {flag}\n\n{TEST_USAGE}"));
            }
            filter => filters.push(filter.to_string()),
        }
    }
    // `--file` is the child's own flag: it is one run of one file, with no
    // discovery to repeat and no second file to run beside it.
    if list && watch {
        return Err("--list names the tests once; there is nothing to watch.".to_string());
    }
    if list && matches!(reporter.as_deref(), Some("junit" | "tap" | "dots")) {
        return Err(format!(
            "--list names tests, and a {} report is of tests that ran.\n\n\
             Use the default report, or --reporter=json for one listed test per line.",
            reporter.as_deref().unwrap_or_default()
        ));
    }
    if file.is_some() && (watch || jobs.is_some()) {
        return Err("--file runs one file, so there is nothing to schedule or \
             re-run.\n\n\
             Drop --file to watch or to run several at once; a filter narrows \
             which ones: `esdev test --watch db`."
            .to_string());
    }
    // `--related`'s files are the arguments a filter would be, so that a
    // pre-commit tool can append the staged files: `esdev test --related a.ts`.
    let affected_by = match (changed, related) {
        (Some(_), true) => {
            return Err(
                "--changed and --related both choose the files a change reaches; \
                 give one.\n\n\
                 --changed asks git what changed; --related names the files."
                    .to_string(),
            );
        }
        (Some(since), false) => Some(test::AffectedBy::Changed(since)),
        (None, true) if filters.is_empty() => {
            return Err(
                "--related runs the tests that import the files named after it, \
                 and none were.\n\n\
                 Name the source files: esdev test --related src/cart.ts src/price.ts"
                    .to_string(),
            );
        }
        (None, true) => Some(test::AffectedBy::Related(std::mem::take(&mut filters))),
        (None, false) => None,
    };
    if affected_by.is_some() && (file.is_some() || watch) {
        return Err(format!(
            "{} selects test files for one run, and {} has no such selection.\n\n\
             Drop one of them.",
            if related { "--related" } else { "--changed" },
            if watch { "--watch" } else { "--file" },
        ));
    }
    if shard.is_some() && file.is_some() {
        return Err("--file runs one file, so there is nothing to shard.\n\n\
             Drop --file; --shard splits the files a run discovers."
            .to_string());
    }
    if shard.is_some() && watch {
        return Err(
            "--shard splits a suite across machines, and --watch re-runs \
             it on this one.\n\n\
             Drop --shard to watch, or --watch to run a shard."
                .to_string(),
        );
    }
    let snapshot_prune = snapshot_prune.unwrap_or(file.is_some());
    // A rehearsal of a grant that cannot exist fails before running anything,
    // not once per file in every child.
    test_capabilities(&permission_args)?;
    Ok(TestConfig {
        // Filled in from the project by `resolve_test`, or read whole from the
        // parent's `--_settings`.
        run: test::FileRun {
            source: settings::Source::default(),
            dom,
            setup,
            provided,
            quiet,
            update_snapshots,
            ci,
            full_diff,
            snapshot_prune,
            name_pattern,
            skip_pattern,
            seed,
            repeats,
            list,
            bench,
            max_concurrency,
            tags_filter,
            detect_leaks,
            tag_definitions: Vec::new(),
            strict_tags: true,
            inspect,
            permission_args,
        },
        browser,
        headed,
        bail,
        summary,
        global_setup,
        global_setup_out,
        coverage_flag: coverage.is_some(),
        coverage,
        coverage_out,
        coverage_dir: None,
        typecheck,
        list_tags,
        affected_by,
        shard,
        randomize,
        reporter_outfile,
        file,
        filters,
        jobs,
        isolation,
        watch,
        watch_keys,
        timeout,
        reporter,
        settings_file,
        config_path,
    })
}

/// Resolves `esdev test`'s permission flags the way a run resolves them.
///
/// Shared by the fail-fast validation in [`parse_test`] and the
/// `--isolation=none` run, which executes files in this process rather than
/// in children that could re-parse their own command line.
fn test_capabilities(
    args: &[String],
) -> Result<
    (
        es_runtime_common::CapabilitySet,
        es_runtime_cli_common::permissions::Scopes,
    ),
    String,
> {
    let mut permissions = Permissions::new(Baseline::Everything);
    for arg in args {
        let (flag, value) = split_flag_value(arg);
        // Validated when the command line was parsed; an arg that is not a
        // permission flag here is an internal caller passing nonsense.
        if !try_permission_flag(&mut permissions, flag, value)? {
            return Err(format!("{arg} is not a permission flag."));
        }
    }
    Ok((permissions.resolve()?, permissions.scopes()?))
}

/// The grant a test file runs under: its own `@permissions`, resolved from
/// `esrun`'s baseline so the flags read as its deployment's; or, with none,
/// the command line's rehearsal (D121).
fn file_capabilities(
    file: &std::path::Path,
    run_flags: &[String],
) -> Result<
    (
        es_runtime_common::CapabilitySet,
        es_runtime_cli_common::permissions::Scopes,
    ),
    String,
> {
    let Some(declared) = std::fs::read_to_string(file)
        .ok()
        .and_then(|source| tags::declared_permissions(&source))
    else {
        return test_capabilities(run_flags);
    };
    let mut permissions = Permissions::new(Baseline::Nothing);
    for arg in &declared {
        let (flag, value) = split_flag_value(arg);
        let known = try_permission_flag(&mut permissions, flag, value)
            .map_err(|err| format!("{}: @permissions: {err}", file.display()))?;
        if !known {
            return Err(format!(
                "{}: @permissions: `{arg}` is not a permission flag. \
                 It takes the flags esrun is run with, such as --allow-read=./data.",
                file.display()
            ));
        }
    }
    let resolved = permissions
        .resolve()
        .and_then(|capabilities| Ok((capabilities, permissions.scopes()?)))
        .map_err(|err| format!("{}: @permissions: {err}", file.display()))?;
    Ok(resolved)
}

/// `esdev <file>` and `esdev -e`: the module, read the way the project
/// around it says source is read.
async fn run_module(
    run: settings::Run,
    inspect: Option<&InspectConfig>,
    config: Option<&str>,
) -> Result<(), String> {
    let settings = settings::Settings::load(config)?;
    let mut config = settings.source.run_config(run).await?;
    attach_debugger(&mut config, inspect)?;
    es_runtime_cli_common::run("esdev", config).await
}

/// A module named relative to `dir`, as the file URL a child imports it by. A
/// name that is not a file there is left as written: a bare specifier names a
/// package, and where that lives is the resolver's question.
fn module_url(dir: &std::path::Path, module: &str) -> Result<String, String> {
    let path = dir.join(module);
    if path.exists() {
        url::Url::from_file_path(&path)
            .map(|url| url.to_string())
            .map_err(|()| format!("cannot name module {module} as a file URL"))
    } else {
        Ok(module.to_string())
    }
}

/// The test flags, resolved against the project: what `esdev.json`'s `test`
/// section says, where a flag did not already answer.
///
/// **A flag beats the file**, the same rule the build uses. Setup modules are
/// resolved against the project directory and made file URLs, because a child
/// process is started from wherever the parent was and a relative path would
/// otherwise mean two different files. A raw absolute Windows path would be
/// parsed as a `d:` module specifier rather than a local file.
fn resolve_test(config: &mut TestConfig, settings: &settings::Settings) -> Result<(), String> {
    // A flag's global setup is named from where the run was started: the
    // process that runs it is started from here too, but imports by URL.
    let here = std::env::current_dir().unwrap_or_default();
    config.global_setup = config
        .global_setup
        .iter()
        .map(|module| module_url(&here, module))
        .collect::<Result<_, _>>()?;
    config.run.source = settings.source.clone();
    let root = &settings.source.root;
    // Every key of the file's `test` section, named: one added to the file
    // without a rule here does not compile, rather than being read and dropped
    // (DECISIONS D125).
    let config::TestSettings {
        setup,
        global_setup,
        timeout,
        jobs,
        isolation,
        reporter,
        browser,
        coverage,
        max_concurrency,
        tags,
        strict_tags,
    } = &settings.test;
    if config.run.setup.is_empty() {
        config.run.setup = setup
            .iter()
            .map(|module| -> Result<_, String> {
                let path = root.join(module);
                // A bare specifier stays one: `"setup": "my-preset/register"`
                // names a package, and where that lives is the resolver's
                // question rather than this file's.
                if path.exists() {
                    url::Url::from_file_path(&path)
                        .map(|url| url.to_string())
                        .map_err(|()| format!("cannot name setup module {module} as a file URL"))
                } else {
                    Ok(module.clone())
                }
            })
            .collect::<Result<_, _>>()?;
    }
    config.run.tag_definitions.clone_from(tags);
    if config.run.max_concurrency.is_none() {
        config.run.max_concurrency = *max_concurrency;
    }
    config.run.strict_tags = strict_tags.unwrap_or(true);
    if config.global_setup.is_empty() {
        config.global_setup = global_setup
            .iter()
            .map(|module| module_url(root, module))
            .collect::<Result<_, _>>()?;
    }
    // The project's coverage settings, when the flag or the project turns it on.
    if let Some(section) = coverage
        && (config.coverage.is_some() || section.enabled)
    {
        config.coverage = Some(section.settings.clone());
    }
    if config.timeout.is_none() {
        config.timeout = *timeout;
    }
    if config.jobs.is_none() {
        config.jobs = *jobs;
    }
    if config.isolation.is_none() {
        config.isolation = *isolation;
    }
    if config.reporter.is_none() {
        config.reporter.clone_from(reporter);
    }
    if config.browser.is_none() {
        config.browser.clone_from(browser);
    }
    Ok(())
}

/// Refuses what cannot mean anything once the files run in a browser.
///
/// Checked after the project is read, since `test.browser` in `esdev.json`
/// puts a run in a browser as surely as the flag does.
fn validate_browser_test_config(config: &TestConfig) -> Result<(), String> {
    if config.browser.is_none() {
        if config.headed {
            return Err(
                "--headed shows the browser a run uses, and this run uses none.\n\n\
                 Add --browser, or set \"browser\" under \"test\" in esdev.json."
                    .to_string(),
            );
        }
        return Ok(());
    }
    if config.run.dom {
        return Err("--dom and --browser are two answers to one question.\n\n\
             --dom installs esdev's own DOM in this runtime; --browser runs the file \
             in a browser, which has the real one. Pick one."
            .to_string());
    }
    if !config.run.permission_args.is_empty() {
        return Err(
            "permission flags rehearse this runtime's grant, and a browser \
             run is not in this runtime.\n\n\
             A page has the browser's own sandbox; drop the --deny/--allow flags, \
             or drop --browser to rehearse the production grant."
                .to_string(),
        );
    }
    if config.isolation == Some(TestIsolation::None) {
        return Err("--isolation=none shares one process between files, and a \
             browser run has no process of esdev's to share.\n\n\
             In a browser each file gets a browsing context of its own."
            .to_string());
    }
    Ok(())
}

/// Runs one test file, or discovers and runs them all.
///
/// The parent spawns a child per file rather than looping in-process, so a file
/// that hangs or exits takes only itself down. `--file` is what a child is
/// invoked with, and is equally a supported way to run one file by hand.
/// Runs the test files in a browser over WebDriver BiDi.
///
/// What is chosen, and what was passed over, goes to stderr: it describes the
/// run rather than being a result of it, and stdout belongs to the reporter.
async fn run_browser_tests(config: &TestConfig) -> ExitCode {
    if let Err(err) = validate_browser_test_config(config) {
        eprintln!("error: {err}");
        return ExitCode::FAILURE;
    }
    let Some(choice) = config.browser.clone() else {
        return ExitCode::FAILURE;
    };
    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let discover = || match &config.file {
        Some(file) => Ok((vec![root.join(file)], 1)),
        None => select_test_files(&root, config, true),
    };
    let (files, discovered) = match discover() {
        Ok(selected) => selected,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };
    // A shard left with none passed; see the process run.
    if files.is_empty() && discovered > 0 {
        return ExitCode::SUCCESS;
    }
    if files.is_empty() && !config.watch {
        eprintln!(
            "no test files found (looked for {})",
            test::sought_description()
        );
        return ExitCode::FAILURE;
    }
    let choices = choice.each();
    let several = choices.len() > 1;
    // The run's closing line, for the terminal a person reads; a machine
    // reporter wrote its own ending.
    let report = |total: usize, failed: usize| {
        if !config.terminal_human() {
        } else if config.run.list {
            test::report_listed(total, failed);
        } else {
            test::report(total, failed);
        }
    };

    if !config.watch {
        // Each browser in turn, every file in each, into one report. A browser
        // that cannot be driven fails the run, and the others still run.
        let mut reporter = test::Reporter::new(config);
        let (mut total, mut failed, mut broken) = (0, 0, 0usize);
        for choice in &choices {
            let open = match OpenBrowser::start(choice, config, several).await {
                Ok(open) => open,
                Err(err) => {
                    eprintln!("error: {err}");
                    broken += 1;
                    continue;
                }
            };
            // Stopped from outside — ^C, or a CI job cancelled — the browser
            // is still closed: it is a separate process, and would otherwise
            // outlive the run.
            let ran = tokio::select! {
                ran = open.runner.run(&root, &files, config, open.label.as_deref(), &mut reporter) => ran,
                () = watch::stopped() => {
                    open.close().await;
                    return ExitCode::from(130);
                }
            };
            open.close().await;
            match ran {
                Ok(failed_here) => {
                    total += files.len();
                    failed += failed_here;
                }
                Err(err) => {
                    eprintln!("error: {err}");
                    broken += 1;
                }
            }
        }
        if total > 0 {
            reporter.finish(total, failed);
            report(total, failed);
        }
        // Said last, where the tally is read: a run that passed in one browser
        // and never ran in another has not passed.
        if broken > 0 && several {
            eprintln!(
                "{broken} of {} browsers could not run the tests",
                choices.len()
            );
        }
        return if failed == 0 && broken == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    // `--watch`: the same browsers for every pass, and the files discovered
    // afresh each time, so a test written after the watch began is found.
    let mut open = Vec::new();
    for choice in &choices {
        match OpenBrowser::start(choice, config, several).await {
            Ok(browser) => open.push(browser),
            Err(err) => {
                for browser in open {
                    browser.close().await;
                }
                eprintln!("error: {err}");
                return ExitCode::FAILURE;
            }
        }
    }
    let close = |open: Vec<OpenBrowser>| async move {
        for browser in open {
            browser.close().await;
        }
    };
    let (_watcher, mut changes) = match test::change_watcher(&root, &config.project_file(&root)) {
        Ok(watching) => watching,
        Err(err) => {
            close(open).await;
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };
    let paint = style::Palette::stderr();
    'watching: loop {
        let (files, _) = match discover() {
            Ok(selected) => selected,
            Err(err) => {
                eprintln!("error: {err}");
                (Vec::new(), 0)
            }
        };
        if files.is_empty() {
            eprintln!(
                "no test files found (looked for {})",
                test::sought_description()
            );
        } else {
            let mut reporter = test::Reporter::new(config);
            let (mut total, mut failed) = (0, 0);
            for browser in &open {
                tokio::select! {
                    ran = browser.runner.run(&root, &files, config, browser.label.as_deref(), &mut reporter) => match ran {
                        Ok(failed_here) => {
                            total += files.len();
                            failed += failed_here;
                        }
                        Err(err) => {
                            // The browser went away; nothing more can run in it.
                            eprintln!("error: {err}");
                            break 'watching;
                        }
                    },
                    () = watch::stopped() => break 'watching,
                }
            }
            reporter.finish(total, failed);
            report(total, failed);
        }
        eprintln!("{}", paint.dim("watching for changes — ^C to stop"));
        tokio::select! {
            change = watch::coalesce(&mut changes) => match change {
                None => break,
                Some(burst) if burst.contains(&test::Change::Project) => {
                    close(open).await;
                    let Ok(exe) = std::env::current_exe() else {
                        eprintln!("error: cannot find the esdev binary");
                        return ExitCode::FAILURE;
                    };
                    return restart_watch(&exe, config).await;
                }
                Some(_) => {}
            },
            () = watch::stopped() => break,
        }
        println!();
    }
    close(open).await;
    ExitCode::SUCCESS
}

/// Starts a watching run again, from scratch, after its project file changed:
/// every setting was resolved from that file, so the session that read the old
/// one ends and a new one reads the new one — as Vitest restarts on a change to
/// its config.
async fn restart_watch(exe: &std::path::Path, config: &TestConfig) -> ExitCode {
    let paint = style::Palette::stderr();
    eprintln!(
        "\n{}",
        paint.dim(format!(
            "{} changed — restarting",
            config.config_path.as_deref().unwrap_or(config::FILE_NAME)
        ))
    );
    relaunch(exe).await
}

/// Runs `esdev` again with the same arguments, replacing this process: a
/// session restarted ten times is one process, not ten waiting on each other.
#[cfg(unix)]
async fn relaunch(exe: &std::path::Path) -> ExitCode {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!("error: cannot restart esdev: {err}");
    ExitCode::FAILURE
}

/// Where a process cannot replace itself, the new session runs as a child and
/// this one waits for it.
#[cfg(not(unix))]
async fn relaunch(exe: &std::path::Path) -> ExitCode {
    let status = tokio::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .status()
        .await;
    match status {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(_) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("error: cannot restart esdev: {err}");
            ExitCode::FAILURE
        }
    }
}

/// A browser started for a run, and the runner driving it.
struct OpenBrowser {
    session: bidi::Session,
    runner: browser_run::Runner,
    /// Its name in each file's report, when the run uses several.
    label: Option<String>,
}

impl OpenBrowser {
    /// Finds and starts the browser `choice` names, saying on stderr what it
    /// chose and what it passed over.
    async fn start(
        choice: &browser::Choice,
        config: &TestConfig,
        several: bool,
    ) -> Result<OpenBrowser, String> {
        let selected = browser::select(choice, &browser::System)?;
        eprintln!("{}", selected.describe());
        let session = bidi::Session::start(&selected.launch, !config.headed).await?;
        let runner = match browser_run::Runner::new(&session, selected.launch.browser.name()).await
        {
            Ok(runner) => runner,
            Err(err) => {
                session.end().await;
                return Err(err);
            }
        };
        Ok(OpenBrowser {
            session,
            runner,
            label: several.then(|| selected.launch.browser.to_string()),
        })
    }

    async fn close(self) {
        drop(self.runner);
        self.session.end().await;
    }
}

/// How one file's run ends: the report a person reads, nothing (the parent
/// reports it), or — run directly with a machine reporter — that report.
fn finish_test_file(config: &TestConfig, file: &str) -> ExitCode {
    if config.run.quiet {
        return guest::test::finish_quiet();
    }
    match config.reporter.as_deref() {
        None | Some("human") => guest::test::finish(),
        Some(format) => {
            let code = guest::test::finish_quiet();
            let result = guest::test::file_result(file, file);
            let text = match format {
                "json" => report::json_file(&result),
                "junit" => report::junit(std::slice::from_ref(&result)),
                "tap" => report::tap(std::slice::from_ref(&result)),
                _ => format!(
                    "{}{}",
                    report::dots(&result),
                    report::dots_end(std::slice::from_ref(&result))
                ),
            };
            if let Err(err) = test::write_report(config, &text) {
                eprintln!("error: {err}");
                return ExitCode::FAILURE;
            }
            code
        }
    }
}

/// Runs the tests — and, under `--typecheck`, the project's `tsc --noEmit`
/// first, whose failure fails the run however the tests do.
async fn run_tests(config: TestConfig) -> ExitCode {
    let typechecked = if config.typecheck && !config.run.list {
        let root = std::env::current_dir().unwrap_or_default();
        eprintln!("typecheck: tsc --noEmit");
        match check::check_to(&root, &[], !config.terminal_human()).await {
            Ok(()) => true,
            Err(err) => {
                eprintln!("typecheck: {err}");
                false
            }
        }
    } else {
        true
    };
    let parent = config.settings_file.is_none();
    let code = run_tests_inner(config).await;
    // The settings this run wrote for its children, which have all ended.
    if parent {
        test::remove_settings();
    }
    if typechecked { code } else { ExitCode::FAILURE }
}

async fn run_tests_inner(mut config: TestConfig) -> ExitCode {
    if config.run.bench {
        test::discover_benchmarks();
    }
    // A child runs with what its parent resolved, read whole; anything else
    // resolves the project itself, once. The two cannot disagree, because a
    // child never reads the project.
    let resolved = match config.settings_file.clone() {
        Some(path) => test::FileRun::read(&path).map(|run| config.run = run),
        None => {
            settings::Settings::load(None).and_then(|project| resolve_test(&mut config, &project))
        }
    };
    if let Err(err) = resolved {
        eprintln!("error: {err}");
        return ExitCode::FAILURE;
    }

    if let Some(json) = config.list_tags {
        print!("{}", list_tags(&config.run.tag_definitions, json));
        return ExitCode::SUCCESS;
    }
    // The process a run starts to hold its global setup.
    if let Some(out) = config.global_setup_out.clone() {
        return global_setup::run(&config, &out).await;
    }
    if let Some(refusal) = coverage::refused(&config) {
        if config.coverage_flag {
            eprintln!("error: {refusal}");
            return ExitCode::FAILURE;
        }
        // Turned on by the project, and this run cannot: said, and skipped.
        if config.file.is_none() || config.summary.is_none() {
            eprintln!(
                "coverage: not collected — {}",
                refusal.lines().next().unwrap_or_default()
            );
        }
        config.coverage = None;
    }
    // Listing runs no code worth measuring.
    if config.run.list {
        config.coverage = None;
    }
    if config.run.detect_leaks {
        let refusal = if config.browser.is_some() {
            Some("a browser run's pending work is the browser's, which this runtime cannot see")
        } else if config.isolation == Some(TestIsolation::None) {
            Some(
                "with --isolation=none the files share one event loop, so nothing pending is any one file's",
            )
        } else {
            None
        };
        if let Some(refusal) = refusal {
            eprintln!("error: --detect-async-leaks: {refusal}.");
            return ExitCode::FAILURE;
        }
    }
    if let Err(err) = prepare_inspect(&mut config) {
        eprintln!("error: {err}");
        return ExitCode::FAILURE;
    }

    // A shuffled run says its seed, so the order that failed can be run again.
    // A child is handed the seed alone and says nothing.
    if config.randomize || config.run.seed.is_some() {
        let seed = config.run.seed.unwrap_or_else(test::fresh_seed);
        config.run.seed = Some(seed);
        if config.randomize || config.file.is_none() {
            eprintln!("randomize: seed={seed} (run this order again with --seed={seed})");
        }
    }

    if config.browser.is_some() || config.headed {
        if let Err(err) = validate_browser_test_config(&config) {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
        let Ok(exe) = std::env::current_exe() else {
            eprintln!("error: cannot find the esdev binary");
            return ExitCode::FAILURE;
        };
        // Global setup runs here, not in the page, as it would for any run —
        // and, as for any run, only when there is a file to run.
        let root = std::env::current_dir().unwrap_or_default();
        let has_files = config.file.is_some()
            || config.watch
            || select_test_files(&root, &config, false).is_ok_and(|(files, _)| !files.is_empty());
        let setup = if has_files {
            match global_setup::start_or_report(&exe, &config).await {
                Ok(setup) => setup,
                Err(code) => return code,
            }
        } else {
            None
        };
        config.run.provided = setup
            .as_ref()
            .map(|running| running.provided().to_path_buf());
        let code = run_browser_tests(&config).await;
        return global_setup::stop_into(setup, code).await;
    }

    if let Some(file) = config.file.clone() {
        // Run directly, rather than as a child of a run that did it already,
        // a file gets the global setup its suite would.
        let direct = config.summary.is_none() && config.run.provided.is_none();
        let setup = if direct {
            let Ok(exe) = std::env::current_exe() else {
                eprintln!("error: cannot find the esdev binary");
                return ExitCode::FAILURE;
            };
            match global_setup::start_or_report(&exe, &config).await {
                Ok(setup) => setup,
                Err(code) => return code,
            }
        } else {
            None
        };
        let provided = match (&setup, &config.run.provided) {
            (Some(running), _) => running.provided_json(),
            (None, Some(path)) => std::fs::read_to_string(path).ok(),
            (None, None) => None,
        };
        guest::test::configure_provided(provided);
        // Run on its own with `--coverage`, the file reports its own coverage.
        let scratch = if direct && config.coverage.is_some() {
            match coverage::scratch() {
                Ok(dir) => {
                    config.coverage_out = Some(dir.join("0.json"));
                    Some(dir)
                }
                Err(err) => {
                    eprintln!("error: {err}");
                    return global_setup::stop_into(setup, ExitCode::FAILURE).await;
                }
            }
        } else {
            None
        };
        let mut code = run_test_file(&config, file).await;
        if let Some(dir) = scratch {
            code = coverage_verdict(&dir, &config, code);
        }
        return global_setup::stop_into(setup, code).await;
    }

    let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let Ok(exe) = std::env::current_exe() else {
        eprintln!("error: cannot find the esdev binary");
        return ExitCode::FAILURE;
    };

    if config.watch {
        if let Err(message) = validate_unisolated_test_config(&config) {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
        // Once for the whole session, torn down when it ends, as Vitest does.
        let setup = match global_setup::start_or_report(&exe, &config).await {
            Ok(setup) => setup,
            Err(code) => return code,
        };
        config.run.provided = setup
            .as_ref()
            .map(|running| running.provided().to_path_buf());
        guest::test::configure_provided(
            setup
                .as_ref()
                .and_then(global_setup::Running::provided_json),
        );
        // A watched run ends when the developer ends it, so its status is the
        // watcher's rather than any pass's.
        // One directory for every pass's coverage, reported after each.
        if config.coverage.is_some() {
            match coverage::scratch() {
                Ok(dir) => config.coverage_dir = Some(dir),
                Err(err) => {
                    eprintln!("error: {err}");
                    return global_setup::stop_into(setup, ExitCode::FAILURE).await;
                }
            }
        }
        let ended = test::watch(&root, &config, &exe).await;
        let code = match ended {
            Ok(_) => ExitCode::SUCCESS,
            Err(ref err) => {
                eprintln!("error: {err}");
                ExitCode::FAILURE
            }
        };
        if let Some(dir) = &config.coverage_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
        let code = global_setup::stop_into(setup, code).await;
        // After the teardown, so the new session's global setup is the only
        // one running.
        if matches!(ended, Ok(test::WatchEnd::Restart)) {
            return restart_watch(&exe, &config).await;
        }
        return code;
    }

    let (files, discovered) = match select_test_files(&root, &config, true) {
        Ok(selected) => selected,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };
    config.run.snapshot_prune = config.filters.is_empty();
    // More shards than files leaves some with none: a shard that passed, with
    // its (empty) report written, rather than a suite that is missing.
    if files.is_empty() && discovered == 0 {
        eprintln!(
            "no test files found (looked for {})",
            test::sought_description()
        );
        return ExitCode::FAILURE;
    }

    if config.isolation == Some(TestIsolation::None)
        && let Err(message) = validate_unisolated_test_config(&config)
    {
        eprintln!("error: {message}");
        return ExitCode::FAILURE;
    }
    // Once, before any file, and only for a run that has files to run.
    let setup = if files.is_empty() {
        None
    } else {
        match global_setup::start_or_report(&exe, &config).await {
            Ok(setup) => setup,
            Err(code) => return code,
        }
    };
    config.run.provided = setup
        .as_ref()
        .map(|running| running.provided().to_path_buf());
    let scratch = match config.coverage.as_ref().map(|_| coverage::scratch()) {
        Some(Ok(dir)) => Some(dir),
        Some(Err(err)) => {
            eprintln!("error: {err}");
            return global_setup::stop_into(setup, ExitCode::FAILURE).await;
        }
        None => None,
    };
    config.coverage_dir.clone_from(&scratch);
    let mut code = run_selected(&exe, &root, &files, &config).await;
    if let Some(dir) = scratch {
        code = coverage_verdict(&dir, &config, code);
    }
    global_setup::stop_into(setup, code).await
}

/// `--list-tags`: each tag with what it says about itself, or all of it as
/// JSON.
fn list_tags(tags: &[crate::config::TagDefinition], json: bool) -> String {
    if json {
        return format!("{}\n", serde_json::json!({ "tags": tags }));
    }
    if tags.is_empty() {
        return "no tags are defined (test.tags in esdev.json)\n".to_string();
    }
    tags.iter()
        .map(|tag| match &tag.description {
            Some(description) => format!("{}: {description}\n", tag.name),
            None => format!("{}\n", tag.name),
        })
        .collect()
}

/// Checks `--inspect` against the rest of the run, and shapes the run for a
/// debugger: one file at a time, since one debugger follows one process, and
/// no per-file time limit, which a paused breakpoint would trip.
fn prepare_inspect(config: &mut TestConfig) -> Result<(), String> {
    let Some(inspect) = &config.run.inspect else {
        return Ok(());
    };
    let flag = if inspect.wait {
        "--inspect-brk"
    } else {
        "--inspect"
    };
    if !es_runtime_cli_common::HAS_INSPECTOR {
        return Err(es_runtime_cli_common::NO_INSPECTOR_MESSAGE.to_string());
    }
    if config.coverage_flag {
        return Err(format!(
            "{flag} and --coverage both use the file's inspector, and there is only one.\n\n\
             Debug with {flag}, then measure with --coverage."
        ));
    }
    // Coverage the project turns on is set aside while debugging.
    config.coverage = None;
    if config.browser.is_some() {
        return Err(format!(
            "{flag} debugs this runtime, and a browser run runs in the browser.\n\n\
             Use --headed and the browser's own developer tools."
        ));
    }
    if config.jobs.is_some_and(|jobs| jobs > 1) {
        return Err(format!(
            "{flag} debugs one file at a time, so --jobs cannot be more than 1."
        ));
    }
    let parent = config.summary.is_none() && config.file.is_none();
    if parent {
        // One process already, with `--isolation=none`.
        if config.isolation != Some(TestIsolation::None) {
            config.jobs = Some(1);
        }
        if config.timeout.take().is_some() {
            eprintln!(
                "inspect: --timeout is off, since a breakpoint can hold a file as long as it likes"
            );
        }
    }
    Ok(())
}

/// Reports a run's coverage and folds its thresholds into the exit code.
fn coverage_verdict(dir: &std::path::Path, config: &TestConfig, code: ExitCode) -> ExitCode {
    let root = std::env::current_dir().unwrap_or_default();
    let verdict = coverage::finish(dir, &root, config);
    let _ = std::fs::remove_dir_all(dir);
    match verdict {
        Ok(true) => code,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Runs the files a run selected, each in a process of its own unless the
/// run shares one.
async fn run_selected(
    exe: &std::path::Path,
    root: &std::path::Path,
    files: &[std::path::PathBuf],
    config: &TestConfig,
) -> ExitCode {
    if config.isolation == Some(TestIsolation::None) {
        guest::test::configure_provided(
            config
                .run
                .provided
                .as_ref()
                .and_then(|path| std::fs::read_to_string(path).ok()),
        );
        return run_tests_unisolated(files, config).await;
    }

    let jobs = config
        .jobs
        .unwrap_or_else(test::jobs)
        .min(files.len())
        .max(1);
    let failed = test::run_all(exe, root, files, jobs, config).await.failed;
    let total = files.len();
    if !config.terminal_human() {
        // The reporter wrote its own ending.
    } else if config.run.list {
        test::report_listed(total, failed);
    } else {
        test::report(total, failed);
    }
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Runs one test file in this process: what every child of a run does, and a
/// supported way to run one file directly.
async fn run_test_file(config: &TestConfig, file: String) -> ExitCode {
    let mut run_options = config.run_options();
    // `@module-tag`s tag every test in the file.
    run_options.module_tags = std::fs::read_to_string(&file)
        .map(|source| tags::module_tags(&source))
        .unwrap_or_default();
    guest::test::reset();
    guest::test::configure_run(run_options);
    guest::test::configure_snapshots(
        Some(std::path::PathBuf::from(&file)),
        config.run.update_snapshots,
        config.run.ci,
        config.run.full_diff,
        config.run.snapshot_prune,
    );
    // Nothing is added to the file, unless `--setup` named something to
    // import ahead of it. It is otherwise an ordinary run of an ordinary
    // module — the same transform any `.ts` gets — and the test API comes
    // from the `runtime:test` the file imported. What makes this a *test*
    // run is what `finish()` finds afterwards, not anything done to the
    // source.
    let stripper = if config.run.setup.is_empty() && !config.run.dom {
        TypeStripper::new()
    } else {
        let entry = std::fs::canonicalize(&file)
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(&file));
        TypeStripper::before_with(
            &entry,
            config.run.dom.then(|| "runtime:dom".to_string()),
            config.run.setup.clone(),
        )
    };
    // A rehearsal still applies here: this is also how every child of a
    // restricted parent executes, and the flags arrived on its command
    // line for exactly this run. A file that declares its own grant runs
    // under that instead.
    let (capabilities, scopes) =
        match file_capabilities(std::path::Path::new(&file), &config.run.permission_args) {
            Ok(resolved) => resolved,
            Err(err) => {
                print_error(&err);
                return ExitCode::FAILURE;
            }
        };
    let run = config
        .run
        .source
        .run_config(settings::Run {
            source: Source::File(file.clone()),
            args: Vec::new(),
            capabilities,
            scopes,
            options: RunOptions {
                track_pending_work: config.run.detect_leaks,
                ..RunOptions::default()
            },
            stripper,
            extensions: guest::test_extensions(config.run.dom),
            observer: None,
        })
        .await;
    let mut run = match run {
        Ok(run) => run,
        Err(err) => {
            print_error(&err);
            return ExitCode::FAILURE;
        }
    };
    // Under `--coverage`, V8's counts, collected by this process itself; under
    // `--inspect`, a debugger's endpoint for this file.
    if config.coverage_out.is_some() {
        run.inspector = Some(coverage::collect::inspector());
    } else if let Err(err) = attach_debugger(&mut run, config.run.inspect.as_ref()) {
        print_error(&err);
        return ExitCode::FAILURE;
    }
    let code = match es_runtime_cli_common::run("esdev", run).await {
        Ok(()) => finish_test_file(config, &file),
        Err(err) => {
            print_error(&err);
            // The run died, but the cases that already finished are still
            // results. Losing a hundred of them to one stray promise is
            // precisely the report a suite most needs to see, so it is
            // printed; the exit stays a failure either way.
            finish_test_file(config, &file);
            ExitCode::FAILURE
        }
    };
    // The results, for the parent to report and to count for `--bail`.
    if let Some(summary) = &config.summary {
        guest::test::write_summary(summary, &file);
    }
    if let Some(out) = &config.coverage_out {
        coverage::collect::write(out, &std::env::current_dir().unwrap_or_default());
    }
    code
}

/// The files this run takes, and how many were discovered: those a change
/// reaches, if it asked for them, and its shard of those, in the order it runs
/// them. What was chosen is said on stderr, beside the seed, since the report
/// alone does not say what was left out.
fn select_test_files(
    root: &std::path::Path,
    config: &TestConfig,
    announce: bool,
) -> Result<(Vec<std::path::PathBuf>, usize), String> {
    let mut files = test::discover(root, &config.filters);
    let discovered = files.len();
    if let Some(by) = &config.affected_by {
        let (changed, what) = match by {
            test::AffectedBy::Changed(since) => {
                let changed = related::changed_files(root, since.as_deref())?;
                let what = match since {
                    Some(since) => format!("since {since}"),
                    None => "uncommitted".to_string(),
                };
                (changed, what)
            }
            test::AffectedBy::Related(named) => (
                named.iter().map(|file| root.join(file)).collect(),
                "named".to_string(),
            ),
        };
        // The modules every file runs with: a change they reach, reaches all.
        let setup: Vec<_> = config
            .run
            .setup
            .iter()
            .chain(&config.global_setup)
            .filter_map(|module| match url::Url::parse(module) {
                Ok(url) => url.to_file_path().ok(),
                Err(_) => Some(root.join(module)).filter(|path| path.exists()),
            })
            .collect();
        files = related::affected(root, &config.run.source, &files, &setup, &changed)?;
        if announce {
            let label = match by {
                test::AffectedBy::Changed(_) => "changed",
                test::AffectedBy::Related(_) => "related",
            };
            let (count, plural) = (changed.len(), changed.len() != 1);
            eprintln!(
                "{label}: {count} file{} {what}; {} of {discovered} test files reach {}",
                if plural { "s" } else { "" },
                files.len(),
                if plural { "them" } else { "it" },
            );
        }
    }
    if let Some(shard) = config.shard {
        let before = files.len();
        test::shard(&mut files, root, shard);
        if announce {
            eprintln!("shard {shard}: {} of {before} files", files.len());
        }
    }
    test::shuffle(&mut files, config.run.seed);
    Ok((files, discovered))
}

fn validate_unisolated_test_config(config: &TestConfig) -> Result<(), &'static str> {
    if config.isolation != Some(TestIsolation::None) {
        return Ok(());
    }
    if config.jobs.is_some() {
        return Err("--isolation=none runs one process, so --jobs has no meaning");
    }
    if config.timeout.is_some() {
        return Err("--isolation=none has no per-file boundary, so --timeout is unavailable");
    }
    if !matches!(config.reporter.as_deref(), None | Some("human")) {
        return Err(
            "--isolation=none runs every file in one process, so it has no per-file results for a machine reporter",
        );
    }
    Ok(())
}

/// Runs every selected test module through one entry graph, retaining its V8
/// heap and module map. Static imports preserve discovery order, and ESM's
/// cache makes a dependency shared by two files evaluate once for this run.
///
/// There is deliberately no per-file timeout or JSON report in this mode:
/// neither has a meaningful boundary after isolation was disabled.
pub(crate) async fn run_tests_unisolated(
    files: &[std::path::PathBuf],
    config: &TestConfig,
) -> ExitCode {
    // A watch pass gets a fresh runtime in this host process. Its tally is
    // thread-local host bookkeeping, so start it fresh with the runtime.
    guest::test::reset();
    guest::test::configure_run(config.run_options());
    guest::test::configure_snapshots(
        None,
        config.run.update_snapshots,
        config.run.ci,
        config.run.full_diff,
        config.run.snapshot_prune,
    );
    let mut source = String::new();
    if config.run.dom {
        source.push_str("import \"runtime:dom\";");
    }
    source.push_str("import { __setTestFile } from \"runtime:test\";");
    // One process has one grant; a file that declares its own needs a process
    // of its own to have it.
    if let Some(file) = files.iter().find(|file| {
        std::fs::read_to_string(file)
            .ok()
            .and_then(|source| tags::declared_permissions(&source))
            .is_some()
    }) {
        print_error(&format!(
            "{} declares @permissions, and --isolation=none runs every file in one \
             process with one grant.\n\nDrop --isolation=none to run each file under its own.",
            file.display()
        ));
        return ExitCode::FAILURE;
    }
    for file in files {
        for setup in &config.run.setup {
            source.push_str(&module_import(setup));
        }
        let url = url::Url::from_file_path(file)
            .map(|url| url.to_string())
            .unwrap_or_else(|()| file.display().to_string());
        // With the file's `@module-tag`s, which tag the tests it registers.
        let module_tags = std::fs::read_to_string(file)
            .map(|source| tags::module_tags(&source))
            .unwrap_or_default();
        source.push_str("__setTestFile(");
        source.push_str(&serde_json::to_string(&url).expect("a URL always serializes as JSON"));
        source.push(',');
        source.push_str(&serde_json::to_string(&module_tags).expect("strings serialize as JSON"));
        source.push_str(");await import(");
        source.push_str(&serde_json::to_string(&url).expect("a URL always serializes as JSON"));
        source.push_str(");");
    }
    // A rehearsal resolved like any run: these files execute here rather than
    // in children, so there is no command line for them to re-parse.
    let (capabilities, scopes) = match test_capabilities(&config.run.permission_args) {
        Ok(resolved) => resolved,
        Err(err) => {
            print_error(&err);
            return ExitCode::FAILURE;
        }
    };
    // Compiled as each file would be in a child of its own.
    let run = config
        .run
        .source
        .run_config(settings::Run {
            source: Source::Inline(source),
            args: Vec::new(),
            capabilities,
            scopes,
            options: RunOptions::default(),
            stripper: TypeStripper::new(),
            extensions: guest::test_extensions(config.run.dom),
            observer: None,
        })
        .await;
    let mut run = match run {
        Ok(run) => run,
        Err(err) => {
            print_error(&err);
            return ExitCode::FAILURE;
        }
    };
    // Every file shares this process, so one debugger follows them all.
    if let Err(err) = attach_debugger(&mut run, config.run.inspect.as_ref()) {
        print_error(&err);
        return ExitCode::FAILURE;
    }
    match es_runtime_cli_common::run("esdev", run).await {
        Ok(()) => guest::test::finish(),
        Err(err) => {
            print_error(&err);
            // As above: what ran is reported, and the run still fails.
            guest::test::finish();
            ExitCode::FAILURE
        }
    }
}

fn module_import(specifier: &str) -> String {
    let quoted = serde_json::to_string(specifier).expect("a string always serializes as JSON");
    format!("import {quoted};")
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
        Ok(Command::Run(run, inspect, config)) => {
            run_module(*run, inspect.as_ref(), config.as_deref()).await
        }
        Ok(Command::Watch(config)) => watch::supervise(config).await,
        Ok(Command::Test(config)) => return run_tests(*config).await,
        Ok(Command::Build(request)) => build::run(request).await,
        Ok(Command::Start(config)) => start::start(*config).await,
        Ok(Command::Preview(config)) => preview::run(config).await,
        Ok(Command::Check(args)) => match std::env::current_dir() {
            Ok(root) => check::check(&root, &args).await,
            Err(e) => Err(format!("cannot read working directory: {e}")),
        },
        Ok(Command::Create(config)) => match create::create(&config) {
            Ok(report) => {
                print!("{report}");
                Ok(())
            }
            Err(err) => Err(err),
        },
        Ok(Command::Init(config)) => match init::init(&config) {
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
