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
//! Argument grammar: every flag is `--flag` or `--flag=value` — a value is never
//! a separate argument — and esrun's flags come **before** the script, since
//! everything after it belongs to the script.
//!
//! ```text
//! esrun script.mjs            # run a module file
//! esrun -e='console.log(1)'   # run an inline module snippet
//! esrun --timeout=500 app.js  # values attach with '='
//! esrun --version | --help
//! ```

// A CLI's whole job is to talk to the terminal.
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod dotenv;

use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use es_runtime::{HostProviders, InterruptHandle, ModuleEvalState, ModuleLoader, Process, Runtime};
use es_runtime_common::{Capability, CapabilitySet};
use es_runtime_default_providers::{DriveFailure, Driver};
use es_runtime_default_providers::{
    HostAllowlist, ImportPolicy, NodeModuleLoader, OsEntropy, PathAllowlist, ProcessBroadcastHub,
    ProcessPortHub, ReqwestTransport, SystemClock, SystemCommands, SystemEmbeddedDb,
    SystemFileSystem, SystemHttpServer, SystemNet, SystemProcess, SystemSignals,
    SystemSyncFileSystem, SystemWebSocket, ThreadWorkerHost, TokioTimers, WorkerProcess, path,
};
use es_runtime_providers::{Console, ConsoleLevel, ProviderError, Signal, WorkerScope, WorkerSpec};
use url::Url;

const USAGE: &str = "\
esrun — run JavaScript (ES modules) on the ES-Runtime

Every flag is either `--flag` or `--flag=value`. A value is never a separate
argument: `--timeout=500`, not `--timeout 500`.

USAGE:
    esrun <file>                Run a JavaScript module file
    esrun -e=<code>             Run an inline module snippet
    esrun --deny-all            Run with no host access at all (secure mode)
    esrun --deny-<name>         Deny one capability; repeatable
    esrun --allow-<name>        Grant one back; requires --deny-all; repeatable
                                <name> is one of: read, write, imports, net,
                                listen, env, run, signals, workers
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
    esrun types                 Print the runtime: TypeScript definitions
    esrun types --install       Install the definitions + wire up tsconfig.json
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

Every host capability is granted by default. Restrict a run in one of two ways,
never both — each has a single direction, so no flag ever overrides another:

    esrun --deny-net --deny-run app.js     # everything, minus these
    esrun --deny-all --allow-net app.js    # nothing, plus these
    esrun --deny-all --allow-net=api.example.com app.js   # ...and only there

--allow-<name> requires --deny-all (with everything already granted, there is
nothing for it to add). A denied operation throws NotAllowedError; importing a
runtime: module always works. --deny-all alone runs only the entry file: it can
compute, but cannot read, write, import another file, reach the network, read
the environment, or spawn anything. Ask from JS with `permissions.has(name)`
from runtime:process.

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
scope narrows a grant, so it is written --deny-all --allow-<name>=<list>.

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

/// The V8 startup snapshot with the prelude baked in, built by build.rs.
static SNAPSHOT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/prelude.snapshot.bin"));

/// Bundled TypeScript definitions for the `runtime:` modules, printed by
/// `esrun types` (`esrun types > esrun.d.ts`) and also shipped in the release
/// archive. This is a static `&str` baked into the binary — it is read only
/// when `types` is invoked, so it adds nothing to startup or runtime cost
/// (just a few KB of binary size). The canonical source is `types/` (published
/// as `@opentf/esrun-types`); kept byte-identical.
const TYPES: &str = concat!(
    include_str!("../../../types/runtime-process.d.ts"),
    "\n",
    include_str!("../../../types/runtime-path.d.ts"),
    "\n",
    include_str!("../../../types/runtime-fs.d.ts"),
    "\n",
    include_str!("../../../types/runtime-db.d.ts"),
    "\n",
    include_str!("../../../types/runtime-net.d.ts"),
    "\n",
    include_str!("../../../types/runtime-http.d.ts"),
    "\n",
    include_str!("../../../types/runtime-websocket.d.ts"),
    "\n",
    include_str!("../../../types/runtime-serialization.d.ts"),
    "\n",
    include_str!("../../../types/runtime-wasi.d.ts"),
    "\n",
    include_str!("../../../types/runtime-system.d.ts"),
);

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

/// `esrun types --install` — write the bundled definitions into
/// `node_modules/@opentf/esrun` as a type package and wire them into
/// `tsconfig.json`, so editors and `tsc` resolve the `runtime:*` modules with no
/// manual steps. (TypeScript only auto-loads ambient module declarations that a
/// `tsconfig` actually references, so we set `typeRoots` + `types` — the form
/// language servers honor globally — rather than leaving a loose `.d.ts`.)
fn install_types() -> Result<String, Box<dyn std::error::Error>> {
    use std::fs;
    use std::path::Path;

    let pkg_dir = Path::new("node_modules").join("@opentf").join("esrun");
    fs::create_dir_all(&pkg_dir)?;
    // index.d.ts (so typeRoots resolves the package by convention) + a minimal
    // package.json pointing at it.
    fs::write(pkg_dir.join("index.d.ts"), TYPES)?;
    fs::write(
        pkg_dir.join("package.json"),
        format!(
            "{{\n  \"name\": \"@opentf/esrun\",\n  \"version\": \"{}\",\n  \"types\": \"index.d.ts\"\n}}\n",
            env!("CARGO_PKG_VERSION")
        ),
    )?;

    let mut out = String::from("Installed runtime: types → node_modules/@opentf/esrun\n");
    out.push_str(&update_tsconfig()?);
    out.push('\n');
    Ok(out)
}

/// Ensures `tsconfig.json` resolves the installed type package: adds
/// `node_modules/@opentf` to `typeRoots` and `esrun` to `types`, preserving any
/// existing entries. Creates a sensible config if none exists; if the file is
/// JSONC (comments / trailing commas) it can't be parsed safely, so the lines to
/// add are printed instead of clobbering it.
fn update_tsconfig() -> Result<String, Box<dyn std::error::Error>> {
    use serde_json::{Value, json};
    use std::fs;
    use std::path::Path;

    let path = Path::new("tsconfig.json");
    let manual = "  add to compilerOptions:\n    \"typeRoots\": [\"node_modules/@types\", \"node_modules/@opentf\"],\n    \"types\": [\"esrun\"]";

    if !path.exists() {
        let cfg = json!({
            "compilerOptions": {
                "target": "ESNext",
                "module": "ESNext",
                "moduleResolution": "bundler",
                "strict": true,
                "typeRoots": ["node_modules/@types", "node_modules/@opentf"],
                "types": ["esrun"]
            },
            "include": ["**/*.ts"]
        });
        fs::write(path, format!("{}\n", serde_json::to_string_pretty(&cfg)?))?;
        return Ok("Created tsconfig.json (typeRoots + types).".into());
    }

    let text = fs::read_to_string(path)?;
    let mut cfg: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => {
            return Ok(format!(
                "tsconfig.json looks like JSONC (comments/trailing commas) — left it untouched.\n{manual}"
            ));
        }
    };
    let Some(obj) = cfg.as_object_mut() else {
        return Ok(format!(
            "tsconfig.json is not a JSON object — left it untouched.\n{manual}"
        ));
    };
    let co = obj.entry("compilerOptions").or_insert_with(|| json!({}));
    let Some(co) = co.as_object_mut() else {
        return Ok(format!(
            "tsconfig.json compilerOptions is not an object — left it untouched.\n{manual}"
        ));
    };
    merge_str_array(
        co,
        "typeRoots",
        &["node_modules/@types", "node_modules/@opentf"],
    );
    merge_str_array(co, "types", &["esrun"]);
    fs::write(path, format!("{}\n", serde_json::to_string_pretty(&cfg)?))?;
    Ok("Updated tsconfig.json (typeRoots + types).".into())
}

