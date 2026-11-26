//! The shared surface: module loading, capabilities, the flag grammar and the error block, as `esrun` presents them.

use super::*;

#[test]
fn runs_a_module_file() {
    let app = write("run.mjs", "console.log('ran', 6 * 7);\n");
    let out = esdev().arg(&app).output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "ran 42");
}

/// esdev's compiler says what it turns into a module (D209): TypeScript loads
/// because the stripper claims it, and a file nothing claims is refused before
/// it is read, as under esrun.
#[test]
fn typescript_imports_load_and_a_file_that_is_not_a_module_is_refused() {
    write("d209-util.ts", "export const n: number = 41;\n");
    write("d209-creds.yaml", "password: hunter2\n");
    let app = write(
        "d209-main.ts",
        "import { n } from './d209-util.ts';\n\
         console.log(n + 1);\n\
         try { await import('./d209-creds.yaml'); console.log('LOADED'); }\n\
         catch (e) { console.log((e as { code: string }).code); }\n",
    );
    let out = esdev().arg(&app).output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "42\nERR_UNKNOWN_FILE_EXTENSION\n");
}

/// esdev starts from everything, so a Unix socket path needs no flag there —
/// and under `--deny-all` it must be named, as in esrun (D140).
#[cfg(unix)]
#[test]
fn a_socket_path_follows_the_grant_esdev_starts_from() {
    let dir = std::env::temp_dir().join(format!("esdev-uds-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let path = dir.join("app.sock").display().to_string();
    let code = format!(
        "-e=import {{ listen }} from 'runtime:net';\
         try {{ const l = listen({{ path: '{path}' }}); await l.addr; await l.close(); console.log('ok'); }}\
         catch (e) {{ console.log(e.code); }}"
    );
    let out = esdev().arg(&code).output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "ok");

    let out = esdev()
        .args(["--deny-all", "--allow-listen"])
        .arg(&code)
        .output()
        .expect("spawn esdev");
    assert_eq!(
        stdout(&out).trim(),
        "ERR_PERMISSION_DENIED",
        "{}",
        stderr(&out)
    );

    let out = esdev()
        .args(["--deny-all", &format!("--allow-listen=unix:{path}")])
        .arg(&code)
        .output()
        .expect("spawn esdev");
    assert_eq!(stdout(&out).trim(), "ok", "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn runs_an_inline_snippet() {
    let out = esdev()
        .arg("-e=console.log('inline')")
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "inline");
}

/// `-e` is source being edited, so a type annotation strips like it would in a
/// `.ts` file instead of reaching the engine as a syntax error.
#[test]
fn runs_a_typescript_inline_snippet() {
    let out = esdev()
        .arg("-e=const n: number = 41; console.log(n + 1)")
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "42");
}

/// JSX in a snippet stays refused: a snippet has no file for a `jsx-runtime`
/// import to resolve against, so `-e` does not compile it.
#[test]
fn an_inline_snippet_with_jsx_is_refused() {
    let out = esdev()
        .arg("-e=const x = <div/>; console.log(x)")
        .output()
        .expect("spawn esdev");
    assert!(
        !out.status.success(),
        "JSX compiled in a snippet: {}",
        stdout(&out)
    );
    assert_eq!(stdout(&out).trim(), "");
}

#[test]
fn top_level_await_and_imports_work() {
    let dep = write("dep.mjs", "export const answer = 42;\n");
    // A `file:` URL rather than the path as written: on Windows an absolute
    // path is a URL whose scheme is `c`, so the import never reaches the
    // loader as a file — the same spelling the `esrun` module suite uses.
    let dep = es_runtime_default_providers::path::file_url(&dep)
        .expect("a file URL")
        .to_string();
    let app = write(
        "tla.mjs",
        &format!("const m = await import({dep:?});\nconsole.log(m.answer);\n"),
    );
    let out = esdev().arg(&app).output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "42");
}

#[test]
fn reports_its_own_name_in_version_and_help() {
    let version = esdev().arg("--version").output().expect("spawn esdev");
    assert!(
        stdout(&version).starts_with("esdev "),
        "{}",
        stdout(&version)
    );
    // `-v` and `-V` are the same flag in two spellings, and the help says so.
    for flag in ["-v", "-V"] {
        let out = esdev().arg(flag).output().expect("spawn esdev");
        assert!(out.status.success(), "{flag} failed");
        assert!(stdout(&out).starts_with("esdev "), "{flag}");
    }

    let help = esdev().arg("--help").output().expect("spawn esdev");
    let text = stdout(&help);
    assert!(text.contains("esdev"), "{text}");
    assert!(text.contains("-V"), "{text}");
    // The boundary is part of the help, not just the docs: this binary is not a
    // deployment target and the usage text has to say so.
    assert!(text.contains("not a deployment target"), "{text}");
    // Every subcommand is reachable from the help; `upgrade` is the newest and
    // the one nothing else would tell you about.
    for command in ["create", "start", "build", "test", "upgrade"] {
        assert!(
            text.contains(command),
            "{command} is not in the help:\n{text}"
        );
    }
}

