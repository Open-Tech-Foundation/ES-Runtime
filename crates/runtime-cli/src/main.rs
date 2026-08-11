//! `esrun` — a standalone CLI that runs JavaScript on the ES-Runtime.
//!
//! This is the thin executable wrapper around the embeddable `runtime` library.
//! The wiring itself — the default tokio providers, the [`Runtime`], the module
//! load and the drive loop — lives in `es-runtime-cli-common` and is shared with
//! `esdev`, so a program behaves identically under either binary (SPEC.md §8).
//! What remains here is `esrun`'s own command line: its flags, its `types` and
//! `upgrade` subcommands, and the bundled type definitions they print.
//!
//! Every input runs as an ES module: `import`/`export` and top-level `await`
//! work. Imports resolve via `NodeModuleLoader`: relative/absolute paths and
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

use std::process::ExitCode;

use es_runtime_cli_common::args::{
    RunOptions, reject_value, require_value, split_flag_value, try_permission_flag,
};
use es_runtime_cli_common::diagnostics::print_error;
use es_runtime_cli_common::permissions::Permissions;
use es_runtime_cli_common::{Config, Source};

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

/// Bundled TypeScript definitions for the `runtime:` modules, printed by
/// `esrun types` (`esrun types > esrun.d.ts`) and also shipped in the release
/// archive. This is a static `&str` baked into the binary — it is read only
/// when `types` is invoked, so it adds nothing to startup or runtime cost
/// (just a few KB of binary size). The canonical source is `packages/types/` (published
/// as `@opentf/esrun-types`); kept byte-identical.
const TYPES: &str = concat!(
    include_str!("../../../packages/types/runtime-process.d.ts"),
    "\n",
    include_str!("../../../packages/types/runtime-path.d.ts"),
    "\n",
    include_str!("../../../packages/types/runtime-fs.d.ts"),
    "\n",
    include_str!("../../../packages/types/runtime-db.d.ts"),
    "\n",
    include_str!("../../../packages/types/runtime-net.d.ts"),
    "\n",
    include_str!("../../../packages/types/runtime-http.d.ts"),
    "\n",
    include_str!("../../../packages/types/runtime-websocket.d.ts"),
    "\n",
    include_str!("../../../packages/types/runtime-serialization.d.ts"),
    "\n",
    include_str!("../../../packages/types/runtime-hashing.d.ts"),
    "\n",
    include_str!("../../../packages/types/runtime-wasi.d.ts"),
    "\n",
    include_str!("../../../packages/types/runtime-system.d.ts"),
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

/// Parses `esrun`'s command line.
///
/// The shared flags (`--timeout`, `--env-file`, `--max-heap`, the permission
/// vocabulary, …) are handed to `cli-common` so that they mean the same thing
/// here as they do in `esdev`; what is matched below is what only `esrun` has.
fn parse_args() -> Result<Config, String> {
    let mut options = RunOptions::default();
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

#[cfg(test)]
mod tests {
    use super::TYPES;
    use es_runtime_common::Capability;

    /// `esrun types` must ship definitions for *every* `runtime:` module.
    ///
    /// The bundle is a hand-written `concat!`, so adding a module's `.d.ts` to
    /// `packages/types/` does not add it here — and the symptom is silent: the module
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
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packages/types");
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
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packages/types");
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
            "/../../packages/types/runtime-process.d.ts"
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
            "packages/types/runtime-process.d.ts is out of date"
        );
    }

    /// Every non-standard `WorkerOptions` member has to be described where an
    /// editor will find it. `memory` shipped without types once already.
    #[test]
    fn worker_options_are_typed() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packages/types/globals.d.ts"
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
                "packages/types/globals.d.ts does not declare WorkerOptions {member}"
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
        let dir =
            std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packages/types"));
        let index = std::fs::read_to_string(dir.join("index.d.ts")).expect("read index.d.ts");
        for entry in std::fs::read_dir(dir).expect("read types dir") {
            let path = entry.expect("dir entry").path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with("runtime-") || !name.ends_with(".d.ts") {
                continue;
            }
            assert!(
                index.contains(name),
                "{name} is not referenced by packages/types/index.d.ts"
            );
        }
    }
}