/// Appends any missing `values` to the string array at `key`, creating it if
/// absent. Existing entries (e.g. other `@types` packages) are preserved.
fn merge_str_array(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    values: &[&str],
) {
    use serde_json::Value;
    let arr = obj.entry(key).or_insert_with(|| Value::Array(vec![]));
    if let Value::Array(items) = arr {
        for v in values {
            if !items.iter().any(|x| x.as_str() == Some(*v)) {
                items.push(Value::String((*v).to_string()));
            }
        }
    }
}

/// A console that prints to the process's stdout/stderr, like Node/Deno.
struct StdoutConsole;

impl Console for StdoutConsole {
    fn write(&self, level: ConsoleLevel, message: &str) {
        use std::io::Write;

        // One `write_all` of message-plus-newline, under one lock, because
        // more than one agent writes here now: `writeln!` on the unlocked
        // handle can take the lock once for the message and again for the
        // newline, which is a window for another worker's line to land in the
        // middle of this one.
        let mut line = String::with_capacity(message.len() + 1);
        line.push_str(message);
        line.push('\n');

        if matches!(level, ConsoleLevel::Warn | ConsoleLevel::Error) {
            // Nothing useful to do if stderr itself is gone.
            let _ = std::io::stderr().lock().write_all(line.as_bytes());
            return;
        }

        if let Err(err) = std::io::stdout().lock().write_all(line.as_bytes()) {
            // A closed pipe is how `esrun script.js | head` ends, not a failure:
            // the reader took what it wanted and left. `println!` panics on it,
            // which turned an everyday shell idiom into a Rust backtrace on
            // stderr and an exit code of 1 — leaking internals for something
            // that is not the guest's fault and not ours. Node and Deno both
            // stop quietly here, so do the same.
            //
            // Any other write failure is equally unactionable from inside a
            // console sink, so it is dropped rather than raised into guest code.
            if err.kind() == std::io::ErrorKind::BrokenPipe {
                std::process::exit(0);
            }
        }
    }
}

/// Watches for an interrupt and, if the guest has not taken responsibility for
/// it, drains the HTTP servers instead of letting the process be killed.
///
/// The three-way split is the whole design:
///
/// * **The guest is watching this signal** — it installed a handler, so it owns
///   shutdown. Do nothing; racing its handler would be worse than useless.
/// * **No server is running** — there is no in-flight request to protect, so
///   exit at once. A script with a `setInterval` should still die instantly on
///   `^C`; waiting out a grace period there would be a regression, not a
///   feature.
/// * **Servers are running** — stop accepting, let in-flight requests answer,
///   and exit with the conventional 128+signal once they drain. `grace` is the
///   backstop for a handler that never finishes.
///
/// A second interrupt during the drain exits immediately: someone pressing `^C`
/// twice means it, and the first press has already been given its chance.
fn spawn_shutdown_watcher(
    signals: Arc<SystemSignals>,
    http: Arc<SystemHttpServer>,
    interrupt: InterruptHandle,
    grace: Duration,
) {
    let draining = Arc::new(AtomicBool::new(false));
    for signal in [Signal::Int, Signal::Term] {
        // Watching here also suppresses the default action, which is the point:
        // the process must survive long enough to drain. A platform that cannot
        // deliver this signal simply gets no watcher.
        let Some(mut stream) = watch_process_signal(signal) else {
            continue;
        };
        let (signals, http, interrupt, draining) = (
            signals.clone(),
            http.clone(),
            interrupt.clone(),
            draining.clone(),
        );
        tokio::spawn(async move {
            while stream.recv().await.is_some() {
                // The guest asked for this signal: its handler is the shutdown.
                if signals.is_watched(signal) {
                    continue;
                }
                if draining.swap(true, Ordering::SeqCst) {
                    // Second interrupt while draining — stop waiting.
                    std::process::exit(signal.exit_code());
                }
                if http.shutdown_all() == 0 {
                    // Nothing in flight to protect; behave as the default action
                    // would have.
                    std::process::exit(signal.exit_code());
                }
                eprintln!(
                    "esrun: {} received, draining in-flight requests (up to {}ms)",
                    signal.name(),
                    grace.as_millis()
                );
                // Backstop: a handler that never finishes must not outlive the
                // grace. Terminating the engine unblocks the drive loop, and the
                // exit code is the same either way.
                let handle = interrupt.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(grace).await;
                    handle.terminate();
                    std::process::exit(signal.exit_code());
                });
                // The drive loop reaches quiescence once the servers have
                // drained; record the code it should exit with.
                SHUTDOWN_CODE.store(signal.exit_code(), Ordering::SeqCst);
            }
        });
    }
}

/// The exit code a completed graceful shutdown should use, or `0` if no
/// interrupt was handled. Read once the drive loop returns.
static SHUTDOWN_CODE: AtomicI32 = AtomicI32::new(0);

/// A process-level stream for `signal`, or `None` where the platform has no such
/// signal to deliver. Separate from the guest's `Signals` provider on purpose:
/// this one is the *host's* shutdown behaviour, and both can watch the same
/// signal without competing for deliveries.
#[cfg(unix)]
fn watch_process_signal(signal: Signal) -> Option<tokio::signal::unix::Signal> {
    use tokio::signal::unix::{SignalKind, signal as unix_signal};
    let kind = match signal {
        Signal::Int => SignalKind::interrupt(),
        Signal::Term => SignalKind::terminate(),
        _ => return None,
    };
    unix_signal(kind).ok()
}

