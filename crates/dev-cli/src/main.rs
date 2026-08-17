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
mod prompt;
mod resolve;
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
    esdev create <dir>          Write a new project that already works
                                (`esdev create --list` for the templates)
    esdev start                 Build what esdev.json describes, run it, and
                                keep both current (`esdev start --help`)
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

                                  esrun --allow-read --allow-net app.js

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

RUN OPTIONS (the same flags esrun takes, with one deliberate difference):
    esdev grants every capability by default; esrun grants none. The vocabulary,
    the scope lists and the rules are identical — only the starting point
    differs, so that the inner loop needs no flags and a deployment states what
    it may reach. --trace-permissions turns one into the other.

    --allow-all, -A             Grant everything — the default, said outright
    --deny-<name>               Deny one capability; repeatable
    --deny-all                  Run with no host access at all, as esrun does
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
    assertThrows(fn, expected?, msg?)
    assertRejects(fn, expected?, msg?)

assertEquals compares structurally: BigInt and NaN, typed arrays and
ArrayBuffer by their bytes, Map and Set by contents, objects by their key set
rather than key order, and cycles terminate.

The `expected` error is a string (matched against the error's name, or as a
substring of its message), a RegExp (matched against the message), or a
constructor (an instanceof check). Omit it to accept any throw.

The same vocabulary the runtime's own conformance suite uses. Exits non-zero if
any file fails.
";

const CREATE_USAGE: &str = "\
esdev create — a project that already works

USAGE:
    esdev create <dir>          Write a new project into <dir>
    esdev create <dir> --template=<name>
                                ...from a particular template
    esdev create <dir> --template=<name> --mode=<name>
                                ...in a particular shape, where one exists
    esdev create --list         List the templates and their modes
    esdev create -h, --help     Show this help

OPTIONS:
    --template=<name>           Which template (default: react)
    --mode=<name>               Which shape of that template, where it has more
                                than one: react is static or fullstack
    --force                     Write into a directory that already holds
                                something. It still never replaces a file
    --install[=<manager>]       Install dependencies after writing: npm, bun,
                                pnpm or yarn (default npm)
    --no-install                Write the files and stop
    -y, --yes                   Take every default; never ask
    --list                      List the templates and their modes, and exit

WHAT YOU GET
    A project with its esdev.json written, its entry named by the script tag in
    its index.html, and a permission line that is narrow from the first run:

        esrun --allow-read=./dist --allow-listen=8080 dist/server.js

    The templates are baked into this binary, so `create` works offline and
    always writes a project this esdev can build.

MODES
    Some templates are two projects wearing one name, and scaffolding the union
    of them leaves you deleting half. `react` is one:

        --mode=static       No server. Prerendered HTML (npm run build) or a
                            single-page app (npm run build:spa), on any static
                            host. Nothing to grant, because nothing runs.
        --mode=fullstack    A server of its own, rendered per request, under the
                            capabilities esdev.json names.

    Which one is a deployment decision, so it is asked once, here, and the
    project you get is only that one.

ASKING
    On a terminal it asks which template, which mode if that template has more
    than one, and whether to install. Everywhere
    else — a pipe, a CI job, anything with CI set — it takes the defaults and
    says nothing, because a prompt in a script is a script that hangs.

    Every question has a flag, so nothing is only reachable by answering one:

        esdev create my-app --template=api --install=bun
        esdev create my-app --yes           (defaults, no questions)

    Unattended it installs nothing. There is no lockfile yet to say which
    package manager this project uses, and guessing wrong leaves the wrong one
    behind — which is a reason not to guess, not a reason not to ask.
";

const START_USAGE: &str = "\
esdev start — the dev loop: build, run, rebuild, reload

USAGE:
    esdev start [options]       Build what esdev.json describes, run it, and
                                keep both current