#[test]
fn build_help_names_module_and_stylesheet_entry_types() {
    let out = esdev()
        .args(["build", "--help"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    let help = stdout(&out);
    assert!(help.contains(".js/.mjs/.ts/.tsx/.jsx module"), "{help}");
    assert!(help.contains("<entry.css>"), "{help}");
}

/// A document is not a module: naming an HTML file as the build entry is
/// refused with where it belongs (an esdev.json target), rather than fed to
/// the module bundler whose JSX parse error is three steps from the cause.
#[test]
fn a_document_build_entry_is_refused_with_a_pointer_to_targets() {
    write("build-entry-index.html", "<!DOCTYPE html>\n");
    let out = esdev()
        .args(["build", "./build-entry-index.html"])
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success(), "built a document as a module");
    let message = slash_paths(&stderr(&out));
    assert!(message.contains("is a document"), "{message}");
    assert!(message.contains("esdev.json"), "{message}");
    assert!(!message.contains("JSX"), "{message}");
}

/// The help used to claim every flag but `--file`/`--watch` was also an
/// esdev.json key; `--update-snapshots`, `--ci` and `--full-diff` are flags
/// only. Both halves are pinned: what the help says, and that the file
/// refuses the key.
#[test]
fn test_help_names_which_options_live_in_the_file() {
    let out = esdev()
        .args(["test", "--help"])
        .output()
        .expect("spawn esdev test --help");
    let text = stdout(&out);
    assert!(
        text.contains("are also esdev.json"),
        "help does not name the file keys:\n{text}"
    );
    assert!(
        !text.contains("Everything but --file and --watch"),
        "help still claims every flag is a key:\n{text}"
    );

    let dir = build_dir("t_test_keys");
    write_in(
        &dir,
        "one.test.ts",
        "import { test } from \"runtime:test\";\ntest(\"x\", () => {});\n",
    );
    write_in(&dir, "app.mjs", "console.log(1);\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{"test": {"updateSnapshots": true}, "build": {"targets": {"app": {"entry": "app.mjs", "out": "dist/app.js"}}}}"#,
    );
    let ran = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        !ran.status.success(),
        "a flag-only option was accepted as a key"
    );
    assert!(
        stderr(&ran).contains("unknown key `updateSnapshots`"),
        "{}",
        stderr(&ran)
    );
}

/// The help for `esdev test` is where the test API is learned, and it described
/// an ambient `test()` that D71 replaced with an import — so a file written from
/// it failed with `ReferenceError: test is not defined`. Both halves are pinned:
/// what the help says, and that a file written from it runs.
#[test]
fn test_help_documents_the_api_that_exists() {
    let out = esdev()
        .args(["test", "--help"])
        .output()
        .expect("spawn esdev test --help");
    let text = stdout(&out);
    assert!(text.contains(r#"from "runtime:test""#), "{text}");
    assert!(!text.contains("globals already defined"), "{text}");

    let dir = build_dir("t_help_api");
    write_in(
        &dir,
        "help.test.ts",
        "import { test, assert, assertEquals, assertThrows, assertRejects } from \"runtime:test\";\n\
         test(\"it adds\", () => {\n\
         \u{20}\u{20}assertEquals(1 + 1, 2);\n\
         \u{20}\u{20}assert(true);\n\
         \u{20}\u{20}assertThrows(() => {\n\
         \u{20}\u{20}\u{20}\u{20}throw new TypeError(\"no\");\n\
         \u{20}\u{20}}, TypeError);\n\
         });\n\
         test(\"it rejects\", async () => {\n\
         \u{20}\u{20}await assertRejects(() => Promise.reject(new Error(\"nope\")));\n\
         });\n",
    );
    let ran = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        ran.status.success(),
        "the documented API does not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The capability model is the whole reason the two binaries are separate, so
/// `esdev` must enforce it exactly as `esrun` does — a dev binary that quietly
/// granted more would make every permission flag a developer tests with a lie.
#[test]
fn deny_all_denies_under_esdev_too() {
    let out = esdev()
        .arg("--deny-all")
        .arg("-e=const { env } = await import('runtime:process'); console.log(env.HOME);")
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("NotAllowedError"), "{}", stderr(&out));
}

#[test]
fn a_scoped_grant_narrows_and_still_reports_the_capability() {
    let app = write(
        "scoped.mjs",
        "import { env, permissions } from 'runtime:process';\n\
         console.log('has:', permissions.has('env'));\n\
         console.log('KEPT:', env.KEPT);\n\
         console.log('HIDDEN:', env.HIDDEN);\n",
    );
    let out = esdev()
        .arg("--deny-all")
        .arg("--allow-env=KEPT")
        .arg(&app)
        .env("KEPT", "yes")
        .env("HIDDEN", "no")
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("has: true"), "{text}");
    assert!(text.contains("KEPT: yes"), "{text}");
    assert!(text.contains("HIDDEN: undefined"), "{text}");
}

/// D38 rule 2, enforced by the shared grammar rather than by a copy of it.
#[test]
fn allow_without_deny_all_is_rejected() {
    let out = esdev()
        .arg("--allow-net")
        .arg("-e=1")
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("requires --deny-all"),
        "{}",
        stderr(&out)
    );
}