#[cfg(windows)]
fn watch_process_signal(signal: Signal) -> Option<tokio::signal::windows::CtrlC> {
    // Windows has no SIGTERM; Ctrl+C is the interrupt that exists.
    match signal {
        Signal::Int => tokio::signal::windows::ctrl_c().ok(),
        _ => None,
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
    /// `.env` file to load, via `--env-file` (last one wins if repeated).
    env_file: Option<String>,
    /// Import policy file, via `--import-policy` (D39). Never auto-discovered:
    /// like `--env-file`, nothing on disk is read unless it is named.
    import_policy: Option<String>,
    /// Whether `--env-file` values override the OS environment (`--env-override`).
    env_override: bool,
    /// How long in-flight HTTP requests get to finish after an interrupt, via
    /// `--shutdown-grace` (see [`DEFAULT_SHUTDOWN_GRACE`]).
    shutdown_grace: Duration,
    /// The heap ceiling in bytes, via `--max-heap=<mb>`; `None` sizes it from
    /// the host. See [`heap_limits`].
    max_heap_bytes: Option<usize>,
    /// User arguments after the script/`-e` code, exposed as `runtime:process`
    /// `args` (the runtime binary and the script/code are excluded).
    args: Vec<String>,
    /// What the script may reach, from `--deny-all` / `--deny-<name>` (D38).
    /// [`CapabilitySet::all`] unless denials were asked for.
    capabilities: CapabilitySet,
    /// The scope lists from `--allow-<name>=a,b`, keyed by capability (D38).
    /// A capability that is granted but absent here is granted **unnarrowed**;
    /// the narrowing itself is enforced provider-side, not by the capability
    /// bit — the bit only says whether the door exists.
    scopes: Scopes,
}

/// Scope lists by capability, in the order the user wrote them.
type Scopes = HashMap<Capability, Vec<String>>;

/// What a scoped `--allow-<name>=<list>` means for `cap`, or `None` if that
/// capability cannot enforce a list.
///
/// Seven of the eight take a list. `imports` deliberately does **not**: what
/// may be loaded is an [import policy](es_runtime_default_providers::ImportPolicy)
/// (`--import-policy=<file>`, D39), not a capability scope — the capability
/// decides whether the loader runs, the policy decides what it may resolve. The
/// `None` arm is the rule, not a placeholder: a capability rejects a value until
/// something enforces it (D38 — a run must never be narrower on the command line
/// than it is in reality).
fn scope_hint(cap: Capability) -> Option<&'static str> {
    match cap {
        Capability::Run => Some("program names, e.g. --allow-run=git,ls"),
        Capability::Env => Some("variable names, e.g. --allow-env=HOME,PATH"),
        Capability::Net => Some("hosts, e.g. --allow-net=api.example.com,db.internal:5432"),
        Capability::NetListen => Some("bind addresses, e.g. --allow-listen=127.0.0.1:8080,8443"),
        // One inside the project and one outside it: a path inside narrows the
        // root jail, and a path outside adds that subtree (D54), which is the
        // only way a run reaches a TLS certificate or a CA bundle.
        Capability::FileRead | Capability::FileWrite => {
            Some("paths, e.g. --allow-read=./data,/etc/ssl/certs")
        }
        Capability::Signals => Some("signal names, e.g. --allow-signals=SIGTERM,SIGINT"),
        _ => None,
    }
}

/// The signal list for `--allow-signals`, or `None` if `signals` was granted
/// whole. Names were validated when the flag was parsed.
fn signal_scope(scopes: &Scopes) -> Option<Vec<Signal>> {
    scopes
        .get(&Capability::Signals)
        .map(|names| names.iter().filter_map(|n| Signal::from_name(n)).collect())
}

/// The path list for `cap`, resolved against `base` — the working directory the
/// user typed the flags in, which is what a relative `./data` means to them.
/// `None` if that capability was granted whole.
fn path_scope(
    scopes: &Scopes,
    cap: Capability,
    base: &std::path::Path,
) -> Result<Option<PathAllowlist>, String> {
    scopes
        .get(&cap)
        .map(|entries| PathAllowlist::parse(entries, base))
        .transpose()
}

/// The address list for `cap`, parsed and validated, or `None` if that
/// capability was granted whole.
///
/// Parsed twice by design — once here at wiring time, once in
/// [`Permissions::record`] so a malformed entry is an *argument* error reported
/// before anything runs rather than a provider error three steps later. The
/// list is a handful of strings; the duplication buys the better message.
fn address_scope(scopes: &Scopes, cap: Capability) -> Result<Option<HostAllowlist>, String> {
    scopes.get(&cap).map(HostAllowlist::parse).transpose()
}

/// The permission flags accumulated while parsing, resolved into a
/// [`CapabilitySet`] once the whole command line has been seen (D38).
///
/// Three rules, and they exist so that **no flag ever overrides another** — a
/// reader goes top to bottom and the list is the answer:
///
/// 1. `--deny-all` and `--deny-<name>` are mutually exclusive. `--deny-all` is
///    precisely the union of the eight, so a combination could only be
///    redundant.
/// 2. `--allow-<name>` requires `--deny-all`. Against the default baseline
///    (everything granted) an allow is either a no-op or a contradiction of its
///    own `--deny-<name>` sibling.
/// 3. Each mode therefore has exactly one direction: `--deny-<name>` subtracts
///    from everything, `--deny-all --allow-<name>` adds to nothing.
#[derive(Default)]
struct Permissions {
    /// The `--deny-all` flag, if given.
    all: bool,
    /// `--deny-<name>` flags, in the order given — so an error can quote the one
    /// the user actually typed.
    denied: Vec<(String, Capability)>,
    /// `--allow-<name>` flags, likewise.
    allowed: Vec<Allow>,
}

/// One `--allow-<name>` flag as written.
struct Allow {
    /// The whole argument as the user typed it, scope list included, so an
    /// error can quote it back — `--allow-env` and `--allow-env=HOME` are
    /// different flags to a reader, and the message that tells them apart is
    /// the one complaining that they conflict.
    flag: String,
    cap: Capability,
    /// The entries of `--allow-<name>=a,b`, or `None` for the bare flag —
    /// "granted, unnarrowed".
    values: Option<Vec<String>>,
}

