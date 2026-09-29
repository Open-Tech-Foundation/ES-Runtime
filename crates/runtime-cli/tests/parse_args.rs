//! `parseArgs` from `runtime:process` (DECISIONS D142), through the real
//! binary: it reads the program's own arguments by default, needs no grant, and
//! refuses a bad command line with Node's codes.

use std::process::{Command, Output};

fn esrun(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_esrun"))
        .args(args)
        .output()
        .expect("spawn esrun")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

const PARSE: &str = "-e=import { parseArgs } from 'runtime:process'; \
    try { \
      const { values, positionals } = parseArgs({ \
        options: { \
          port: { type: 'string', short: 'p', default: '8080' }, \
          verbose: { type: 'boolean', short: 'v', multiple: true }, \
          color: { type: 'boolean' }, \
        }, \
        allowPositionals: true, \
        allowNegative: true, \
      }); \
      console.log(JSON.stringify({ values: { ...values }, positionals })); \
    } catch (e) { console.log(e.name, e.code); }";

/// With no `args` given it parses the program's own, which already exclude the
/// binary and the script — and nothing is granted.
#[test]
fn it_parses_the_programs_own_arguments_with_no_grant() {
    let out = esrun(&[
        PARSE,
        "serve",
        "-vv",
        "--port=3000",
        "--no-color",
        "--",
        "-x",
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        stdout(&out).trim(),
        r#"{"values":{"verbose":[true,true],"port":"3000","color":false},"positionals":["serve","-x"]}"#
    );
}

/// A default fills in what was not given.
#[test]
fn a_default_fills_an_absent_option() {
    let out = esrun(&[PARSE]);
    assert_eq!(
        stdout(&out).trim(),
        r#"{"values":{"port":"8080"},"positionals":[]}"#
    );
}

/// A bad command line is a `TypeError` with Node's code, so a program that
/// branches on `e.code` behaves as it does under Node.
#[test]
fn a_bad_command_line_throws_with_nodes_codes() {
    for (argv, code) in [
        (&["--nope"][..], "ERR_PARSE_ARGS_UNKNOWN_OPTION"),
        (&["--port"][..], "ERR_PARSE_ARGS_INVALID_OPTION_VALUE"),
        (&["--color=yes"][..], "ERR_PARSE_ARGS_INVALID_OPTION_VALUE"),
    ] {
        let mut all = vec![PARSE];
        all.extend_from_slice(argv);
        let out = esrun(&all);
        assert_eq!(stdout(&out).trim(), format!("TypeError {code}"), "{argv:?}");
    }
}

/// Every tokenizer shape and config check, against output recorded from Node's
/// own `util.parseArgs` on the same script — the parity D142 promises.
#[test]
fn it_parses_as_node_does() {
    let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    let out = Command::new(env!("CARGO_BIN_EXE_esrun"))
        .current_dir(fixtures)
        .arg("parse-args-cases.mjs")
        .output()
        .expect("spawn esrun");
    let expected = std::fs::read_to_string(format!("{fixtures}/parse-args-cases.expected"))
        .expect("read expected output");
    let actual = stdout(&out);
    for (index, (got, want)) in actual.lines().zip(expected.lines()).enumerate() {
        assert_eq!(got, want, "case {index}");
    }
    assert_eq!(actual.lines().count(), expected.lines().count(), "{actual}");
}