/// D38 rule 1.
#[test]
fn deny_all_cannot_be_combined_with_a_named_denial() {
    let out = esdev()
        .arg("--deny-all")
        .arg("--deny-net")
        .arg("-e=1")
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("cannot be combined"),
        "{}",
        stderr(&out)
    );
}

/// The single grammar rule: a value attaches with `=`, never as the next word.
#[test]
fn a_separated_value_is_rejected_rather_than_read() {
    let out = esdev()
        .arg("--timeout")
        .arg("500")
        .arg("-e=1")
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("requires a value, attached with '='"),
        "{}",
        stderr(&out)
    );
}

/// Order is part of the grammar, and a silently-ignored `--deny-*` is a security
/// failure rather than a no-op — so it is an error under `esdev` as well.
#[test]
fn a_flag_after_the_script_is_rejected() {
    let app = write("after.mjs", "console.log('x');\n");
    let out = esdev()
        .arg(&app)
        .arg("--deny-net")
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success());
    let text = stderr(&out);
    assert!(text.contains("appears after"), "{text}");
    assert!(
        text.contains("esdev's flags come before the script"),
        "{text}"
    );
}

#[test]
fn a_script_argument_after_a_double_dash_is_left_alone() {
    let app = write(
        "args.mjs",
        "import { args } from 'runtime:process';\nconsole.log(args.join(','));\n",
    );
    let out = esdev()
        .arg(&app)
        .arg("--")
        .arg("--deny-net")
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("--deny-net"), "{}", stdout(&out));
}

#[test]
fn the_watchdog_stops_a_runaway() {
    let out = esdev()
        .arg("--timeout=200")
        .arg("-e=while (true) {}")
        .output()
        .expect("spawn esdev");
    // The same status as esrun's (D238): the run is shared, so is its ending.
    assert_eq!(out.status.code(), Some(124), "{}", stderr(&out));
    assert!(stderr(&out).contains("timed out"), "{}", stderr(&out));
}

/// The async timeout esrun used to race to `0` (D238), under esdev too: a run
/// parked on a timer that outlives the deadline. (An await nothing can settle
/// is reported as such before any deadline, D248.)
#[test]
fn the_watchdog_stops_a_parked_await() {
    let out = esdev()
        .arg("--timeout=150")
        .arg("-e=await new Promise((r) => setTimeout(r, 60000))")
        .output()
        .expect("spawn esdev");
    assert_eq!(out.status.code(), Some(124), "{}", stderr(&out));
}

#[test]
fn max_heap_of_zero_is_rejected() {
    let out = esdev()
        .arg("--max-heap=0")
        .arg("-e=1")
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("no heap at all"), "{}", stderr(&out));
}

/// `types` was never `esdev`'s — it belongs with the runtime that documents
/// them — and `upgrade` now is: install.sh installs both binaries, so a
/// developer whose esdev is stale had no way to move it but the installer
/// (D77).
#[test]
fn types_is_refused_and_upgrade_is_a_subcommand() {
    let types = esdev().arg("types").output().expect("spawn esdev");
    // Treated as a path (it is a bare word), so it fails as a missing file
    // rather than doing something surprising.
    assert!(!types.status.success(), "types should not succeed");
    assert!(
        stderr(&types).contains("cannot read") || stderr(&types).contains("cannot resolve"),
        "types: {}",
        stderr(&types)
    );

    // `upgrade` reaches the network, so what is pinned here is that it is a
    // subcommand rather than a path: a stray argument is refused by name, which
    // a file path would not be.
    let extra = esdev()
        .args(["upgrade", "0.4.0"])
        .output()
        .expect("spawn esdev upgrade");
    assert!(!extra.status.success());
    assert!(
        stderr(&extra).contains("does not take 0.4.0"),
        "{}",
        stderr(&extra)
    );
}