impl Permissions {
    /// Records a `--deny-<name>` / `--allow-<name>` flag, with the value it
    /// carried (if any).
    ///
    /// `name` has already been split off the `--deny-`/`--allow-` prefix and is
    /// rejected here if it is not one of the eight — never ignored, since an
    /// unrecognised permission flag would otherwise read as a sandbox that is
    /// not actually on.
    fn record(
        &mut self,
        flag: &str,
        name: &str,
        allow: bool,
        value: Option<&str>,
    ) -> Result<(), String> {
        let prefix = if allow { "--allow-" } else { "--deny-" };
        let cap = Capability::from_flag_name(name).ok_or_else(|| {
            let known = Capability::HOST_FACING
                .into_iter()
                .filter_map(Capability::flag_name)
                .map(|n| format!("{prefix}{n}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("unknown option: {flag}\n\nexpected one of: {known}")
        })?;
        if !allow {
            if let Some(value) = value {
                // Scoping has one direction, like everything else in D38: it
                // narrows a grant. A `--deny-net=host` would be the other one —
                // "everything except" — and rule 3 says a mode has exactly one.
                return Err(format!(
                    "{flag} takes no value (got {flag}={value}).\n\n\
                     A denial is all-or-nothing: scoping narrows a grant, so it is written \
                     as --deny-all --allow-{name}=<list>, never as a denial of specific \
                     values."
                ));
            }
            self.denied.push((flag.to_string(), cap));
            return Ok(());
        }
        let values = match value {
            None => None,
            Some(value) => {
                if scope_hint(cap).is_none() {
                    // Rejected, never ignored: a value that parsed but was not
                    // enforced would tell the user the run is scoped while the
                    // capability is wide open.
                    if cap == Capability::FileSystem {
                        return Err(format!(
                            "{flag} takes no value (got {flag}={value}).\n\n\
                             What may be *loaded* is an import policy, not a capability \
                             scope: the capability decides whether the loader runs, the \
                             policy decides what it may resolve. Use \
                             --import-policy=<file> — a JSON file with \"allow\" and/or \
                             \"deny\" lists of packages and paths."
                        ));
                    }
                    return Err(format!(
                        "{flag} takes no value (got {flag}={value}).\n\n\
                         Scoping {name} is not implemented — {flag} is all-or-nothing. \
                         It is rejected rather than ignored so a run is never narrower on the \
                         command line than it is in reality.\n\n\
                         A list works for: {}.",
                        scopable_flags()
                    ));
                }
                let entries = parse_scope_list(flag, value)?;
                // Validate the entry syntax now, while the flag is still the
                // thing being talked about: a bad address should be an argument
                // error naming the flag, not a provider failure at the first
                // connect.
                if matches!(cap, Capability::Net | Capability::NetListen) {
                    HostAllowlist::parse(&entries).map_err(|e| {
                        format!(
                            "{flag}: {e}\n\n\
                             An entry is a host (`example.com`), a host and port \
                             (`db.internal:5432`), or a bare port (`8080`, any interface). \
                             Bracket an IPv6 address that carries a port: `[::1]:8080`."
                        )
                    })?;
                }
                if cap == Capability::Signals {
                    // A name this runtime does not know would watch nothing, and
                    // read as protection that is not there.
                    for entry in &entries {
                        if Signal::from_name(entry).is_none() {
                            return Err(format!(
                                "{flag}: {entry} is not a signal name.\n\n\
                                 Expected one of: SIGINT, SIGTERM, SIGHUP, SIGUSR1, SIGUSR2, \
                                 SIGBREAK (what this platform can deliver is reported by \
                                 `signals()` from runtime:process)."
                            ));
                        }
                    }
                }
                Some(entries)
            }
        };
        self.allowed.push(Allow {
            flag: match value {
                Some(value) => format!("{flag}={value}"),
                None => flag.to_string(),
            },
            cap,
            values,
        });
        Ok(())
    }

    /// The scope lists these flags describe, or an error if a capability was
    /// both granted whole and narrowed.
    ///
    /// Repeating a scoped flag **unions** its entries (`--allow-run=git
    /// --allow-run=ls` ≡ `--allow-run=git,ls`), which keeps D38's rule that no
    /// flag ever overrides another: two flags that both add can be read in any
    /// order and the list is still the answer.
    fn scopes(&self) -> Result<Scopes, String> {
        let mut scopes: Scopes = HashMap::new();
        // The bare `--allow-<name>` that granted each capability whole, if any.
        let mut whole: HashMap<Capability, &str> = HashMap::new();
        for allow in &self.allowed {
            match &allow.values {
                None => {
                    if let Some(scoped) = scopes.keys().find(|cap| **cap == allow.cap) {
                        let scoped = self.first_scoped_flag(*scoped);
                        return Err(mixed_scope_error(scoped, &allow.flag));
                    }
                    whole.insert(allow.cap, &allow.flag);
                }
                Some(values) => {
                    if let Some(whole) = whole.get(&allow.cap) {
                        return Err(mixed_scope_error(&allow.flag, whole));
                    }
                    let entries = scopes.entry(allow.cap).or_default();
                    for value in values {
                        if !entries.contains(value) {
                            entries.push(value.clone());
                        }
                    }
                }
            }
        }
        Ok(scopes)
    }

    /// The first scoped flag written for `cap`, for an error message.
    fn first_scoped_flag(&self, cap: Capability) -> &str {
        self.allowed
            .iter()
            .find(|allow| allow.cap == cap && allow.values.is_some())
            .map_or("", |allow| allow.flag.as_str())
    }

    /// The capability set these flags describe, or an error naming the rule that
    /// was broken.
    fn resolve(&self) -> Result<CapabilitySet, String> {
        if let (true, Some((flag, _))) = (self.all, self.denied.first()) {
            return Err(format!(
                "--deny-all cannot be combined with {flag}: --deny-all already denies \
                 everything {flag} would. Use one or the other."
            ));
        }
        if let (false, Some(Allow { flag, .. })) = (self.all, self.allowed.first()) {
            return Err(format!(
                "{flag} requires --deny-all: everything is granted by default, so there is \
                 nothing for {flag} to add. Use --deny-all {flag} to start from nothing, or \
                 --deny-<name> to take single capabilities away."
            ));
        }
        if self.all {
            let mut caps = CapabilitySet::all().without_host_access();
            for allow in &self.allowed {
                // A scoped allow grants the same bit: the capability is what
                // opens the door, the scope list is what the provider then
                // refuses to hand over. `--allow-env=HOME` therefore reports
                // `has("env") === true`, which is the truth — the guest can
                // read *an* environment variable.
                caps.grant(allow.cap);
            }
            Ok(caps)
        } else {
            let mut caps = CapabilitySet::all();
            for (_, cap) in &self.denied {
                caps.revoke(*cap);
            }
            Ok(caps)
        }
    }
}

/// The `--allow-<name>=<list>` flags that accept a scope list today, for the
/// error that names them.
fn scopable_flags() -> String {
    Capability::HOST_FACING
        .into_iter()
        .filter(|cap| scope_hint(*cap).is_some())
        .filter_map(Capability::flag_name)
        .map(|name| format!("--allow-{name}=<list>"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The error for a capability that was both granted whole and narrowed.
///
/// There is no precedence rule to apply here and deliberately so (D38 rule 3):
/// taking the wider flag widens a run the user asked to narrow, and taking the
/// narrower one silently ignores a flag they typed. Both are the failure this
/// design exists to avoid, so the command line is rejected instead.
fn mixed_scope_error(scoped: &str, whole: &str) -> String {
    format!(
        "{scoped} and {whole} disagree: one narrows the grant to a list, the other grants it \
         whole.\n\n\
         No flag overrides another, so there is nothing to resolve this with. Pass only \
         {scoped} to narrow, or only {whole} to grant it all."
    )
}

/// Splits a scoped permission value into its entries — D38's value grammar,
/// one grammar for every capability that takes a list:
///
/// - entries are separated by commas, so `--allow-run=git,ls` is two programs;
/// - surrounding whitespace on each entry is trimmed, so `--allow-run="git, ls"`
///   is the same thing and quoting is a shell convenience, not a syntax;
/// - an **empty entry** (`a,,b`, a trailing comma) is an error, because a typo
///   must never silently change what the run may reach;
/// - a repeated entry is kept once, in first-written order.
fn parse_scope_list(flag: &str, value: &str) -> Result<Vec<String>, String> {
    if value.is_empty() {
        return Err(format!(
            "{flag}= has an empty value — write `{flag}=<list>` to narrow the grant, or the \
             bare `{flag}` to grant it whole."
        ));
    }
    let mut entries: Vec<String> = Vec::new();
    for entry in value.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            return Err(format!(
                "{flag}={value} has an empty entry.\n\n\
                 A stray or trailing comma is a typo, and a typo must not quietly change what \
                 the run may reach. Write the list as `{flag}=a,b`; spaces around an entry are \
                 trimmed."
            ));
        }
        if !entries.iter().any(|seen| seen == entry) {
            entries.push(entry.to_string());
        }
    }
    Ok(entries)
}

/// Splits `--flag=value` into its parts. A flag with no `=` yields `None`, which
/// is distinct from `--flag=` (an empty value) — the latter is a mistake worth
/// naming rather than treating as absent.
fn split_flag_value(arg: &str) -> (&str, Option<&str>) {
    match arg.split_once('=') {
        Some((flag, value)) => (flag, Some(value)),
        None => (arg, None),
    }
}

/// The value of a flag that requires one.
///
/// **The single grammar rule of this parser: a value attaches with `=`, never as
/// the next argument.** `--timeout 500` is rejected, not read.
///
/// One rule for every flag is the whole point. Two — a space form here, an `=`
/// form there — is how `--allow-net example.com app.js` silently runs
/// `example.com` as the script and hands `app.js` to it as an argument. With one
/// rule the parser never has to guess whether the next word belongs to the flag
/// or is the script, so there is nothing to guess wrong.
fn require_value<'a>(flag: &str, value: Option<&'a str>) -> Result<&'a str, String> {
    match value {
        Some(value) if !value.is_empty() => Ok(value),
        Some(_) => Err(format!("{flag}= has an empty value — use `{flag}=<value>`")),
        None => Err(format!(
            "{flag} requires a value, attached with '=': use `{flag}=<value>`.\n\n\
             A value is never a separate word: `{flag} <value>` would leave <value> \
             to be mistaken for the script to run."
        )),
    }
}

/// Rejects a value on a flag that takes none.
fn reject_value(flag: &str, value: Option<&str>) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    // Permission flags never reach here: `Permissions::record` owns their
    // values, since for them a value is sometimes a scope list and otherwise an
    // error that has to explain itself. `--deny-all` does — it is a mode switch
    // rather than a capability, so scoping could never apply to it.
    Err(format!("{flag} takes no value (got {flag}={value})"))
}