OPTIONS:
    --port=<n>                  The port you open, and it gets that one or
                                fails. For a project with a server of its own
                                that is your server's port; for a frontend
                                project it is the one esdev serves on. Without
                                it: your `listen` grant's port, or 5173 for a
                                frontend project, and any free port if that is
                                taken — the one it took is printed
    --config=<path>             Read this instead of ./esdev.json
    --shutdown-grace=<ms>       How long the server may drain on a restart
    -h, --help                  Show this help

WHAT IT DOES
    It is `esdev build` on a loop. A dev build differs from a release build in
    exactly two ways — process.env.NODE_ENV is \"development\", and nothing is
    content-hashed — and in nothing else, because a dev and a prod that
    disagree about how a module resolves is the failure this toolchain exists
    to prevent.

    On a change: rebuild, restart the server, tell the browser to reload. A
    build that fails leaves everything running — a syntax error mid-edit should
    cost you a message, not the server you were about to fix it on.

THE SERVER IS YOURS
    \"start\": { \"run\": \"server\" } names the target whose output is your
    server. esdev runs that output as a child process, under the config's
    `permissions`, and restarts it with a SIGTERM — the same graceful stop
    production gets, so a request in flight when you save is answered rather
    than dropped. It is the same file production runs: no dev server stands in
    for it and nothing wraps it.

    A project with no server of its own — a static site, a single-page app —
    has nothing to run, so esdev serves the output directory itself: files, an
    index.html fallback for client-side routes, and nothing else.

PORTS
    There is one, and it is the one you open. When your project runs a server
    of its own that is your server's port; when it does not -- a static site, a
    single-page app -- esdev is what you open, so it is esdev's.

    esdev's own endpoint on a fullstack project is not a port you deal with. It
    carries one message to the page, the build writes its address into the page,
    and it takes a free one. There is no flag for it because there is nobody to
    type one.

    Either way the rule is the same: one you named is a promise and fails if it
    is taken, one you did not is a convenience and moves out of the way,
    printing where it went.

    Your server's port is moved only when the project says enough for it to be
    moved safely, and both halves are grants you already write:

        \"listen\": [\"8080\"]     one port and no more, so there is one to move
        \"env\": [\"PORT\"]        so the server can be told which one it got

    The rewritten grant is the same capability with a different number, never a
    wider one. A project shaped any other way is left exactly as it is — so two
    of these run side by side without either of them being about a number
    nobody chose.

RELOAD
    Every built document gets a few lines that open an EventSource against
    esdev and reload when a build lands. It is esdev's endpoint rather than
    your application's, so nothing dev-only is in your source, and it is in the
    output only — the file you edit is never written to.

    Nothing is preserved across a reload: this is a full page load, not hot
    module replacement.
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
    --lib                       Build a library: keep the module structure,
                                leave dependencies external, emit .d.ts
    --no-types                  --lib only: skip the .d.ts files
    --dts-bundle[=<entry>]      --lib only: link every declaration into one
                                .d.ts instead of one beside each module.
                                Default entry: <srcdir>/index.ts
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

A PROJECT (esdev.json)
    An application that renders on the server and hydrates in the browser is
    two bundles from two entries with two shapes of output, and a command line
    can only describe one of them. What a project builds is a property of the
    project, so it lives in the project:

        {
          \"targets\": {
            \"server\":  { \"entry\": \"src/server.ts\", \"out\": \"dist/server.js\",
                        \"assets\": [\"index.html\", \"public\"] },
            \"browser\": { \"entry\": \"src/entry.client.tsx\", \"outdir\": \"dist/client\",
                        \"platform\": \"browser\" }
          }
        }

    `esdev build` then builds all of them, and `--target=browser` one. Each
    target takes:

      entry       The module the bundle is rooted at — or an .html file, which
                  is a different kind of build (see below)
      out         One file …
      outdir      … or a directory, which is what a browser target needs: a
                  dynamic import() emits a chunk beside its entry
      platform    \"server\" (this runtime, the default) or \"browser\", which
                  decides whether a dependency hands over its `worker` build or
                  its `browser` one
      assets      Files and directories copied into the output. A file by name,
                  a directory by its contents — so public/styles.css is served
                  at /styles.css, and dist/ is the whole deployment
      then        \"run\": execute the output once it is built. How a prerender
                  step emits a directory of HTML without esdev knowing what a
                  static site is
      minify, define, conditions
                  As the flags below, for this target alone

    A flag beats the file, so `--minify` takes a release build of a project
    whose day to day is unminified. Naming an entry ignores the file entirely.

    esrun never reads esdev.json. A production binary that picked up a
    checked-in file granting itself capabilities is the thing the capability
    model exists to prevent: the grant a service runs under belongs on the
    command that deployed it.

