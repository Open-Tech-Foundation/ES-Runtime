//! End-to-end tests for `--env-file` loading, OS/file precedence, and the
//! secret-masking convention (DECISIONS D30). These spawn the real `esrun`
//! binary so the actual dotenv parser, `SystemProcess` overlay, and the
//! `runtime:process` `Secret` wrapper are exercised together. The OS
//! environment is set per-process via `Command::env`, so no `unsafe` set_var is
//! needed (and the test process's own env is untouched).

use std::path::PathBuf;
use std::process::{Command, Output};

/// A unique path under Cargo's per-test temp dir (`CARGO_TARGET_TMPDIR`).
fn temp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn write(name: &str, contents: &str) -> PathBuf {
    let path = temp(name);
    std::fs::write(&path, contents).expect("write temp file");
    path
}

/// esrun, already granted `env`.
///
/// The grant is fixture, not subject: esrun grants nothing on its own (D65) and
/// every test in this file is about what `env` *contains*, so making each one
/// spell the flag would say the same thing thirty times. A test about the gate
/// itself lives in `permissions.rs`.
fn esrun() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_esrun"));
    command.arg("--allow-env");
    command
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A program that prints selected env vars; used across the precedence tests.
const PRINT_APP: &str = r#"
import { env } from "runtime:process";
for (const k of ["A", "B", "C"]) console.log(k + "=" + env[k]);
"#;

#[test]
fn loads_values_from_env_file() {
    let envf = write("load.env", "A=one\nB=two\n");
    let app = write("load.mjs", PRINT_APP);
    let out = esrun()
        .arg(format!("--env-file={}", envf.display()))
        .arg(&app)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("A=one"), "{s}");
    assert!(s.contains("B=two"), "{s}");
}

#[test]
fn os_env_wins_by_default() {
    let envf = write("prec_default.env", "A=from_file\n");
    let app = write("prec_default.mjs", PRINT_APP);
    let out = esrun()
        .arg(format!("--env-file={}", envf.display()))
        .arg(&app)
        .env("A", "from_os")
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("A=from_os"), "{}", stdout(&out));
}

#[test]
fn env_override_lets_file_win() {
    let envf = write("prec_override.env", "A=from_file\n");
    let app = write("prec_override.mjs", PRINT_APP);
    let out = esrun()
        .arg(format!("--env-file={}", envf.display()))
        .arg("--env-override")
        .arg(&app)
        .env("A", "from_os")
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("A=from_file"), "{}", stdout(&out));
}

#[test]
fn missing_env_file_is_an_error() {
    let app = write("missing.mjs", "console.log('ran')");
    let out = esrun()
        .arg(format!(
            "--env-file={}",
            temp("does-not-exist.env").display()
        ))
        .arg(&app)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("--env-file"), "{}", stderr(&out));
    assert!(!stdout(&out).contains("ran"), "should not have run");
}

#[test]
fn secret_keyed_values_are_redacted_everywhere_but_unmaskable() {
    // Keys matching *_SECRET(S) / *_PASSWORD(S) are masked; others are plain.
    let envf = write(
        "secret.env",
        "DB_PASSWORD=s3cr3t-pw\nAPI_SECRET=tok-123\nPLAIN_VALUE=visible\n",
    );
    let app = write(
        "secret.mjs",
        r#"
        import { env, unmask } from "runtime:process";
        console.log("log:" + "" , env.DB_PASSWORD);
        console.log("tmpl:" + `${env.API_SECRET}`);
        console.log("json:" + JSON.stringify({ a: env.DB_PASSWORD, b: env.API_SECRET }));
        console.log("whole:" + (JSON.stringify(env).includes("s3cr3t") ? "LEAK" : "clean"));
        console.log("plain:" + env.PLAIN_VALUE);
        console.log("unmask:" + unmask(env.DB_PASSWORD));
        console.log("unmask-plain:" + unmask(env.PLAIN_VALUE));
        "#,
    );
    let out = esrun()
        .arg(format!("--env-file={}", envf.display()))
        .arg(&app)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);

    // No raw secret leaks via console, template literal, or JSON.stringify.
    assert!(
        !s.contains("s3cr3t-pw") || s.contains("unmask:s3cr3t-pw"),
        "{s}"
    );
    assert!(s.contains("log: [redacted]"), "{s}");
    assert!(s.contains("tmpl:[redacted]"), "{s}");
    assert!(
        s.contains(r#"json:{"a":"[redacted]","b":"[redacted]"}"#),
        "{s}"
    );
    assert!(s.contains("whole:clean"), "{s}");
    // Plain (non-secret) values are untouched.
    assert!(s.contains("plain:visible"), "{s}");
    // unmask reveals the real value; plain strings pass through.
    assert!(s.contains("unmask:s3cr3t-pw"), "{s}");
    assert!(s.contains("unmask-plain:visible"), "{s}");
    // The token must not appear except where explicitly unmasked (it isn't here).
    assert!(!s.contains("tok-123"), "API_SECRET leaked: {s}");
}

