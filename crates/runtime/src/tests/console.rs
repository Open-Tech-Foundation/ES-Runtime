//! `console`: formatting, grouping, counting and inspection.

use super::*;

#[test]
fn console_routes_to_the_injected_sink() {
    let _g = v8_guard();
    let console = Arc::new(TestConsole::default());
    let mut rt = runtime_with(
        console.clone(),
        Arc::new(FixedClock {
            monotonic: 0,
            wall: 0,
        }),
    );
    rt.eval(r#"console.log("hi", 42); console.error("boom");"#)
        .unwrap();
    let lines = console.lines.lock().unwrap().clone();
    assert_eq!(
        lines,
        vec![
            (ConsoleLevel::Log, "hi 42".to_string()),
            (ConsoleLevel::Error, "boom".to_string()),
        ]
    );
}

/// Captures everything the guest logged, as `level: message` lines.
fn console_lines(script: &str) -> Vec<String> {
    let console = Arc::new(TestConsole::default());
    let mut rt = runtime_with(
        console.clone(),
        Arc::new(FixedClock {
            monotonic: 0,
            wall: 0,
        }),
    );
    rt.eval(script).unwrap();
    let lines = console.lines.lock().unwrap().clone();
    lines
        .into_iter()
        .map(|(level, message)| format!("{level:?}: {message}").to_lowercase())
        .collect()
}

/// The Console Standard's Formatter: `%s`/`%d`/`%i`/`%f`/`%o`/`%O`/`%j`
/// consume the following arguments, `%%` is a literal, and anything left
/// over is appended.
#[test]
fn console_applies_format_specifiers() {
    let _g = v8_guard();
    let lines = console_lines(
        r#"console.log("%s is %d and %f", "Ada", 36.7, 1.5);
           console.log("%o|%j", { a: [1, 2] }, { b: 2 });
           console.log("100%% sure");
           console.log("%s", "one", "extra", 2);
           console.log("%d", "not a number");
           console.log("no specifiers", 1, { a: 1 });"#,
    );
    assert_eq!(
        lines,
        vec![
            "log: ada is 36 and 1.5",
            "log: { a: [ 1, 2 ] }|{\"b\":2}",
            // A lone string with no arguments is not a format string.
            "log: 100%% sure",
            "log: one extra 2",
            "log: nan",
            "log: no specifiers 1 { a: 1 }",
        ]
    );
}

/// A group indents everything until it closes, including the inner lines of
/// a multi-line value.
#[test]
fn console_group_indents_until_closed() {
    let _g = v8_guard();
    let lines = console_lines(
        r#"console.group("outer");
           console.log("a\nb");
           console.group();
           console.log("deep");
           console.groupEnd();
           console.groupEnd();
           console.log("flush left");"#,
    );
    assert_eq!(
        lines,
        vec![
            "log: outer",
            "log:   a\n  b",
            "log:     deep",
            "log: flush left",
        ]
    );
}

/// `count`/`countReset` and `time`/`timeLog`/`timeEnd` were no-ops that
/// silently discarded what they were asked to measure.
#[test]
fn console_counts_and_times() {
    let _g = v8_guard();
    let lines = console_lines(
        r#"console.count(); console.count(); console.count("x");
           console.countReset(); console.count();
           console.time("t"); console.timeLog("t", "mid"); console.timeEnd("t");
           console.timeEnd("t");
           console.time("t"); console.time("t");"#,
    );
    assert_eq!(lines[0], "log: default: 1");
    assert_eq!(lines[1], "log: default: 2");
    assert_eq!(lines[2], "log: x: 1");
    assert_eq!(lines[3], "log: default: 1");
    assert!(lines[4].starts_with("log: t: "), "{}", lines[4]);
    assert!(lines[4].ends_with("ms mid"), "{}", lines[4]);
    assert!(lines[5].starts_with("log: t: "), "{}", lines[5]);
    // Ending a timer that is not running, and starting one twice, both warn.
    assert_eq!(lines[6], "warn: timer 't' does not exist");
    assert_eq!(lines[7], "warn: timer 't' already exists");
}

/// `table` renders rows and columns; dumping the object would have made the
/// method pointless.
#[test]
fn console_table_renders_a_table() {
    let _g = v8_guard();
    let lines = console_lines(r#"console.table([{ a: 1, b: "x" }, { a: 22 }]);"#);
    assert_eq!(
        lines[0],
        "log: ┌─────────┬────┬─────┐\n\
         │ (index) │ a  │ b   │\n\
         ├─────────┼────┼─────┤\n\
         │ 0       │ 1  │ 'x' │\n\
         │ 1       │ 22 │     │\n\
         └─────────┴────┴─────┘"
    );

    // Primitive rows go in a Values column, and an object's keys index it.
    let lines = console_lines(r#"console.table({ r: 1 });"#);
    assert!(lines[0].contains("(key)"), "{}", lines[0]);
    assert!(lines[0].contains("values"), "{}", lines[0]);
}

#[test]
fn console_trace_includes_the_stack() {
    let _g = v8_guard();
    let lines = console_lines(r#"console.trace("why");"#);
    assert!(lines[0].starts_with("error: trace: why\n"), "{}", lines[0]);
    assert!(lines[0].contains("    at "), "{}", lines[0]);
}

// ----- console.log inspection -------------------------------------------

/// Captures the last console line emitted by `source`.
fn console_line(source: &str) -> String {
    let console = Arc::new(TestConsole::default());
    let mut rt = runtime_with(
        console.clone(),
        Arc::new(FixedClock {
            monotonic: 0,
            wall: 0,
        }),
    );
    rt.eval(source).expect("eval");
    let lines = console.lines.lock().unwrap_or_else(|e| e.into_inner());
    lines.last().expect("a console line").1.clone()
}

#[test]
fn console_inspects_objects_without_dropping_functions() {
    // The regression behind the moderndash report: an object/namespace of
    // functions must not render as `{}` (JSON.stringify drops functions).
    let line =
        console_line("console.log({ n: 1, fn: function foo() {}, arr: [1, 'two', { x: 3 }] })");
    assert!(line.contains("n: 1"), "{line}");
    assert!(line.contains("fn: [Function: foo]"), "{line}");
    assert!(line.contains("arr: [ 1, 'two', { x: 3 } ]"), "{line}");
}

#[test]
fn console_inspects_a_namespace_of_functions() {
    // Name inference applies to bindings/literals, not `obj.x = fn`, so the
    // arrow is anonymous and the named function keeps its name.
    let line = console_line(
        "const ns = Object.create(null); ns.a = () => {}; ns.b = function bee() {}; \
         console.log(ns);",
    );
    assert!(line.starts_with("[Object: null prototype]"), "{line}");
    assert!(line.contains("a: [Function (anonymous)]"), "{line}");
    assert!(line.contains("b: [Function: bee]"), "{line}");
}

#[test]
fn console_top_level_string_is_bare_nested_is_quoted() {
    assert_eq!(console_line("console.log('hello', 42)"), "hello 42");
    assert_eq!(console_line("console.log(['hello'])"), "[ 'hello' ]");
}

#[test]
fn console_handles_class_circular_and_builtins() {
    assert!(console_line("class P {} console.log(P)").contains("[class P]"));
    assert!(console_line("const o = {}; o.self = o; console.log(o)").contains("[Circular]"));
    assert!(console_line("console.log(new Map([['k', 1]]))").contains("Map(1) { 'k' => 1 }"));
}

#[test]
fn console_uses_a_class_s_custom_inspect() {
    let hook = "const I = Symbol.for('nodejs.util.inspect.custom');";
    // A string is printed as it is, nested too.
    let line = console_line(&format!(
        "{hook} class Id {{ [I]() {{ return 'Id(7)'; }} }} console.log(new Id(), [new Id()])"
    ));
    assert_eq!(line.trim(), "Id(7) [ Id(7) ]");
    // Anything else is inspected in its place; returning itself does not loop.
    assert!(
        console_line(&format!(
            "{hook} console.log({{ [I]() {{ return {{ a: 1 }}; }} }})"
        ))
        .contains("{ a: 1 }")
    );
    assert!(
        console_line(&format!(
            "{hook} const o = {{ x: 1, [I]() {{ return this; }} }}; console.log(o)"
        ))
        .contains("x: 1")
    );
    // A secret's redaction comes before any hook.
    assert!(console_line(&format!(
        "{hook} console.log({{ [Symbol.for('runtime.secret.redacted')]: true, [I]() {{ return 'leak'; }} }})"
    ))
    .contains("[redacted]"));
}