/// `upgrade` takes no arguments, but like every other subcommand it answers
/// `--help` — the top-level help promises each command takes it, and this one
/// refused it.
#[test]
fn upgrade_answers_help() {
    for flag in ["--help", "-h"] {
        let out = esdev()
            .args(["upgrade", flag])
            .output()
            .expect("spawn esdev upgrade --help");
        assert!(out.status.success(), "{flag}: {}", stderr(&out));
        let text = stdout(&out);
        assert!(text.contains("esdev upgrade"), "{text}");
        assert!(text.contains("--dry-run"), "{text}");
        assert!(text.contains("--only=esrun"), "{text}");
    }
}

/// `--only=` takes the names `install.sh`'s does, and nothing else — checked
/// before anything reaches the network.
#[test]
fn upgrade_only_takes_esdev_or_esrun() {
    for args in [
        &["upgrade", "--only=both"][..],
        &["upgrade", "--dry-run", "--only=node"][..],
    ] {
        let out = esdev().args(args).output().expect("spawn esdev upgrade");
        assert!(!out.status.success(), "{args:?} should not succeed");
        assert!(
            stderr(&out).contains("expected esrun or esdev"),
            "{args:?}: {}",
            stderr(&out)
        );
    }
}

/// `--only=esrun` with no esrun beside esdev is refused, naming the directory
/// it looked in, before anything reaches the network. The build directory may
/// hold an `esrun` beside the built `esdev`, so the check runs a copy of esdev
/// from a directory of its own.
#[cfg(unix)]
#[test]
fn upgrade_only_esrun_needs_an_esrun_beside_esdev() {
    let dir = std::env::temp_dir().join(format!("esdev-upgrade-only-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let copy = dir.join("esdev");
    std::fs::copy(env!("CARGO_BIN_EXE_esdev"), &copy).unwrap();
    let out = Command::new(&copy)
        .args(["upgrade", "--dry-run", "--only=esrun"])
        .output()
        .expect("spawn the esdev copy");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("no esrun is installed beside esdev"),
        "{}",
        stderr(&out)
    );
}

/// `--dry-run` is the read half of `upgrade`: it reaches the network, so what
/// is pinned here is the grammar — a value is refused by name without going
/// anywhere, which a live check then honors. (The live path is exercised by
/// hand: `esdev upgrade --dry-run` against the real release listing.)
#[test]
fn upgrade_dry_run_takes_no_value() {
    let out = esdev()
        .args(["upgrade", "--dry-run=yes"])
        .output()
        .expect("spawn esdev upgrade --dry-run=yes");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("does not take --dry-run=yes"),
        "{}",
        stderr(&out)
    );
}