/// How long a graceful shutdown waits for in-flight HTTP requests before giving
/// up and exiting anyway. Long enough for an ordinary request to finish, short
/// enough that an orchestrator's own kill deadline (commonly 30s) is not the
/// thing that ends the process.
const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

fn parse_args() -> Result<Config, String> {
    let mut timeout = None;
    let mut env_file: Option<String> = None;
    let mut import_policy: Option<String> = None;
    let mut env_override = false;
    let mut shutdown_grace = DEFAULT_SHUTDOWN_GRACE;
    let mut max_heap_bytes = None;
    let mut permissions = Permissions::default();
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
        match flag {
            "-h" | "--help" => {
                reject_value(flag, value)?;
                println!("{USAGE}");
                std::process::exit(0);
            }
            "types" => {
                reject_value(flag, value)?;
                if args.next().as_deref() == Some("--install") {
                    match install_types() {
                        Ok(msg) => print!("{msg}"),
                        Err(e) => {
                            eprintln!("error: types --install failed: {e}");
                            std::process::exit(1);
                        }
                    }
                } else {
                    print!("{TYPES}");
                }
                std::process::exit(0);
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
            "-t" | "--timeout" => {
                let ms = require_value(flag, value)?;
                let ms: u64 = ms
                    .parse()
                    .map_err(|_| format!("invalid {flag} value: {ms} (expected milliseconds)"))?;
                timeout = Some(Duration::from_millis(ms));
            }
            "--env-file" => {
                env_file = Some(require_value(flag, value)?.to_string());
            }
            "--import-policy" => {
                import_policy = Some(require_value(flag, value)?.to_string());
            }
            "--env-override" => {
                reject_value(flag, value)?;
                env_override = true;
            }
            "--max-heap" => {
                let mb = require_value(flag, value)?;
                let mb: usize = mb.parse().map_err(|_| {
                    format!("invalid {flag} value: {mb} (expected whole megabytes)")
                })?;
                if mb == 0 {
                    return Err(format!("{flag}=0 would leave no heap at all"));
                }
                max_heap_bytes = Some(mb * 1024 * 1024);
            }
            "--shutdown-grace" => {
                let ms = require_value(flag, value)?;
                let ms: u64 = ms
                    .parse()
                    .map_err(|_| format!("invalid {flag} value: {ms} (expected milliseconds)"))?;
                shutdown_grace = Duration::from_millis(ms);
            }
            "--deny-all" => {
                reject_value(flag, value)?;
                permissions.all = true;
            }
            // A Deno habit, and it is the default here — say so rather than
            // rejecting it as an unknown option.
            "--allow-all" | "-A" => {
                return Err(
                    "there is no --allow-all: esrun grants every capability by default, so \
                     there is nothing to allow.\n\n\
                     Drop --deny-all to run unrestricted, or name what you need with \
                     --allow-<name>."
                        .to_string(),
                );
            }
            "-e" | "--eval" => {
                let code = require_value(flag, value)?.to_string();
                let rest: Vec<String> = args.collect();
                reject_esrun_flags_after_source(&rest, "the -e code")?;
                return Ok(Config {
                    source: Source::Inline(code),
                    timeout,
                    env_file,
                    import_policy,
                    env_override,
                    shutdown_grace,
                    max_heap_bytes,
                    args: rest,
                    capabilities: permissions.resolve()?,
                    scopes: permissions.scopes()?,
                });
            }
            flag if flag.starts_with("--deny-") || flag.starts_with("--allow-") => {
                let allow = flag.starts_with("--allow-");
                let prefix = if allow { "--allow-" } else { "--deny-" };
                let name = flag[prefix.len()..].to_string();
                permissions.record(flag, &name, allow, value)?;
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
                    timeout,
                    env_file,
                    import_policy,
                    env_override,
                    shutdown_grace,
                    max_heap_bytes,
                    args: rest,
                    capabilities: permissions.resolve()?,
                    scopes: permissions.scopes()?,
                });
            }
        }
    }
    Err(format!("missing script argument\n\n{USAGE}"))
}