#[test]
fn secret_key_convention_covers_the_full_pattern_set() {
    // Positives: *_SECRET(S), *_PASSWORD(S), *_PASS, *_KEY(S), *_TOKEN(S), and
    // CREDENTIAL / AUTH as underscore-delimited words. Negatives: lookalikes
    // (MONKEY ends in KEY, AUTHOR contains AUTH) and ordinary config keys.
    let envf = write(
        "patterns.env",
        "API_KEY=v\nACCESS_TOKEN=v\nDB_PASS=v\nDB_PASSWORD=v\nAPP_SECRET=v\n\
         AWS_CREDENTIALS=v\nAUTH_TOKEN=v\nAPI_AUTH=v\nPUBLIC_KEY=v\n\
         MONKEY=v\nAUTHOR=v\nDATABASE_URL=v\n",
    );
    let app = write(
        "patterns.mjs",
        r#"
        import { env } from "runtime:process";
        const keys = ["API_KEY","ACCESS_TOKEN","DB_PASS","DB_PASSWORD","APP_SECRET",
          "AWS_CREDENTIALS","AUTH_TOKEN","API_AUTH","PUBLIC_KEY",
          "MONKEY","AUTHOR","DATABASE_URL"];
        for (const k of keys)
          console.log(k + "=" + (String(env[k]) === "[redacted]" ? "masked" : "plain"));
        "#,
    );
    let out = esrun()
        .arg(format!("--env-file={}", envf.display()))
        .arg(&app)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    for masked in [
        "API_KEY=masked",
        "ACCESS_TOKEN=masked",
        "DB_PASS=masked",
        "DB_PASSWORD=masked",
        "APP_SECRET=masked",
        "AWS_CREDENTIALS=masked",
        "AUTH_TOKEN=masked",
        "API_AUTH=masked",
        "PUBLIC_KEY=masked",
    ] {
        assert!(s.contains(masked), "expected {masked}\n{s}");
    }
    for plain in ["MONKEY=plain", "AUTHOR=plain", "DATABASE_URL=plain"] {
        assert!(s.contains(plain), "expected {plain}\n{s}");
    }
}