/// A flag that is taken and dropped is one somebody keeps passing and keeps
/// believing. `esdev start` runs the server build's output as a child, under
/// esdev's development permissions — so the run-shaping flags it
/// used to accept and ignore are refused by name. `--shutdown-grace` is the one
/// that does apply: it bounds the drain on a restart.
///
/// Run in a directory with no esdev.json, so nothing here can start a server:
/// the refused flags fail while parsing, and the kept one gets past parsing and
/// fails on the missing file — which is exactly the difference being tested.
#[test]
fn start_refuses_the_run_flags_it_cannot_apply() {
    let dir = build_dir("s_run_flags");

    // `--otel` among them since D244 made the shared-flag check ask the parser
    // itself: before, `esdev start` called it an unknown option.
    for flag in [
        "--timeout=500",
        "--max-heap=64",
        "--env-override",
        "--otel=http://127.0.0.1:4318",
    ] {
        let out = esdev_in(&dir)
            .args(["start", flag])
            .output()
            .expect("spawn esdev start");
        assert!(!out.status.success(), "{flag} was accepted");
        assert!(
            stderr(&out)
                .contains("runs its server child with esdev's normal development permissions"),
            "{flag}: {}",
            stderr(&out)
        );
    }

    let grace = esdev_in(&dir)
        .args(["start", "--shutdown-grace=100"])
        .output()
        .expect("spawn esdev start");
    assert!(!grace.status.success());
    assert!(stderr(&grace).contains("esdev.json"), "{}", stderr(&grace));
    assert!(
        !stderr(&grace)
            .contains("runs its server child with esdev's normal development permissions"),
        "{}",
        stderr(&grace)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// An uncaught error is one block, and it names the file — the Phase 13 error
/// model, reached through the shared printer rather than a second copy of it.
#[test]
fn an_uncaught_error_is_reported_as_one_block() {
    let app = write("throws.mjs", "throw new Error('boom');\n");
    let out = esdev().arg(&app).output().expect("spawn esdev");
    assert!(!out.status.success());
    let text = stderr(&out);
    assert!(text.contains("uncaught exception in"), "{text}");
    assert!(text.contains("boom"), "{text}");
}

/// esdev gives the programs it runs a `RUNTIME_ENV` (D192): `development` for
/// a run, `test` for `esdev test`, and never over one the shell set.
#[test]
fn esdev_sets_runtime_env_for_what_it_runs_unless_the_shell_did() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("runtime_env_default");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("env.js"),
        "import { environment } from \"runtime:process\";\nconsole.log(environment());\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("env.test.js"),
        "import { environment } from \"runtime:process\";\nimport { test, assertEquals } from \"runtime:test\";\n\
         test(\"environment\", () => assertEquals(environment(), \"test\"));\n",
    )
    .unwrap();
    let run = |runtime_env: Option<&str>| {
        let mut command = esdev_in(&dir);
        command.env_remove("RUNTIME_ENV").env_remove("NODE_ENV");
        if let Some(value) = runtime_env {
            command.env("RUNTIME_ENV", value);
        }
        let out = command.arg("env.js").output().unwrap();
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    assert_eq!(run(None), "development");
    assert_eq!(run(Some("staging")), "staging");

    let out = esdev_in(&dir)
        .env_remove("RUNTIME_ENV")
        .args(["test", "--file=env.test.js"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `(ca, cert, key)` PEM: a private CA and a `localhost` server it signed.
fn private_ca() -> (String, String, String) {
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};
    let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.distinguished_name.push(DnType::CommonName, "Private CA");
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca_key = KeyPair::generate().unwrap();
    let ca_pem = ca.self_signed(&ca_key).unwrap().pem();
    let key = KeyPair::generate().unwrap();
    let cert = CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .signed_by(&key, &Issuer::from_params(&ca, &ca_key))
        .unwrap();
    (ca_pem, cert.pem(), key.serialize_pem())
}

// esdev takes `--tls-ca` like esrun (D197): the program's `fetch` trusts the
// CA it names, and without it the same server is refused.
#[test]
fn esdev_trusts_exactly_what_tls_ca_names() {
    let dir = build_dir("t_tls_ca");
    let (ca, cert, key) = private_ca();
    let ca_path = write_in(&dir, "ca.pem", &ca);
    let app = write_in(
        &dir,
        "app.mjs",
        r#"import { serve } from "runtime:http";
import { env } from "runtime:process";
const server = serve(
  { hostname: "127.0.0.1", port: 0, secureTransport: "on", cert: env.CERT, key: env.KEY },
  () => new Response("ok"),
);
const { port } = await server.addr;
try { console.log("FETCH:" + await (await fetch(`https://localhost:${port}/`)).text()); }
catch { console.log("FETCH:refused"); }
await server.stop();
"#,
    );
    let run = |flags: &[String]| {
        esdev()
            .current_dir(&dir)
            .args(flags)
            .arg(&app)
            .env("CERT", &cert)
            .env("KEY", &key)
            .env_remove("SSL_CERT_FILE")
            .env_remove("NODE_EXTRA_CA_CERTS")
            .output()
            .expect("spawn esdev")
    };
    let refused = run(&[]);
    assert!(refused.status.success(), "stderr: {}", stderr(&refused));
    assert!(
        stdout(&refused).contains("FETCH:refused"),
        "{}",
        stdout(&refused)
    );
    let trusted = run(&[format!("--tls-ca={}", ca_path.display())]);
    assert!(trusted.status.success(), "stderr: {}", stderr(&trusted));
    assert!(
        stdout(&trusted).contains("FETCH:ok"),
        "{}",
        stdout(&trusted)
    );
    let missing = run(&["--tls-ca=no-such-ca.pem".to_string()]);
    assert!(!missing.status.success(), "a missing CA file was accepted");
    assert!(
        stderr(&missing).contains("--tls-ca=no-such-ca.pem: cannot read it"),
        "{}",
        stderr(&missing)
    );
}
