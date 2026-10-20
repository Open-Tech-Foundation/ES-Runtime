use super::parser::{EventKind, Parser};
use super::scanner::ScalarStyle;
use super::*;

// ---- yaml-test-suite ---------------------------------------------------------

struct Case {
    id: String,
    name: String,
    yaml: String,
    events: Option<String>,
    json: Option<String>,
    error: bool,
}

fn cases() -> Vec<Case> {
    let all: serde_json::Value =
        serde_json::from_str(include_str!("../../yaml-test-suite/cases.json")).unwrap();
    let text = |c: &serde_json::Value, k: &str| c[k].as_str().map(str::to_string);
    all.as_array()
        .unwrap()
        .iter()
        .map(|c| Case {
            id: text(c, "id").unwrap(),
            name: text(c, "name").unwrap(),
            yaml: text(c, "yaml").unwrap(),
            events: text(c, "events"),
            json: text(c, "json"),
            error: c["error"].as_bool().unwrap(),
        })
        .collect()
}

/// The event stream in the suite's `test.event` notation.
fn events(input: &str) -> Result<String, Error> {
    let mut parser = Parser::new(input)?;
    let mut out = String::new();
    let props = |out: &mut String, anchor: &Option<String>, tag: &Option<String>| {
        if let Some(a) = anchor {
            out.push_str(&format!(" &{a}"));
        }
        if let Some(t) = tag {
            out.push_str(&format!(" <{t}>"));
        }
    };
    while let Some(event) = parser.next_event()? {
        match &event.kind {
            EventKind::StreamStart => out.push_str("+STR"),
            EventKind::StreamEnd => out.push_str("-STR"),
            EventKind::DocumentStart { explicit } => {
                out.push_str(if *explicit { "+DOC ---" } else { "+DOC" })
            }
            EventKind::DocumentEnd { explicit } => {
                out.push_str(if *explicit { "-DOC ..." } else { "-DOC" })
            }
            EventKind::Alias(name) => out.push_str(&format!("=ALI *{name}")),
            EventKind::SequenceStart { anchor, tag, flow } => {
                out.push_str("+SEQ");
                if *flow {
                    out.push_str(" []");
                }
                props(&mut out, anchor, tag);
            }
            EventKind::SequenceEnd => out.push_str("-SEQ"),
            EventKind::MappingStart { anchor, tag, flow } => {
                out.push_str("+MAP");
                if *flow {
                    out.push_str(" {}");
                }
                props(&mut out, anchor, tag);
            }
            EventKind::MappingEnd => out.push_str("-MAP"),
            EventKind::Scalar {
                anchor,
                tag,
                value,
                style,
            } => {
                out.push_str("=VAL");
                props(&mut out, anchor, tag);
                out.push(' ');
                out.push(match style {
                    ScalarStyle::Plain => ':',
                    ScalarStyle::SingleQuoted => '\'',
                    ScalarStyle::DoubleQuoted => '"',
                    ScalarStyle::Literal => '|',
                    ScalarStyle::Folded => '>',
                });
                for c in value.chars() {
                    match c {
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\t' => out.push_str("\\t"),
                        '\u{8}' => out.push_str("\\b"),
                        '\r' => out.push_str("\\r"),
                        c => out.push(c),
                    }
                }
            }
        }
        out.push('\n');
    }
    Ok(out)
}

fn to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Number(n) => serde_json::Number::from_f64(*n)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::String(s) => serde_json::Value::String(s.clone()),
        Value::Array(a) => serde_json::Value::Array(a.iter().map(to_json).collect()),
        Value::Object(o) => {
            serde_json::Value::Object(o.iter().map(|(k, v)| (k.clone(), to_json(v))).collect())
        }
        other => panic!("unexpected value {other:?}"),
    }
}

/// JSON documents compare by value, with every number as a double, which is
/// all a JavaScript value can carry.
fn normalize(v: serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Number(n) => serde_json::Number::from_f64(n.as_f64().unwrap())
            .map(serde_json::Value::Number)
            .unwrap(),
        serde_json::Value::Array(a) => {
            serde_json::Value::Array(a.into_iter().map(normalize).collect())
        }
        serde_json::Value::Object(o) => {
            serde_json::Value::Object(o.into_iter().map(|(k, v)| (k, normalize(v))).collect())
        }
        other => other,
    }
}