/// Whether `flag` is one esrun itself understands.
fn is_esrun_flag(flag: &str) -> bool {
    if matches!(
        flag,
        "-h" | "--help"
            | "-v"
            | "-V"
            | "--version"
            | "-t"
            | "--timeout"
            | "--env-file"
            | "--import-policy"
            | "--env-override"
            | "--shutdown-grace"
            | "--max-heap"
            | "-e"
            | "--eval"
            | "--deny-all"
            | "--allow-all"
            | "-A"
    ) {
        return true;
    }
    ["--deny-", "--allow-"].iter().any(|prefix| {
        flag.strip_prefix(prefix)
            .is_some_and(|name| Capability::from_flag_name(name).is_some())
    })
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
use std::io::IsTerminal;

fn print_error(err: &str) {
    let use_color = std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    if !use_color {
        eprintln!("error: {}", err);
        return;
    }

    let mut lines = err.lines();
    if let Some(first) = lines.next() {
        eprintln!("\x1b[1;31merror\x1b[0m: {}", first);
    }
    for line in lines {
        if line.starts_with("    at ") {
            eprintln!("\x1b[2m{}\x1b[0m", line);
        } else {
            eprintln!("{}", line);
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            print_error(&err);
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    // Before anything that could log. Installing a subscriber is a
    // process-global act, so a library crate must not do it — which meant that
    // until this call existed, every `tracing` event the runtime emitted was
    // discarded, including the accept-loop failures that were written to be
    // read. Quiet by default (`warn`); `RUST_LOG` opens it up, e.g.
    // `RUST_LOG=runtime::http=debug`.
    es_runtime_common::telemetry::init_tracing();
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
    // `--allow-net=<hosts>` / `--allow-listen=<addresses>` narrow the addresses
    // the guest may reach and bind (D38). Every provider that opens a socket
    // consults the same list for its half of the pair — `net` and `listen` are
    // one capability each, and which API the guest used to get there is not
    // something the policy should care about.
    let allow_net = address_scope(&config.scopes, Capability::Net)?;
    let allow_listen = address_scope(&config.scopes, Capability::NetListen)?;
    let transport = ReqwestTransport::new().map_err(|e| format!("http transport: {e}"))?;
    let net = Arc::new(match allow_net.clone() {
        Some(allow) => transport.with_allowlist(allow),
        None => transport,
    });
    // Host process view for runtime:process (env/cwd/platform from the OS; args
    // are the user's, after the script/-e). A concrete handle is kept to read
    // the exit code a guest `process.exit()` may request. The `.env` file is
    // loaded only via explicit --env-file (never auto-discovered, D30); its
    // values override the OS env only with --env-override.
    let env_overlay = match &config.env_file {
        Some(file) => dotenv::load(std::path::Path::new(file))?,
        None => Vec::new(),
    };
    // `--allow-env=<names>` narrows the environment snapshot to those names
    // (D38): the capability bit opens the door, the provider decides what is
    // behind it. Unlisted variables are absent rather than unreadable, so the
    // guest cannot even enumerate the names of the host's secrets.
    // Read before `config` is taken apart below, and used for every agent this
    // process builds: the main one here, and each worker through `spec.limits`.
    let max_heap_bytes = config.max_heap_bytes;
    let mut system_process =
        SystemProcess::new(config.args).with_env(env_overlay, config.env_override);
    if let Some(names) = config.scopes.get(&Capability::Env) {
        system_process = system_process.with_env_allowlist(names.clone());
    }
    let process = Arc::new(system_process);
    // Filesystem view for runtime:fs: relative paths resolve under the entry's
    // directory, jailed to the same detected project root the loader uses (D25).
    let fs_root = path::detect_root(&base_dir);
    // `--allow-read=<paths>` / `--allow-write=<paths>` narrow the jail (D38).
    // Entries are resolved against the *working directory*, because that is
    // where the user typed them; the jail's own base is the entry file's
    // directory, which is not what `./data` means on a command line.
    let flag_dir = std::env::current_dir().unwrap_or_else(|_| base_dir.clone());
    let allow_read = path_scope(&config.scopes, Capability::FileRead, &flag_dir)?;
    let allow_write = path_scope(&config.scopes, Capability::FileWrite, &flag_dir)?;
    // An entry outside the jail is not an error: it *adds* that subtree (D54).
    // The jail is still the default boundary and guest code can never move it —
    // only a path typed here can, which is the deployment operator naming a
    // location the project does not contain. A TLS certificate under
    // /etc/letsencrypt is the case this exists for.
    // Both filesystem views take the same lists: `runtime:fs` and `runtime:wasi`
    // are two doors onto one filesystem, and a policy that differed between
    // them would be a bug wearing a feature's clothes.
    let mut file_system = SystemFileSystem::new(&base_dir, &fs_root);
    let mut sync_file_system = SystemSyncFileSystem::new(&base_dir, &fs_root);
    if let Some(allow) = &allow_read {
        file_system = file_system.with_read_allowlist(allow.clone());
        sync_file_system = sync_file_system.with_read_allowlist(allow.clone());
    }
    if let Some(allow) = &allow_write {
        file_system = file_system.with_write_allowlist(allow.clone());
        sync_file_system = sync_file_system.with_write_allowlist(allow.clone());
    }
    let file_system = Arc::new(file_system);
    // The same view, synchronously, for `runtime:wasi` — WASI's syscalls cannot
    // await. Same base and same jail, so both paths agree on what is reachable.
    let sync_file_system = Arc::new(sync_file_system);
    // Held here as well as in the providers: the interrupt watcher below needs
    // to ask the signal registry what the guest is watching, and to tell the
    // HTTP servers to stop accepting.
    // `--allow-signals=<names>` narrows which signals may be watched. A watch
    // suppresses the default action, so this is the privilege to decline to die
    // on request, granted one signal at a time.
    let signals = Arc::new(match signal_scope(&config.scopes) {
        Some(names) => SystemSignals::new().with_allowlist(names),
        None => SystemSignals::new(),
    });
    let http_server = Arc::new(match allow_listen.clone() {
        Some(allow) => SystemHttpServer::new().with_listen_allowlist(allow),
        None => SystemHttpServer::new(),
    });
    let mut system_net = SystemNet::new();
    if let Some(allow) = allow_net.clone() {
        system_net = system_net.with_allowlist(allow);
    }
    if let Some(allow) = allow_listen {
        system_net = system_net.with_listen_allowlist(allow);
    }
    let web_socket = match allow_net {
        Some(allow) => SystemWebSocket::new().with_allowlist(allow),
        None => SystemWebSocket::new(),
    };
    let commands = match config.scopes.get(&Capability::Run) {
        Some(programs) => SystemCommands::new().with_allowlist(programs.clone()),
        None => SystemCommands::new(),
    };
    let providers = HostProviders::new(
        clock.clone(),
        Arc::new(StdoutConsole),
        net,
        Arc::new(OsEntropy),
    )
    .with_process(process.clone())
    .with_signals(signals.clone())
    .with_file_system(file_system.clone())
    // `runtime:db`'s embedded engine resolves through the *same* filesystem
    // view, so a database is scoped by `--allow-read`/`--allow-write` exactly
    // as a file is — and the write-ahead log the engine opens beside it is
    // judged by the same list, rather than by nothing.
    .with_embedded_db(Arc::new(SystemEmbeddedDb::new(file_system)))
    .with_sync_file_system(sync_file_system)
    .with_net_provider(Arc::new(system_net))
    .with_http_server(http_server.clone())
    .with_web_socket(Arc::new(web_socket))
    // Child processes for runtime:system. Unrestricted unless
    // `--allow-run=<programs>` named the ones that may be spawned (D38) — the
    // same provider seam an embedder uses to grant Run without granting a shell.
    .with_commands(Arc::new(commands))
    // BroadcastChannel's agent cluster is this process: every worker `esrun`
    // starts shares the hub, so a channel opened in one reaches the rest.
    .with_broadcast(Arc::new(ProcessBroadcastHub::new()))
    // MessagePort queues, so a port transferred into a worker keeps working.
    .with_ports(Arc::new(ProcessPortHub::new()));
    // Module loader: relative/absolute/file: specifiers resolve as local files,
    // bare specifiers through node_modules (ESM packages only). Based at the
    // entry's directory, from which it detects the sandbox root (the project
    // root containing node_modules/package.json) — resolution is jailed under it
    // by default (D25). Held behind an Arc so dynamic import() can reach it.
    // `--import-policy=<file>` governs what the loader may resolve (D39) — a
    // layer above the `imports` capability, which governs whether it runs at
    // all. The entry file is unaffected: it is read before a loader exists, and
    // the user named it on the command line.
    let mut loader_impl =
        NodeModuleLoader::with_base_dir(&base_dir).map_err(|e| format!("module loader: {e}"))?;
    if let Some(file) = &config.import_policy {
        loader_impl = loader_impl.with_policy(ImportPolicy::from_file(std::path::Path::new(file))?);
    }
    let loader: Arc<dyn ModuleLoader> = Arc::new(loader_impl);

    // Workers. Each gets its own thread and its own isolate, built by this
    // factory *on that thread* — `V8Engine` is `!Send`, so it cannot be built
    // here and moved. Passing the factory rather than reaching for the engine
    // inside the provider is what keeps the worker path engine-agnostic.
    //
    // The snapshot is shared, not copied: `&'static [u8]` from `include_bytes!`,
    // so a worker restores the same blob the main agent did and starts as
    // cheaply.
    let worker_providers = providers.clone();
    let worker_process = process.clone();
    let worker_loader = loader.clone();
    // Late-bound, because the host and the runtimes it builds each need the
    // other: a worker must itself be able to start workers (the spec allows
    // nesting, and the capability chain is what bounds it), so the bundle its
    // runtime gets has to name the very host that is being constructed here.
    // Filled in immediately below, and only read on a worker thread — long
    // after.
    let worker_host_slot: Arc<std::sync::OnceLock<Arc<dyn es_runtime_providers::WorkerHost>>> =
        Arc::new(std::sync::OnceLock::new());
    let factory_slot = worker_host_slot.clone();
    let workers = Arc::new(ThreadWorkerHost::new(Arc::new(
        move |spec: &WorkerSpec, scope: Arc<dyn WorkerScope>| {
            let mut providers = worker_providers
                .clone()
                .with_worker_scope(scope)
                // `exit()` inside a worker stops that worker, not the program:
                // halting is already per-agent, but the exit *code* is recorded
                // on a shared provider, so a worker would otherwise decide what
                // the process exits with.
                .with_process(Arc::new(
                    WorkerProcess::new(worker_process.clone()).with_env(spec.env.clone()),
                ));
            if let Some(host) = factory_slot.get() {
                providers = providers.with_workers(host.clone());
            }
            let runtime = Runtime::with_snapshot_and_limits(SNAPSHOT, spec.limits, providers)
                .map_err(|e| ProviderError::Other(e.to_string()))?;
            Ok((runtime, worker_loader.clone()))
        },
    )));
    let _ = worker_host_slot.set(workers.clone());
    let providers = providers.with_workers(workers.clone());

    // Restore the prelude from the snapshot baked in at build time (build.rs)
    // instead of compiling + evaluating it — the bulk of construction cost.
    let mut runtime =
        Runtime::with_snapshot_and_limits(SNAPSHOT, heap_limits(max_heap_bytes), providers)
            .map_err(|e| e.to_string())?;
    // A local script is trusted by default: the full capability set (incl.
    // FileSystem, which module loading requires). `--deny-all` / `--deny-<name>`
    // narrow it (D38); the entry file has already been read by this point, so a
    // fully denied run still executes what the user named.
    runtime.set_capabilities(config.capabilities);

    // Graceful shutdown on ^C / SIGTERM. Installed before the module runs, so a
    // server that binds immediately is covered from its first request.
    spawn_shutdown_watcher(
        signals,
        http_server.clone(),
        runtime.interrupt_handle(),
        config.shutdown_grace,
    );

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
    // Failures are handed over *as they happen* rather than collected for a
    // quiescence that a server never reaches: an unhandled rejection or a throw
    // out of a timer in a long-running program was only printed when the
    // process finally exited, which for a listening server is never. They are
    // still counted, so the exit status is unchanged.
    // Buffered for one tick rather than printed from the sink directly, because
    // the entry module's *own* top-level throw also arrives here as an unhandled
    // rejection — and that failure is reported once, below, as an uncaught
    // exception naming the file. Holding each batch until the module's
    // evaluation has settled is what lets that one be dropped instead of
    // printed twice; for a long-running server (the case this exists for) the
    // module settled long ago and a tick is no delay at all.
    let pending = Arc::new(Mutex::new(Vec::<Failure>::new()));
    let sink = pending.clone();
    let driver = Driver::new(clock, timers).reporting_failures_to(move |failure| {
        let (headline, error) = match failure {
            DriveFailure::UncaughtError(e) => ("uncaught exception in a timer callback", e),
            DriveFailure::UnhandledRejection(e) => ("unhandled promise rejection", e),
            // The enum is non-exhaustive; anything added later is still reported
            // rather than silently dropped.
            other => {
                sink.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(Failure {
                        text: format!("error: {other:?}"),
                        body: String::new(),
                    });
                return;
            }
        };
        let body = error.to_string();
        sink.lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Failure {
                text: format!("error: {headline}\n{body}"),
                body,
            });
    });
    let reported = Arc::new(AtomicI32::new(0));
    // Stopped as soon as the entry module's evaluation *fails*, rather than at
    // quiescence. A program whose top-level code threw has already failed, and
    // anything it started before throwing — a server holding a listener is the
    // ordinary case — keeps the loop alive forever, so waiting for the drive to
    // return meant the exception was never reported and the process never
    // exited. It ran on, serving, with the error discarded.
    //
    // Reported below by the existing `ModuleEvalState::Failed` check, which
    // until now could only be reached by programs that happened to quiesce.
    let flush_pending = pending.clone();
    let flush_count = reported.clone();
    let drive = driver.drive_while(&mut runtime, move |rt| {
        let state = rt.module_eval_state();
        if !matches!(state, ModuleEvalState::Pending) {
            flush_failures(&flush_pending, &flush_count, &state);
        }
        !matches!(state, ModuleEvalState::Failed(_))
    });
    let outcome = match config.timeout {
        Some(deadline) => match tokio::time::timeout(deadline, drive).await {
            Ok(outcome) => outcome,
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

    // The drive returned because a graceful shutdown drained the servers. The
    // guest is done, but its last responses have only been *handed* to the HTTP
    // transport — exiting now would turn them into empty replies, which is the
    // very failure the drain exists to prevent. Wait for the connections to
    // close, then report the interruption in the status an orchestrator reads.
    let shutdown_code = SHUTDOWN_CODE.load(Ordering::SeqCst);
    if shutdown_code != 0 {
        if !http_server.wait_for_idle(config.shutdown_grace).await {
            eprintln!("esrun: shutdown grace expired with requests still in flight");
        }
        std::process::exit(shutdown_code);
    }

    // A top-level throw (or a rejected top-level await) fails the module's
    // evaluation. Report it as the primary error — its rejection also shows up
    // in `rejections`, so it is the one uncaught-rejection we don't re-report.
    if let ModuleEvalState::Failed(message) = runtime.module_eval_state() {
        return Err(format!("uncaught exception in {label}\n{message}"));
    }

    // Anything the drive stopped before flushing, on the same terms.
    flush_failures(&pending, &reported, &runtime.module_eval_state());

    // Everything was printed the moment it happened, so this is only the exit
    // status: repeating the messages would report each failure twice.
    let count = reported.load(Ordering::SeqCst);
    if count > 0 {
        let plural = if count == 1 { "" } else { "s" };
        return Err(format!(
            "{count} unhandled failure{plural} — reported above"
        ));
    }
    let _ = &outcome;
    Ok(())
}

/// The ceilings this run's agents are built with.
///
/// `esrun` is not the embeddable library: it *is* the process, so it takes the
/// machine's answer rather than the library's conservative 256 MiB — which on a
/// 16 GiB host is a sixteenth of what Node and Deno would give the same script,
/// and does not move when the host does. `--max-heap=<mb>` pins it instead.
///
/// It applies to workers as well, because a worker derives its limits from the
/// agent that started it: one number bounds the process, however many agents it
/// ends up with.
fn heap_limits(max_heap_bytes: Option<usize>) -> es_runtime_common::Limits {
    let limits = es_runtime_common::Limits::default();
    match max_heap_bytes {
        Some(bytes) => limits.with_heap_limit_bytes(bytes),
        None => limits.with_system_heap_limit(),
    }
}

/// One failure the drive handed over, with the body kept separately so the
/// entry module's own rejection can be recognised (see [`flush_failures`]).
struct Failure {
    text: String,
    body: String,
}

/// Prints the buffered failures, dropping the one that *is* the entry module's
/// evaluation failure — that one is reported once, by name, as an uncaught
/// exception. Everything else is a failure the guest left unclaimed while it
/// ran, and is printed at the point it happened rather than at exit, which for
/// a program that never quiesces (a listening server) never came.
fn flush_failures(
    pending: &Arc<Mutex<Vec<Failure>>>,
    reported: &Arc<AtomicI32>,
    state: &ModuleEvalState,
) {
    let module_failure = match state {
        ModuleEvalState::Failed(e) => Some(e.to_string()),
        _ => None,
    };
    for failure in pending.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
        if module_failure.as_deref() == Some(failure.body.as_str()) {
            continue;
        }
        eprintln!("{}", failure.text);
        reported.fetch_add(1, Ordering::SeqCst);
    }
}

fn timeout_message(timeout: Option<Duration>) -> String {
    match timeout {
        Some(d) => format!("execution timed out after {} ms", d.as_millis()),
        None => "execution timed out".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::TYPES;
    use es_runtime_common::Capability;

    /// `esrun types` must ship definitions for *every* `runtime:` module.
    ///
    /// The bundle is a hand-written `concat!`, so adding a module's `.d.ts` to
    /// `types/` does not add it here — and the symptom is silent: the module
    /// simply has no types, in an editor, for whoever installed them. This walks
    /// the directory instead of trusting the list.
    /// Every declaration file must actually be published.
    ///
    /// `index.d.ts` references its siblings, so one missing from the npm
    /// package is not a gap — it is an installed package that cannot resolve
    /// itself. The list in `package.json` had drifted twice; it is a glob now,
    /// and this asserts the glob still covers everything.
    #[test]
    fn every_types_file_is_publishable() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../types");
        let manifest =
            std::fs::read_to_string(format!("{dir}/package.json")).expect("read manifest");
        let files: Vec<&str> = manifest
            .split("\"files\"")
            .nth(1)
            .expect("a files list")
            .split(']')
            .next()
            .expect("a closing bracket")
            .split('"')
            .filter(|s| s.ends_with(".d.ts") || s.ends_with(".md"))
            .collect();
        let covers_declarations = files.contains(&"*.d.ts");
        for entry in std::fs::read_dir(dir).expect("read types dir") {
            let name = entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned();
            if !name.ends_with(".d.ts") {
                continue;
            }
            assert!(
                covers_declarations || files.contains(&name.as_str()),
                "{name} is not in the published file list, so an installed \
                 @opentf/esrun-types could not resolve it"
            );
        }
    }

    #[test]
    fn every_types_file_is_bundled() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../types");
        let mut checked = 0;
        for entry in std::fs::read_dir(dir).expect("read types dir") {
            let path = entry.expect("dir entry").path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // `index.d.ts` is the reference list for the npm package, not a
            // module declaration, and carries no `declare module` of its own.
            if !name.starts_with("runtime-") || !name.ends_with(".d.ts") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read types file");
            let declaration = source
                .lines()
                .find(|line| line.starts_with("declare module "))
                .unwrap_or_else(|| panic!("{name} has no `declare module` line"));
            assert!(
                TYPES.contains(declaration),
                "{name} is not bundled into `esrun types` (missing {declaration})"
            );
            checked += 1;
        }
        assert!(checked >= 8, "only found {checked} module definitions");
    }

    /// The TypeScript `PermissionName` union is the last hand-written copy of
    /// the denial vocabulary — Rust owns it, and the two JS readers now ask the
    /// host for it — so this is where it can silently fall behind. An editor
    /// would then reject a name the runtime accepts, or accept one it does not.
    #[test]
    fn the_permission_union_matches_the_capabilities() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../types/runtime-process.d.ts"
        ))
        .expect("read runtime-process.d.ts");
        let union = source
            .split_once("export type PermissionName =")
            .expect("PermissionName union")
            .1
            .split_once(';')
            .expect("union terminator")
            .0;
        let listed: Vec<&str> = union
            .split('|')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(|part| part.trim_matches('"'))
            .collect();
        let expected: Vec<&str> = Capability::HOST_FACING
            .iter()
            .filter_map(|capability| capability.flag_name())
            .collect();
        assert_eq!(
            listed, expected,
            "types/runtime-process.d.ts is out of date"
        );
    }

    /// Every non-standard `WorkerOptions` member has to be described where an
    /// editor will find it. `memory` shipped without types once already.
    #[test]
    fn worker_options_are_typed() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../types/globals.d.ts"
        ))
        .expect("read globals.d.ts");
        for member in [
            "permissions?:",
            "env?:",
            "memory?:",
            "unref(): void",
            "ref(): void",
        ] {
            assert!(
                source.contains(member),
                "types/globals.d.ts does not declare WorkerOptions {member}"
            );
        }
        assert!(
            source.contains(r#""inherit" | readonly import("runtime:process").PermissionName[]"#),
            "permissions should be typed as \"inherit\" or the shared name union"
        );
    }

    /// `index.d.ts` is what the published npm package loads, so a file missing
    /// from it is invisible to anyone consuming the package.
    #[test]
    fn every_types_file_is_referenced_by_the_index() {
        let dir = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../types"));
        let index = std::fs::read_to_string(dir.join("index.d.ts")).expect("read index.d.ts");
        for entry in std::fs::read_dir(dir).expect("read types dir") {
            let path = entry.expect("dir entry").path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with("runtime-") || !name.ends_with(".d.ts") {
                continue;
            }
            assert!(
                index.contains(name),
                "{name} is not referenced by types/index.d.ts"
            );
        }
    }
}
