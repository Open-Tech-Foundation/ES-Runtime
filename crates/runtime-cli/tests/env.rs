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

fn esrun() -> Command {
    Command::new(env!("CARGO_BIN_EXE_esrun"))
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
        .arg("--env-file")
        .arg(&envf)
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
        .arg("--env-file")
        .arg(&envf)
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
        .arg("--env-file")
        .arg(&envf)
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
        .arg("--env-file")
        .arg(temp("does-not-exist.env"))
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
        .arg("--env-file")
        .arg(&envf)
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
        .arg("--env-file")
        .arg(&envf)
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