AN HTML ENTRY
    A server bundle starts at a module, because the runtime does. The browser
    starts at a document — so an .html entry is the build's input, and the tags
    in it name the rest:

        { \"targets\": { \"web\": { \"entry\": \"index.html\", \"outdir\": \"dist\" } } }

        <link rel=\"stylesheet\" href=\"./styles.css\">
        <script type=\"module\" src=\"./src/entry.client.tsx\"></script>

    A <script type=\"module\"> is an entry: it and everything it imports become
    one browser bundle. Anything else a relative reference names — a stylesheet,
    a favicon, an image, a classic script — is copied. Both are content-hashed
    into <outdir>/assets, and the document is written out pointing at them:

        <link rel=\"stylesheet\" href=\"/assets/styles-621d3b66.css\">
        <script type=\"module\" src=\"/assets/entry.client-fccaa347.js\"></script>

    Everything else in the file is untouched, byte for byte — the title, the
    meta tags, the inline snippet. A relative path is an input; a rooted path
    (/assets/vendor.js), a URL and a data: URI are left exactly as written,
    which is the escape hatch for anything the build should keep out of.

APPLICATION (the default)
    The bundle is ES modules, `runtime:*` imports are left for the runtime to
    serve, and CommonJS dependencies are converted on the way in — which is how
    a package that ships CJS becomes runnable without esrun learning `require`.

    It also shortens what production must be granted. An unbundled program needs
    --allow-imports so the loader can walk node_modules; a bundle has no imports
    left to resolve:

        esrun --allow-imports --allow-listen=8080 app.js  # unbundled
        esrun --allow-listen=8080 dist/app.js             # bundled

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
    * The output directory is emptied first, because the build owns it: a
      stale file left in dist is a file your package publishes. An --out that
      holds your source or your project is refused rather than emptied. An
      application build does not clean — its --out is one file, in a directory
      that may hold other things.
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

ONE DECLARATION FILE (--dts-bundle)
    A package whose exports map has a single entry wants one index.d.ts rather
    than a mirror of a source layout nobody outside it should have to know:

        esdev build --lib src --dts-bundle    # → dist/index.d.ts

    Everything reachable from the entry's exports is inlined; a colliding name
    is renamed (Options, Options$1) and every site of it rewritten; a type
    reachable only through a public one is inlined but not exported, so the
    package's surface stays what you wrote; dependencies stay imports, the same
    line --lib draws for JavaScript; and JSDoc travels byte for byte, because
    that is what an editor shows on hover.

    A construct that cannot be linked into one file — a namespace import,
    export =, a module augmentation — stops the build and names itself. A .d.ts
    is believed: nothing runs it and no test covers it, so a wrong one is worse
    than none. Build without --dts-bundle and it stands as written.

    Keep the per-module .d.ts if your exports map has subpaths: `@you/pkg/pool`
    has to find a real pool.d.ts.
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
    let mut options = RunOptions::default();
    for arg in args {
        let (flag, value) = split_flag_value(&arg);
        if options.try_flag(flag, value)? {
            continue;
        }
        match flag {
            "-h" | "--help" => {
                reject_value(flag, value)?;
                println!("{START_USAGE}");
                std::process::exit(0);
            }
            "--config" => config_path = Some(require_value(flag, value)?.to_string()),
            "--port" => {
                let given = require_value(flag, value)?;
                port =
                    Some(given.parse::<u16>().map_err(|_| {
                        format!("--port={given} is not a port number (1 to 65535).")
                    })?);
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