/// Every case in the pinned yaml-test-suite: an error case must be rejected,
/// a valid one must produce the suite's events exactly, and where the suite
/// gives the JSON value of a single-document stream, `parse` must produce it.
#[test]
fn yaml_test_suite() {
    let mut failures = Vec::new();
    let cases = cases();
    for case in &cases {
        let label = format!("{} ({})", case.id, case.name);
        let got = events(&case.yaml);
        if case.error {
            if got.is_ok() && parse(&case.yaml).is_ok() {
                failures.push(format!("{label}: accepted invalid YAML"));
            }
            continue;
        }
        match (&got, &case.events) {
            (Err(e), _) => {
                failures.push(format!("{label}: rejected valid YAML: {e}"));
                continue;
            }
            (Ok(got), Some(want)) if got != want => {
                failures.push(format!(
                    "{label}: events differ\n--- want\n{want}--- got\n{got}"
                ));
                continue;
            }
            _ => {}
        }
        let Some(json) = &case.json else { continue };
        let docs: Vec<serde_json::Value> = serde_json::Deserializer::from_str(json)
            .into_iter()
            .map(|d| d.unwrap())
            .collect();
        if docs.len() != 1 {
            // A stream of several documents; `parse` reads one by design.
            assert!(parse(&case.yaml).is_err() || docs.is_empty(), "{label}");
            continue;
        }
        match parse(&case.yaml) {
            Ok(v) => {
                let want = normalize(docs.into_iter().next().unwrap());
                let got = normalize(to_json(&v));
                if got != want {
                    failures.push(format!(
                        "{label}: value differs\n--- want\n{want}\n--- got\n{got}"
                    ));
                }
            }
            Err(e) if is_policy_refusal(&e) => {}
            Err(e) => failures.push(format!("{label}: parse failed: {e}")),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} yaml-test-suite cases fail:\n\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
}

/// Valid YAML that `parse` refuses on purpose: a tag it does not implement.
fn is_policy_refusal(e: &Error) -> bool {
    e.message.starts_with("unsupported tag")
}

// ---- parse -------------------------------------------------------------------

fn obj(pairs: &[(&str, Value)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

fn s(v: &str) -> Value {
    Value::String(v.into())
}

fn n(v: f64) -> Value {
    Value::Number(v)
}

fn err(input: &str) -> Error {
    parse(input).expect_err(input)
}

#[test]
fn core_schema() {
    let v = parse(
        "a: ~\nb: Null\nc: TRUE\nd: False\ne: 0x1F\nf: 0o17\ng: 017\nh: +5\ni: 1e3\nj: .5\nk: 6.\nl: -.INF\nm: 1_000\nn: tRue\no: yes\np: 2001-12-14\nq:",
    )
    .unwrap();
    assert_eq!(
        v,
        obj(&[
            ("a", Value::Null),
            ("b", Value::Null),
            ("c", Value::Bool(true)),
            ("d", Value::Bool(false)),
            ("e", n(31.0)),
            ("f", n(15.0)),
            ("g", n(17.0)),
            ("h", n(5.0)),
            ("i", n(1000.0)),
            ("j", n(0.5)),
            ("k", n(6.0)),
            ("l", n(f64::NEG_INFINITY)),
            ("m", s("1_000")),
            ("n", s("tRue")),
            ("o", s("yes")),
            ("p", s("2001-12-14")),
            ("q", Value::Null),
        ])
    );
    let Value::Object(nan) = parse("x: .NaN").unwrap() else {
        panic!()
    };
    assert!(matches!(nan[0].1, Value::Number(x) if x.is_nan()));
}

#[test]
fn numbers_past_safe_integers() {
    assert_eq!(
        parse("18446744073709551616").unwrap(),
        n(18446744073709551616.0)
    );
    assert_eq!(parse("-1e400").unwrap(), n(f64::NEG_INFINITY));
    // Integer zero has no sign; a float keeps it.
    let Value::Number(z) = parse("-0").unwrap() else {
        panic!()
    };
    assert!(z == 0.0 && z.is_sign_positive());
    let Value::Number(z) = parse("-0.0").unwrap() else {
        panic!()
    };
    assert!(z == 0.0 && z.is_sign_negative());
}

#[test]
fn tags() {
    assert_eq!(parse("!!str 123").unwrap(), s("123"));
    assert_eq!(parse("!!int '7'").unwrap(), n(7.0));
    assert_eq!(parse("!!float 1").unwrap(), n(1.0));
    assert_eq!(parse("!!null ''").unwrap(), Value::Null);
    assert_eq!(parse("! 12").unwrap(), s("12"));
    assert_eq!(parse("!!map {a: 1}").unwrap(), obj(&[("a", n(1.0))]));
    assert!(err("!!bool yes").message.contains("not a valid !!bool"));
    assert!(err("!!int 1.5").message.contains("not a valid !!int"));
    assert_eq!(err("a: !custom x").message, "unsupported tag !custom");
    assert_eq!(err("!!seq {a: 1}").message, "unsupported tag !!seq");
}

#[test]
fn keys_keep_their_text() {
    assert_eq!(
        parse("1: a\n0x10: b\ntrue: c\n~: d\n'q': e").unwrap(),
        obj(&[
            ("1", s("a")),
            ("0x10", s("b")),
            ("true", s("c")),
            ("~", s("d")),
            ("q", s("e"))
        ])
    );
    assert_eq!(parse("? a\n: b").unwrap(), obj(&[("a", s("b"))]));
    assert_eq!(
        parse("&k a: 1\n*k : 2").map_err(|e| e.message),
        Err("duplicate mapping key \"a\"".into())
    );
}

#[test]
fn collection_keys_are_refused() {
    assert!(
        err("[1, 2]: x")
            .message
            .contains("sequence cannot be a mapping key")
    );
    assert!(
        err("{a: 1}: x")
            .message
            .contains("mapping cannot be a mapping key")
    );
}

#[test]
fn duplicate_keys_are_refused() {
    let e = err("a: 1\nb: 2\na: 3");
    assert_eq!(e.message, "duplicate mapping key \"a\"");
    assert_eq!((e.mark.line, e.mark.column), (2, 0));
    assert!(err("{a: 1, a: 2}").message.contains("duplicate"));
}

#[test]
fn merge_keys() {
    let v = parse("base: &b {x: 1, y: 2}\nd:\n  <<: *b\n  y: 3\n  z: 4").unwrap();
    assert_eq!(
        v,
        obj(&[
            ("base", obj(&[("x", n(1.0)), ("y", n(2.0))])),
            ("d", obj(&[("x", n(1.0)), ("y", n(3.0)), ("z", n(4.0))])),
        ])
    );
    // An explicit key wins even when written before the merge.
    assert_eq!(
        parse("a: &a {k: 1}\nb:\n  k: 2\n  <<: *a").unwrap(),
        obj(&[("a", obj(&[("k", n(1.0))])), ("b", obj(&[("k", n(2.0))]))])
    );
    // A list merges in order, earlier sources first.
    assert_eq!(
        parse("- &a {k: 1}\n- &b {k: 2, j: 2}\n- <<: [*a, *b]").unwrap(),
        Value::Array(vec![
            obj(&[("k", n(1.0))]),
            obj(&[("k", n(2.0)), ("j", n(2.0))]),
            obj(&[("k", n(1.0)), ("j", n(2.0))]),
        ])
    );
    // Quoted, it is an ordinary key.
    assert_eq!(parse("'<<': 1").unwrap(), obj(&[("<<", n(1.0))]));
    assert!(err("<<: 1").message.contains("merge key"));
    assert!(err("<<: [1]").message.contains("merge key"));
}

#[test]
fn anchors_and_aliases() {
    assert_eq!(
        parse("a: &x 1\nb: *x").unwrap(),
        obj(&[("a", n(1.0)), ("b", n(1.0))])
    );
    assert_eq!(err("*nope").message, "unknown anchor *nope");
    assert_eq!(err("&a [*a]").message, "alias *a refers to itself");
}

#[test]
fn alias_expansion_is_bounded() {
    let mut doc = String::from("a0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
    for i in 1..9 {
        let refs = vec![format!("*a{}", i - 1); 10].join(", ");
        doc.push_str(&format!("a{i}: &a{i} [{refs}]\n"));
    }
    let e = err(&doc);
    assert_eq!(e.kind, ErrorKind::Limit);
    assert!(e.message.contains("aliases expand"), "{e}");
}

#[test]
fn nesting_is_bounded() {
    let ok = format!("{}{}", "[".repeat(MAX - 1), "]".repeat(MAX - 1));
    assert!(parse(&ok).is_ok());
    let deep = format!("{}{}", "[".repeat(MAX + 1), "]".repeat(MAX + 1));
    assert_eq!(err(&deep).kind, ErrorKind::Limit);
    // Unclosed and absurdly deep: still an error, not a stack overflow.
    assert!(parse(&"[".repeat(1_000_000)).is_err());
    assert!(parse(&"- ".repeat(100_000)).is_err());
}
const MAX: usize = compose::MAX_DEPTH;

#[test]
fn one_document_only() {
    assert_eq!(parse("").unwrap(), Value::Null);
    assert_eq!(parse("# only a comment\n").unwrap(), Value::Null);
    assert_eq!(parse("--- 5").unwrap(), n(5.0));
    assert_eq!(
        parse("%YAML 1.2\n---\na: 1\n...\n").unwrap(),
        obj(&[("a", n(1.0))])
    );
    assert!(
        err("---\na: 1\n---\nb: 2")
            .message
            .contains("more than one document")
    );
}

#[test]
fn errors_carry_positions() {
    let e = err("name: Alice\n  age: 30\n");
    assert_eq!(e.kind, ErrorKind::Syntax);
    assert_eq!(e.mark.line, 1);
    assert!(e.to_string().ends_with("at line 2 column 6"), "{e}");
    assert!(err("a: b: c").message.contains("not allowed"));
    assert!(err("a: @x").message.contains("cannot start any token"));
    assert!(err("\u{1}").message.contains("U+0001"));
}

#[test]
fn block_scalars_chomp() {
    let a = |src: &str| match parse(src).unwrap() {
        Value::Object(mut o) => o.remove(0).1,
        _ => panic!(),
    };
    assert_eq!(a("a: |\n  l1\n  l2\n"), s("l1\nl2\n"));
    assert_eq!(a("a: >\n  l1\n  l2\n"), s("l1 l2\n"));
    assert_eq!(a("a: |\n  l1\n  l2"), s("l1\nl2"));
    assert_eq!(a("a: |\n  l1\n  l2\n\n\n"), s("l1\nl2\n"));
    assert_eq!(a("a: |-\n  l1\n  l2\n"), s("l1\nl2"));
    assert_eq!(a("a: |+\n  l1\n  l2\n\n"), s("l1\nl2\n\n"));
    assert_eq!(a("a: >-\n  x\n\n  y\n   z\n  w"), s("x\ny\n z\nw"));
}

#[test]
fn line_breaks_normalize() {
    assert_eq!(
        parse("a: 1\r\nb: |\r\n  x\r\n  y\r\n").unwrap(),
        obj(&[("a", n(1.0)), ("b", s("x\ny\n"))])
    );
}

// ---- emit --------------------------------------------------------------------

fn round_trip(v: &Value) -> String {
    let text = emit(v);
    let back = parse(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
    assert_eq!(&back, v, "round trip through:\n{text}");
    text
}

#[test]
fn emit_layout() {
    let v = obj(&[
        ("user", obj(&[("name", s("Bob")), ("id", n(42.0))])),
        (
            "a",
            Value::Array(vec![
                n(1.0),
                Value::Array(vec![n(2.0), n(3.0)]),
                obj(&[("b", n(4.0)), ("c", n(5.0))]),
            ]),
        ),
        ("e", Value::Array(vec![])),
        ("f", Value::Object(vec![])),
        ("g", Value::Null),
        ("h", Value::Bool(true)),
        ("i", n(2.5)),
    ]);
    assert_eq!(
        round_trip(&v),
        "user:\n  name: Bob\n  id: 42\na:\n- 1\n- - 2\n  - 3\n- b: 4\n  c: 5\ne: []\nf: {}\ng: null\nh: true\ni: 2.5\n"
    );
    assert_eq!(emit(&Value::Null), "null\n");
    assert_eq!(emit(&n(5.0)), "5\n");
    assert_eq!(emit(&s("s")), "s\n");
    assert_eq!(emit(&Value::Array(vec![])), "[]\n");
}

#[test]
fn emit_quotes_what_would_not_read_back() {
    for (input, want) in [
        ("1", "'1'"),
        ("true", "'true'"),
        ("null", "'null'"),
        ("~", "'~'"),
        ("", "''"),
        ("0x10", "'0x10'"),
        (".inf", "'.inf'"),
        ("yes", "'yes'"),
        ("Off", "'Off'"),
        ("x: y", "'x: y'"),
        ("- x", "'- x'"),
        ("#x", "'#x'"),
        ("a #b", "'a #b'"),
        (" lead", "' lead'"),
        ("trail ", "'trail '"),
        ("it's", "it's"),
        ("'q", "'''q'"),
        ("---", "'---'"),
        ("...", "'...'"),
        ("a#b", "a#b"),
        ("é", "é"),
        ("2001-01-01", "2001-01-01"),
        ("\t", "\"\\t\""),
        ("\u{7}", "\"\\a\""),
        ("\r", "\"\\r\""),
        ("\u{feff}x", "\"\\uFEFFx\""),
        ("line \nx", "\"line \\nx\""),
    ] {
        let v = obj(&[("a", s(input))]);
        assert_eq!(round_trip(&v), format!("a: {want}\n"), "{input:?}");
    }
}

#[test]
fn emit_multiline_as_literal() {
    for (input, want) in [
        ("l1\nl2", "a: |-\n  l1\n  l2\n"),
        ("l1\nl2\n", "a: |\n  l1\n  l2\n"),
        ("l1\nl2\n\n", "a: |+\n  l1\n  l2\n\n"),
        ("l1\n\nl2", "a: |-\n  l1\n\n  l2\n"),
        ("\nl1", "a: |-\n\n  l1\n"),
        ("\n", "a: \"\\n\"\n"),
        (" l1\nl2", "a: \" l1\\nl2\"\n"),
    ] {
        assert_eq!(round_trip(&obj(&[("a", s(input))])), want, "{input:?}");
    }
    assert_eq!(
        round_trip(&Value::Array(vec![s("x\ny")])),
        "- |-\n  x\n  y\n"
    );
    let nested = obj(&[("m", obj(&[("t", s("x\ny\n"))])), ("z", n(1.0))]);
    assert_eq!(round_trip(&nested), "m:\n  t: |\n    x\n    y\nz: 1\n");
}

#[test]
fn emit_keys() {
    let v = obj(&[
        ("1", n(2.0)),
        ("k k", n(1.0)),
        ("", n(3.0)),
        ("<<", n(4.0)),
        ("a\nb", n(5.0)),
    ]);
    assert_eq!(
        round_trip(&v),
        "'1': 2\nk k: 1\n'': 3\n'<<': 4\n\"a\\nb\": 5\n"
    );
    let long = "k".repeat(2000);
    let text = round_trip(&obj(&[(&long, obj(&[("x", n(1.0))]))]));
    assert!(text.starts_with("? kkk"), "{text}");
}

#[test]
fn emit_numbers() {
    for (v, want) in [
        (1.0, "1"),
        (-0.0, "0"),
        (0.1, "0.1"),
        (1e21, "1e+21"),
        (1.5e-7, "1.5e-7"),
        (9007199254740993.0, "9007199254740992"),
        // Past i64::MAX an `i64` cast saturated to 9223372036854775807.
        (2f64.powi(63), "9223372036854776000"),
        (1e300, "1e+300"),
        (f64::INFINITY, ".inf"),
        (f64::NEG_INFINITY, "-.inf"),
    ] {
        assert_eq!(emit(&n(v)), format!("{want}\n"));
        assert_eq!(
            parse(&emit(&n(v))).unwrap(),
            n(if v == 0.0 { 0.0 } else { v })
        );
    }
    assert_eq!(emit(&n(f64::NAN)), ".nan\n");
}

#[test]
fn build_writes_a_bigint_and_refuses_bytes() {
    assert_eq!(
        build(&obj(&[("n", Value::BigInt(12_345_678_901_234_567_890))])).unwrap(),
        "'n': 12345678901234567890\n"
    );
    assert!(build(&obj(&[("b", Value::Bytes(vec![1]))])).is_err());
    assert!(build(&Value::Array(vec![Value::Bytes(vec![])])).is_err());
    assert_eq!(
        build(&obj(&[("u", Value::Undefined)])).unwrap(),
        "u: null\n"
    );
}

#[test]
fn fuzz_entry_point_survives_odd_input() {
    for input in [
        "",
        "a: [",
        "- &a\n- *a",
        "? |\n  x\n: >\n y",
        "{? a: b}",
        "\"\\u",
        "!<",
        "%TAG ! !x\n--- !a b",
    ] {
        fuzz_round_trip(input);
    }
}