/// Masking applies to what a program *writes*, not only to the host snapshot.
///
/// `env.MY_API_KEY = "…"` — how a program threads a value it just fetched down
/// to a child — stored the string raw, so the same key that arrives masked from
/// the environment stayed unmasked when the program set it, and leaked in a log
/// line or a `JSON.stringify` like any other value.
#[test]
fn a_secret_key_assigned_at_runtime_is_masked_too() {
    let app = write(
        "runtime-secret.mjs",
        r#"
import { env, unmask, Secret } from "runtime:process";
env.MY_API_KEY = "secret_123";
env.PLAIN_VALUE = "not a secret";
console.log("wrapped=" + (env.MY_API_KEY instanceof Secret));
console.log("string=" + String(env.MY_API_KEY));
console.log("json=" + JSON.stringify({ k: env.MY_API_KEY }));
console.log("template=" + `${env.MY_API_KEY}`);
// The real value is still reachable deliberately…
console.log("unmask=" + unmask(env.MY_API_KEY));
// …a key that is not secret-bearing is untouched…
console.log("plain=" + env.PLAIN_VALUE);
// …and an already-wrapped value is not wrapped twice.
env.OTHER_TOKEN = new Secret("wrapped once");
console.log("double=" + unmask(env.OTHER_TOKEN));
"#,
    );
    let out = esrun().arg(&app).output().expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    for expected in [
        "wrapped=true",
        "string=[redacted]",
        "json={\"k\":\"[redacted]\"}",
        "template=[redacted]",
        "unmask=secret_123",
        "plain=not a secret",
        "double=wrapped once",
    ] {
        assert!(s.contains(expected), "missing {expected:?} in:\n{s}");
    }
}

/// `env` is a string-to-string map — the only thing an OS environment can hold,
/// and the only thing a child process can receive. Assigning a non-string left
/// the raw value in place: `typeof env.PORT` came back "number", and passing the
/// object on as `new Command(cmd, { env })` then threw "must be a string" for a
/// value the program had every reason to believe it had set. Node and Deno both
/// coerce with string semantics, including rejecting a symbol.
#[test]
fn env_values_are_coerced_to_strings_on_assignment() {
    let app = write(
        "env-coercion.mjs",
        r#"
import { env, unmask, Secret } from "runtime:process";
const show = (k) => console.log(k + "=" + JSON.stringify(env[k]) + ":" + typeof env[k]);

env.NUM = 8080;      show("NUM");
env.BOOL = true;     show("BOOL");
env.NUL = null;      show("NUL");
env.UNDEF = undefined; show("UNDEF");
env.OBJ = { a: 1 };  show("OBJ");
env.STR = "plain";   show("STR");

// A write that bypasses the `set` trap must coerce too.
Object.defineProperty(env, "DEFINED", {
  value: 42, writable: true, enumerable: true, configurable: true,
});
show("DEFINED");

// A symbol has no string value to store; Node and Deno throw here.
try { env.SYM = Symbol("x"); console.log("sym=no throw"); }
catch (e) { console.log("sym=" + e.constructor.name); }

// Masking still applies, and wraps the *coerced* value rather than the raw one.
env.MY_API_KEY = 8080;
console.log("secret=" + String(env.MY_API_KEY));
console.log("secretWrapped=" + (env.MY_API_KEY instanceof Secret));
console.log("secretUnmasked=" + JSON.stringify(unmask(env.MY_API_KEY)));
"#,
    );
    let out = esrun().arg(&app).output().expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    for expected in [
        "NUM=\"8080\":string",
        "BOOL=\"true\":string",
        "NUL=\"null\":string",
        "UNDEF=\"undefined\":string",
        "OBJ=\"[object Object]\":string",
        "STR=\"plain\":string",
        "DEFINED=\"42\":string",
        "sym=TypeError",
        "secret=[redacted]",
        "secretWrapped=true",
        "secretUnmasked=\"8080\"",
    ] {
        assert!(s.contains(expected), "missing {expected:?} in:\n{s}");
    }
}

/// The point of the coercion: a value assigned to `env` can be handed straight
/// to a child, which used to be impossible for anything but a string.
#[test]
fn a_coerced_env_value_reaches_a_child_process() {
    let app = write(
        "env-coercion-child.mjs",
        r#"
import { env } from "runtime:process";
import { Command } from "runtime:system";
env.PORT = 8080;
const out = await new Command("sh", {
  args: ["-c", "echo PORT=[$PORT]"],
  env: { PORT: env.PORT },
}).output();
console.log(new TextDecoder().decode(out.stdout).trim());
"#,
    );
    let out = esrun()
        .arg("--allow-run=sh")
        .arg(&app)
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("PORT=[8080]"),
        "the child did not receive the coerced value:\n{}",
        stdout(&out)
    );
}
