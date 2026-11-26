//! `esdev test` (D59).

use super::*;

#[test]
fn test_jsx_compiles_the_way_the_project_configured_it() {
    let dir = build_dir("t_jsx_classic");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "esdev.json",
        r#"{ "jsx": { "factory": "h", "fragment": "Fragment" } }"#,
    );
    write_in(
        &dir,
        "src/runtime.js",
        "export function h(type, props, ...children) { return { type, props: props ?? {}, children: children.flat() }; }
         export const Fragment = 'fragment';
",
    );
    write_in(
        &dir,
        "src/view.test.jsx",
        "import { test, assertEquals } from 'runtime:test';
         import { h, Fragment } from './runtime.js';
         test('the project names the factory', () => {
           const el = <ul class=\'list\'><li>one</li><>two</></ul>;
           assertEquals([el.type, el.props.class], ['ul', 'list']);
           assertEquals([el.children[0].type, el.children[1].type], ['li', Fragment]);
         });
",
    );
    let ran = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test with classic jsx");
    assert!(
        ran.status.success(),
        "classic JSX did not run:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_jsx_with_nothing_configured_is_refused() {
    // The alternative is a default, and a default is a framework chosen for the
    // project by its build tool. The message has to be the whole answer,
    // because there is nowhere else to look it up.
    let dir = build_dir("t_jsx_unconfigured");
    write_in(&dir, "esdev.json", r#"{ "test": { "jobs": 1 } }"#);
    write_in(
        &dir,
        "view.test.jsx",
        "import { test } from 'runtime:test';
         test('never runs', () => (<div/>));
",
    );
    let ran = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        !ran.status.success(),
        "unconfigured JSX ran:\n{}",
        stdout(&ran)
    );
    let said = format!("{}{}", stdout(&ran), stderr(&ran));
    assert!(said.contains("nothing has said how JSX compiles"), "{said}");
    assert!(said.contains("importSource"), "{said}");
    assert!(said.contains("factory"), "{said}");
    assert!(said.contains("@jsxImportSource"), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_a_files_jsx_pragma_overrides_the_project() {
    let dir = build_dir("t_jsx_pragma");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    // The project imports the function from a package; the file names one it
    // defines itself.
    write_in(
        &dir,
        "esdev.json",
        r#"{ "jsx": { "importSource": "nothing" } }"#,
    );
    write_in(
        &dir,
        "src/pragma.test.jsx",
        "/** @jsx h */
         import { test, assertEquals } from 'runtime:test';
         function h(type) { return { type, made: 'by the pragma' }; }
         test('the file decides', () => {
           assertEquals((<span/>).made, 'by the pragma');
         });
",
    );
    let ran = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test with a jsx pragma");
    assert!(
        ran.status.success(),
        "a pragma did not override the project:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_a_config_with_no_targets_tests_but_does_not_build() {
    let dir = build_dir("t_jsx_no_targets");
    write_in(&dir, "esdev.json", r#"{ "test": { "jobs": 1 } }"#);
    write_in(
        &dir,
        "one.test.mjs",
        "import { test } from 'runtime:test';
test('runs', () => {});
",
    );
    let tested = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test without targets");
    assert!(
        tested.status.success(),
        "a config with no targets could not test:\n{}{}",
        stdout(&tested),
        stderr(&tested)
    );

    let built = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(!built.status.success());
    let text = format!("{}{}", stdout(&built), stderr(&built));
    assert!(text.contains("names no targets"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_runs_discovered_files_and_reports_failures() {
    let dir = build_dir("t_run");
    write_in(
        &dir,
        "ok.test.mjs",
        "import { test, assert } from 'runtime:test';\n\
         test('passes', () => assert(true));\n",
    );
    write_in(
        &dir,
        "bad.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('fails', () => assertEquals(1, 2));\n",
    );
    // Not a test file: discovery must not sweep in ordinary source.
    write_in(&dir, "helper.mjs", "export const x = 1;\n");

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));

    assert!(!out.status.success(), "a failing suite must exit non-zero");
    assert!(text.contains("ok.test.mjs"), "{text}");
    assert!(text.contains("bad.test.mjs"), "{text}");
    assert!(!text.contains("helper.mjs"), "{text}");
    assert!(text.contains("1 of 2 files failed"), "{text}");
}

#[test]
fn a_passing_suite_exits_zero() {
    let dir = build_dir("t_pass");
    write_in(
        &dir,
        "a.test.mjs",
        "import { test, assert, assertThrows, assertRejects } from 'runtime:test';\n\
         test('sync', () => assert(true));\n\
         test('async', async () => { const v = await Promise.resolve(1); assert(v === 1); });\n\
         test('throws', () => assertThrows(() => { throw new Error('x'); }));\n\
         test('rejects', async () => await assertRejects(async () => { throw new Error('x'); }));\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("4 passed"), "{}", stdout(&out));
}

#[test]
fn snapshots_are_versioned_explicitly_updated_and_checked() {
    let dir = build_dir("t_snapshots");
    write_in(
        &dir,
        "value.test.mjs",
        "import { test, expect } from 'runtime:test';\n\
         test('records supported values', () => {\n\
           const shared = { answer: 42 };\n\
           expect({ b: undefined, a: [BigInt(2), shared, shared], when: new Date('2020-01-01T00:00:00.000Z') }).toMatchSnapshot();\n\
         });\n",
    );

    let written = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        written.status.success(),
        "{}{}",
        stdout(&written),
        stderr(&written)
    );
    assert!(
        stdout(&written).contains("snapshots: 0 matched, 0 failed, 1 written"),
        "{}",
        stdout(&written)
    );

    let updated = esdev_in(&dir)
        .args(["test", "--update-snapshots"])
        .output()
        .expect("spawn esdev test --update-snapshots");
    assert!(
        updated.status.success(),
        "{}{}",
        stdout(&updated),
        stderr(&updated)
    );
    let snapshot = std::fs::read_to_string(dir.join("__snapshots__/value.test.mjs.snap"))
        .expect("read snapshot");
    assert!(snapshot.contains("// esdev snapshot v1"), "{snapshot}");
    assert!(
        snapshot.contains("=== records supported values: snapshot 1 [value]"),
        "{snapshot}"
    );
    assert!(snapshot.contains("[Circular]"), "{snapshot}");

    let checked = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        checked.status.success(),
        "{}{}",
        stdout(&checked),
        stderr(&checked)
    );

    write_in(
        &dir,
        "value.test.mjs",
        "import { test, expect } from 'runtime:test';\n\
         test('records supported values', () => expect({ answer: false }).toMatchSnapshot());\n",
    );
    let changed = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = stdout(&changed);
    assert!(!changed.status.success(), "a changed snapshot must fail");
    assert!(text.contains("--- snapshot"), "{text}");
    assert!(text.contains("+++ received"), "{text}");
    assert!(text.contains("- "), "{text}");
    assert!(text.contains("+ "), "{text}");

    let ci_changed = esdev_in(&dir)
        .arg("test")
        .env("CI", "1")
        .output()
        .expect("run changed snapshot in CI");
    let ci_text = stdout(&ci_changed);
    assert!(
        !ci_changed.status.success(),
        "changed CI snapshot must fail"
    );
    assert!(ci_text.contains("snapshot changed"), "{ci_text}");
    assert!(
        !ci_text.contains("accept with:"),
        "CI must not suggest accepting a snapshot:\n{ci_text}"
    );
}

#[test]
fn ci_never_creates_a_missing_snapshot() {
    let dir = build_dir("t_snapshot_ci");
    write_in(
        &dir,
        "ci.test.mjs",
        "import { test, expect } from 'runtime:test';\n\
         test('requires a committed snapshot', () => expect({ ok: true }).toMatchSnapshot());\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .arg("--ci")
        .output()
        .expect("spawn esdev test --ci");
    let text = stdout(&out);
    assert!(!out.status.success(), "CI must reject missing snapshots");
    assert!(text.contains("--ci does not write them"), "{text}");
    assert!(
        !dir.join("__snapshots__/ci.test.mjs.snap").exists(),
        "CI wrote a snapshot:\n{text}"
    );
}

#[test]
fn snapshots_print_stable_values_errors_and_files() {
    let dir = build_dir("t_snapshot_printer");
    write_in(
        &dir,
        "printer.test.mjs",
        "import { test, expect } from 'runtime:test';\n\
         test('prints values', () => {\n\
           const cycle = {}; cycle.self = cycle;\n\
           expect({ text: 'one\\ntwo', big: 9n, bytes: new Uint8Array([3, 1]), map: new Map([['z', 1], ['a', 2]]), set: new Set(['z', 'a']), objectMap: new Map([[{ z: 1 }, 'z'], [{ a: 1 }, 'a']]), cycle }).toMatchSnapshot();\n\
           expect('<main>ada</main>\\n').toMatchFileSnapshot('home.html');\n\
           expect(new Uint8Array([0, 255, 3])).toMatchFileSnapshot('chart.bin');\n\
         });\n\
         test('prints errors', () => expect(() => { const error = new Error('broken', { cause: 'network' }); error.code = 'E_BROKEN'; throw error; }).toThrowErrorMatchingSnapshot());\n",
    );
    let first = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        first.status.success(),
        "{}{}",
        stdout(&first),
        stderr(&first)
    );
    let snapshot = std::fs::read_to_string(dir.join("__snapshots__/printer.test.mjs.snap"))
        .expect("read snapshot");
    assert!(snapshot.contains("9n"), "{snapshot}");
    assert!(snapshot.contains("Uint8Array [3, 1]"), "{snapshot}");
    assert!(snapshot.contains("Map {\n    \"a\" => 2,"), "{snapshot}");
    assert!(
        snapshot.contains("\"objectMap\": Map {\n    {\n      \"a\": 1,"),
        "{snapshot}"
    );
    assert!(snapshot.contains("Set {\n    \"a\","), "{snapshot}");
    assert!(snapshot.contains("[Circular]"), "{snapshot}");
    assert!(snapshot.contains("[error]"), "{snapshot}");
    assert!(snapshot.contains("E_BROKEN"), "{snapshot}");
    assert!(snapshot.contains("\"cause\": \"network\""), "{snapshot}");
    assert_eq!(
        std::fs::read(dir.join("__snapshots__/printer.test.mjs/chart.bin")).expect("read bytes"),
        vec![0, 255, 3]
    );
    let second = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("rerun esdev test");
    assert!(
        second.status.success(),
        "{}{}",
        stdout(&second),
        stderr(&second)
    );
    assert!(
        stdout(&second).contains("snapshots: 4 matched"),
        "{}",
        stdout(&second)
    );
}

#[test]
fn a_snapshot_property_matcher_fails_before_snapshot_comparison() {
    let dir = build_dir("t_snapshot_matcher_failure");
    write_in(
        &dir,
        "matcher.test.mjs",
        "import { test, expect } from 'runtime:test';\n\
         test('checks volatile fields', () => expect({ id: 1 }).toMatchSnapshot({ id: expect.any(String) }));\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = stdout(&out);
    assert!(!out.status.success(), "a bad matcher must fail");
    assert!(
        text.contains("snapshot value did not satisfy its property matchers"),
        "{text}"
    );
    assert!(
        !dir.join("__snapshots__/matcher.test.mjs.snap").exists(),
        "{text}"
    );
}

#[test]
fn snapshots_number_hints_and_honor_the_ci_environment() {
    let dir = build_dir("t_snapshot_keys");
    write_in(
        &dir,
        "keys.test.mjs",
        "import { describe, test, assertSnapshot } from 'runtime:test';\n\
         describe('outer', () => describe('inner', () => test('keys', () => {\n\
           assertSnapshot({ first: true }, 'after lowering');\n\
           assertSnapshot({ second: true }, 'after lowering');\n\
         })));\n",
    );
    let first = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("write snapshots");
    assert!(
        first.status.success(),
        "{}{}",
        stdout(&first),
        stderr(&first)
    );
    let snapshot = std::fs::read_to_string(dir.join("__snapshots__/keys.test.mjs.snap"))
        .expect("read snapshot");
    assert!(
        snapshot.contains("outer > inner > keys: after lowering 1"),
        "{snapshot}"
    );
    assert!(
        snapshot.contains("outer > inner > keys: after lowering 2"),
        "{snapshot}"
    );
    assert!(
        stdout(&first).contains("written:\n  "),
        "{}",
        stdout(&first)
    );

    write_in(
        &dir,
        "missing.test.mjs",
        "import { test, expect } from 'runtime:test'; test('CI', () => expect(1).toMatchSnapshot());\n",
    );
    let ci = esdev_in(&dir)
        .arg("test")
        .env("CI", "1")
        .output()
        .expect("run CI test");
    assert!(!ci.status.success(), "CI must reject a missing snapshot");
    assert!(
        stdout(&ci).contains("--ci does not write them"),
        "{}",
        stdout(&ci)
    );
    assert!(!dir.join("__snapshots__/missing.test.mjs.snap").exists());
}

#[test]
fn file_snapshot_diffs_and_filtered_updates_keep_other_entries() {
    let dir = build_dir("t_snapshot_file_diff");
    write_in(
        &dir,
        "files.test.mjs",
        "import { test, expect } from 'runtime:test';\n\
         test('text', () => expect('before\\n').toMatchFileSnapshot('page.html'));\n\
         test('binary', () => expect(new Uint8Array([1, 2, 3])).toMatchFileSnapshot('bytes.bin'));\n\
         test('kept', () => expect({ kept: true }).toMatchSnapshot());\n",
    );
    let initial = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("write snapshots");
    assert!(
        initial.status.success(),
        "{}{}",
        stdout(&initial),
        stderr(&initial)
    );
    write_in(
        &dir,
        "files.test.mjs",
        "import { test, expect } from 'runtime:test';\n\
         test('text', () => expect('after\\n').toMatchFileSnapshot('page.html'));\n\
         test('binary', () => expect(new Uint8Array([1, 9, 3])).toMatchFileSnapshot('bytes.bin'));\n",
    );
    let changed = esdev_in(&dir).arg("test").output().expect("diff snapshots");
    let text = stdout(&changed);
    assert!(!changed.status.success(), "file changes must fail");
    assert!(text.contains("file snapshot differs:"), "{text}");
    assert!(text.contains("--- snapshot"), "{text}");
    assert!(text.contains("first differing byte 1"), "{text}");
    assert!(text.contains("--file=files.test.mjs"), "{text}");

    let filtered = esdev_in(&dir)
        .args(["test", "--update-snapshots", "files.test.mjs"])
        .output()
        .expect("update filtered test");
    assert!(
        filtered.status.success(),
        "{}{}",
        stdout(&filtered),
        stderr(&filtered)
    );
    let values = std::fs::read_to_string(dir.join("__snapshots__/files.test.mjs.snap"))
        .expect("read retained snapshot");
    assert!(
        values.contains("kept: snapshot 1"),
        "filtered update pruned an entry:\n{values}"
    );

    let pruned = esdev_in(&dir)
        .args(["test", "--update-snapshots"])
        .output()
        .expect("prune complete test file");
    assert!(
        pruned.status.success(),
        "{}{}",
        stdout(&pruned),
        stderr(&pruned)
    );
    assert!(stdout(&pruned).contains("1 removed"), "{}", stdout(&pruned));
    let values = std::fs::read_to_string(dir.join("__snapshots__/files.test.mjs.snap"))
        .expect("read pruned snapshot");
    assert!(!values.contains("kept: snapshot 1"), "{values}");

    write_in(
        &dir,
        "files.test.mjs",
        "import { test } from 'runtime:test'; test('no snapshots remain', () => {});\n",
    );
    let remove_files = esdev_in(&dir)
        .args(["test", "--update-snapshots"])
        .output()
        .expect("remove orphaned file snapshots");
    assert!(
        remove_files.status.success(),
        "{}{}",
        stdout(&remove_files),
        stderr(&remove_files)
    );
    assert!(
        stdout(&remove_files).contains("2 removed"),
        "{}",
        stdout(&remove_files)
    );
    assert!(!dir.join("__snapshots__/files.test.mjs/page.html").exists());
    assert!(!dir.join("__snapshots__/files.test.mjs/bytes.bin").exists());
}

/// A `.test.ts` file is the ordinary case: it must be stripped like any other,
/// and its relative imports must resolve from its own directory.
#[test]
fn a_typescript_test_file_runs_with_its_imports() {
    let dir = build_dir("t_ts");
    write_in(
        &dir,
        "math.ts",
        "export const add = (a: number, b: number): number => a + b;\n",
    );
    write_in(
        &dir,
        "math.test.ts",
        "import { test, assertEquals } from 'runtime:test';\n\
         import { add } from './math.ts';\n\
         interface Case { a: number; b: number; want: number }\n\
         test('adds', () => {\n\
         \x20 const c: Case = { a: 2, b: 3, want: 5 };\n\
         \x20 assertEquals(add(c.a, c.b), c.want);\n\
         });\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("1 passed"), "{}", stdout(&out));
}

/// `expect` is a second spelling of the assertions, not a second
/// implementation — so this drives the whole vocabulary through one file and
/// checks the tally, rather than trusting that a matcher nobody ran works.
///
/// Every matcher appears, in both directions where negation means anything,
/// because a matcher that forgot to consult `not` passes its positive test
/// perfectly.
#[test]
fn the_expect_vocabulary_holds_in_both_directions() {
    let dir = build_dir("t_expect");
    write_in(
        &dir,
        "expect.test.ts",
        r#"
import { test, expect } from "runtime:test";

test("values", () => {
  expect(2 + 2).toBe(4);
  expect({ a: [1, { b: 2 }] }).toEqual({ a: [1, { b: 2 }] });
  expect({ a: 1 }).toStrictEqual({ a: 1 });
  expect(1).not.toBe(2);
  expect("").toBeFalsy();
  expect("x").toBeTruthy();
  expect(null).toBeNull();
  expect(undefined).toBeUndefined();
  expect(0).toBeDefined();
  expect(Number.NaN).toBeNaN();
  expect(new Error("x")).toBeInstanceOf(Error);
  expect("x").toBeTypeOf("string");
  expect(0.1 + 0.2).toBeCloseTo(0.3);
  expect(3).toBeGreaterThan(2);
  expect(3).toBeGreaterThanOrEqual(3);
  expect(2).toBeLessThan(3);
  expect(2).toBeLessThanOrEqual(2);
});

test("collections and text", () => {
  expect([1, 2, 3]).toContain(2);
  expect([{ a: 1 }]).toContainEqual({ a: 1 });
  expect([1, 2]).toHaveLength(2);
  expect("hello").toHaveLength(5);
  expect({ a: { b: 1 } }).toHaveProperty("a.b", 1);
  expect("hello").toMatch(/ell/);
  expect("hello").toMatch("ell");
  expect({ a: 1, b: 2 }).toMatchObject({ a: 1 });
  expect([1, 2]).not.toContain(9);
});

test("throwing, and the alias", () => {
  const boom = () => {
    throw new TypeError("bad input");
  };
  expect(boom).toThrow();
  expect(boom).toThrow("bad input");
  expect(boom).toThrow(/bad/);
  expect(boom).toThrow(TypeError);
  expect(boom).toThrowError("bad input");
  expect(() => {}).not.toThrow();
});

test("promises", async () => {
  await expect(Promise.resolve(7)).resolves.toBe(7);
  await expect(Promise.reject(new Error("no"))).rejects.toThrow("no");
  await expect(Promise.resolve(7)).resolves.not.toBe(8);
});

test("asymmetric matchers, including nested", () => {
  expect({ id: 1, name: "a", at: new Date() }).toEqual({
    id: expect.any(Number),
    name: expect.stringContaining("a"),
    at: expect.anything(),
  });
  expect([1, 2, 3]).toEqual(expect.arrayContaining([3, 1]));
  expect({ a: 1, b: 2 }).toEqual(expect.objectContaining({ a: 1 }));
  expect("hello").toEqual(expect.stringMatching(/^he/));
});

// A matcher that does not fail when it should is the failure this whole file
// is about, so each direction is provoked once.
test("a matcher that should fail, does", () => {
  const refused = (fn: () => void) => {
    let threw = false;
    try {
      fn();
    } catch {
      threw = true;
    }
    if (!threw) throw new Error("that assertion should have failed");
  };
  refused(() => expect(1).toBe(2));
  refused(() => expect(1).not.toBe(1));
  refused(() => expect([1]).toHaveLength(2));
  refused(() => expect("a").toMatch(/b/));
  refused(() => expect(() => {}).toThrow());
});
"#,
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("6 passed, 0 failed"), "{text}");
}

/// `mock` and `clock` — the two subsystems that stand in for something real.
///
/// The clock is the half worth driving end to end: it replaces `setTimeout`,
/// `setInterval` and `Date` on `globalThis` for the whole process, and the
/// property that makes that safe is that a test file is a process. Freezing it
/// here and never releasing it in the last case is deliberate — the runner
/// drains on microtasks, so it must still print a tally.
#[test]
fn mocks_record_calls_and_the_clock_moves_only_when_told() {
    let dir = build_dir("t_mock");
    write_in(
        &dir,
        "mock.test.ts",
        r#"
import { test, expect, mock, clock } from "runtime:test";

test("a mock records what it was asked", () => {
  const add = mock.fn((a: number, b: number) => a + b);
  expect(add(1, 2)).toBe(3);
  add(4, 5);
  expect(add).toHaveBeenCalled();
  expect(add).toHaveBeenCalledTimes(2);
  expect(add).toHaveBeenCalledWith(1, 2);
  expect(add).toHaveBeenLastCalledWith(4, 5);
  expect(add).toHaveBeenNthCalledWith(1, 1, 2);
  expect(add).toHaveReturnedWith(9);
  expect(add).toHaveReturnedTimes(2);
  expect(add).not.toHaveBeenCalledWith(9, 9);
  // The short spellings are the same matchers.
  expect(add).toBeCalledTimes(2);
  expect(add).lastCalledWith(4, 5);
  expect(add.mock.calls).toEqual([[1, 2], [4, 5]]);
  const once = mock.fn(); once();
  expect(once).toHaveBeenCalledOnce();
});

test("it answers however it was told, once and then always", () => {
  const f = mock.fn().mockReturnValueOnce("first").mockReturnValue("rest");
  expect([f(), f(), f()]).toEqual(["first", "rest", "rest"]);
  const g = mock.fn().mockResolvedValue("ok");
  expect(g()).resolves;
  const boom = mock.fn(() => {
    throw new Error("boom");
  });
  expect(boom).toThrow("boom");
  expect(boom.mock.results[0].type).toBe("throw");
});

test("clear, reset, restore", () => {
  const f = mock.fn(() => 1).mockReturnValue(2);
  f();
  f.mockClear();
  expect(f).not.toHaveBeenCalled();
  f.mockReset();
  expect(f()).toBe(1);
});

test("a call matcher refuses anything that is not a mock", () => {
  expect(() => expect(() => {}).toHaveBeenCalled()).toThrow("needs a mock");
});

test("the failure names the mock", () => {
  const send = mock.fn().mockName("send");
  expect(() => expect(send).toHaveBeenCalled()).toThrow("send");
});

test("a spy calls through, and gives the method back", () => {
  const object = { double: (n: number) => n * 2 };
  const spy = mock.spyOn(object, "double");
  expect(object.double(4)).toBe(8);
  expect(spy).toHaveBeenCalledWith(4);
  spy.mockRestore();
  expect(mock.is(object.double)).toBe(false);
  expect(object.double(4)).toBe(8);
});

test("an inherited method goes back to the prototype", () => {
  class Greeter {
    hello() {
      return "hi";
    }
  }
  const greeter = new Greeter();
  const spy = mock.spyOn(greeter, "hello");
  expect(greeter.hello()).toBe("hi");
  spy.mockRestore();
  expect(Object.hasOwn(greeter, "hello")).toBe(false);
});

test("accessor spies call through and restore inherited descriptors", () => {
  class Meter {
    #value = 2;
    get value() { return this.#value; }
    set value(next: number) { this.#value = next; }
  }
  const meter = new Meter();
  const read = mock.spyOn(meter, "value", "get");
  expect(meter.value).toBe(2);
  expect(read).toHaveBeenCalledOnce();
  read.mockRestore();
  expect(Object.hasOwn(meter, "value")).toBe(false);
  const write = mock.spyOn(meter, "value", "set");
  meter.value = 7;
  expect(write).toHaveBeenCalledWith(7);
  write.mockRestore();
  expect(Object.hasOwn(meter, "value")).toBe(false);
  expect(meter.value).toBe(7);
});

test("a replaced global is put back by restoreAll", () => {
  const before = globalThis.fetch;
  mock.global("fetch", mock.fn());
  expect(mock.is(globalThis.fetch)).toBe(true);
  mock.restoreAll();
  expect(globalThis.fetch).toBe(before);
});

test("nothing fires until the clock is moved", () => {
  clock.freeze();
  const ran = mock.fn();
  setTimeout(ran, 100);
  expect(clock.pending()).toBe(1);
  clock.advance(99);
  expect(ran).not.toHaveBeenCalled();
  clock.advance(1);
  expect(ran).toHaveBeenCalledTimes(1);
  clock.release();
});

test("an interval repeats, and clearing it from inside stops it", () => {
  clock.freeze();
  let seen = 0;
  const id = setInterval(() => {
    seen += 1;
    if (seen === 3) clearInterval(id);
  }, 10);
  clock.advance(100);
  expect(seen).toBe(3);
  expect(clock.pending()).toBe(0);
  clock.release();
});

test("Date moves with the clock", () => {
  clock.freeze(new Date("2020-01-01T00:00:00Z"));
  expect(Date.now()).toBe(1577836800000);
  expect(new Date().toISOString()).toBe("2020-01-01T00:00:00.000Z");
  clock.advance(1000);
  expect(Date.now()).toBe(1577836801000);
  clock.release();
});

// The distinction the async form exists for.
test("the async form resumes a chain of awaits; the sync form does not", async () => {
  clock.freeze();
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
  const twice = () => {
    let done = false;
    void (async () => {
      await sleep(50);
      await sleep(50);
      done = true;
    })();
    return () => done;
  };

  const synchronous = twice();
  clock.advance(100);
  expect(synchronous()).toBe(false);

  const asynchronous = twice();
  await clock.advanceAsync(100);
  expect(asynchronous()).toBe(true);
  clock.release();
});

test("runAll drains, and refuses a queue that never will", () => {
  clock.freeze();
  const ran = mock.fn();
  setTimeout(() => setTimeout(ran, 10), 10);
  clock.runAll();
  expect(ran).toHaveBeenCalled();
  setInterval(() => {}, 1);
  expect(() => clock.runAll()).toThrow("not draining");
  clock.clear();
  clock.release();
});

test("runPending fires an interval once rather than for ever", () => {
  clock.freeze();
  const tick = mock.fn();
  setInterval(tick, 10);
  clock.runPending();
  expect(tick).toHaveBeenCalledTimes(1);
  clock.release();
});

test("advancing a clock that is not frozen says which verb to call", () => {
  expect(() => clock.advance(1)).toThrow("clock.freeze()");
});

// Left frozen on purpose: the runner drains on microtasks, so a file that
// forgets to release the clock must still report.
test("a file may end with time stopped", () => {
  clock.freeze();
  expect(clock.isFrozen()).toBe(true);
});
"#,
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("17 passed, 0 failed"), "{text}");
}

/// The vocabulary a suite written elsewhere reaches for: `it`/`suite`, a table,
/// a to-do, and the conditional forms.
///
/// Driven end to end rather than unit-tested, because what matters is the
/// **tally** — a table that registered one case instead of three, or a to-do
/// that vanished rather than being counted as skipped, both look like a passing
/// file from the inside.
#[test]
fn the_table_and_alias_forms_register_what_they_say() {
    let dir = build_dir("t_vocab");
    write_in(
        &dir,
        "vocab.test.ts",
        r#"
import { it, suite, test, describe, expect } from "runtime:test";

suite("aliases", () => {
  it("it is test", () => expect(1).toBe(1));
});

test.each([
  [1, 1, 2],
  [2, 3, 5],
])("adds %d + %d = %d", (a: number, b: number, want: number) => {
  expect(a + b).toBe(want);
});

test.each([{ name: "ada" }, { name: "alan" }])("$name is named", (row: { name: string }) => {
  expect(row.name.length).toBeGreaterThan(0);
});

// Rows whose name does not vary still get distinct identities.
test.each([1, 2, 3])("same name", (n: number) => expect(n).toBeGreaterThan(0));

describe.each([["a"], ["b"]])("group %s", (letter: string) => {
  test("has a letter", () => expect(letter).toHaveLength(1));
});

test.todo("written later");
test.skipIf(true)("not here", () => expect(1).toBe(2));
test.runIf(true)("but here", () => expect(1).toBe(1));
"#,
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    // 1 alias + 2 table + 2 object table + 3 same-name + 2 groups + 1 runIf.
    assert!(text.contains("11 passed"), "{text}");
    // The to-do and the skipIf, counted rather than missing from the report.
    assert!(text.contains("2 skipped"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--setup` imports a module before the file, and **does not move a line**.
///
/// The second half is the one that could regress silently. A prelude prepended
/// to the *source* goes through the printer, which lays a statement on a line
/// of its own — so the file runs one line lower than it was written, and every
/// frame in every failure points one line off (D71). It is applied to the
/// printer's output instead.
#[test]
fn a_setup_module_runs_first_and_costs_no_line_numbers() {
    let dir = build_dir("t_setup");
    write_in(
        &dir,
        "setup.ts",
        "(globalThis as unknown as { __ready: string }).__ready = \"yes\";\n",
    );
    write_in(
        &dir,
        "s.test.ts",
        r#"import { test, expect } from "runtime:test";
test("the setup ran first", () => {
  expect((globalThis as unknown as { __ready?: string }).__ready).toBe("yes");
});
test("and the line is the one written", () => {
  const err = new Error("x");
  expect(String(err.stack)).toContain("s.test.ts:6");
});
"#,
    );

    // Without it the first case fails: nothing put the global there.
    let bare = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(!bare.status.success(), "the setup was not needed");

    let out = esdev_in(&dir)
        .args(["test", "--setup=./setup.ts"])
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("2 passed"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--setup` names its module from the project root, exactly as the `setup` key
/// does — not from each test file's own directory.
///
/// The bases only differ once a test sits under a subdirectory, so that is the
/// case that pins it: with both files at the root a relative path resolves the
/// same way under either reading, and a wrong base stays invisible. A path that
/// the file accepts must be the path the flag accepts.
#[test]
fn a_setup_flag_names_its_module_from_the_project_root() {
    let dir = build_dir("t_setup_root");
    std::fs::create_dir_all(dir.join("test")).expect("create test dir");
    std::fs::create_dir_all(dir.join("src")).expect("create src dir");
    write_in(
        &dir,
        "test/setup.ts",
        "(globalThis as unknown as { __ready: string }).__ready = \"yes\";\n",
    );
    write_in(
        &dir,
        "src/a.test.ts",
        r#"import { test, expect } from "runtime:test";
test("the setup ran first", () => {
  expect((globalThis as unknown as { __ready?: string }).__ready).toBe("yes");
});
"#,
    );

    // The flag's path is the project's, so `./test/setup.ts` is the file under
    // `test/` — read from `src/` it would name a file that does not exist.
    let out = esdev_in(&dir)
        .args(["test", "--setup=./test/setup.ts"])
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("1 passed"), "{text}");

    // The same path in `esdev.json` resolves the same way: a flag and the file
    // cannot disagree about which file a setup path names.
    write_in(
        &dir,
        "esdev.json",
        r#"{ "test": { "setup": ["./test/setup.ts"] } }"#,
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("1 passed"), "{text}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A file that never finishes is **ended**, not waited on.
///
/// The failure this pins is subtle: cancelling the future that waits on a child
/// does not end the child. The first version of the budget did exactly that and
/// left the process running, holding the inherited stdout — so the run appeared
/// to hang after the timeout had already fired.
#[test]
fn a_file_that_runs_too_long_is_stopped_and_failed() {
    let dir = build_dir("t_timeout");
    write_in(
        &dir,
        "hang.test.mjs",
        "import { test } from 'runtime:test';\n\
         setInterval(() => {}, 1000);\n\
         test('passes, but the file never exits', () => {});\n",
    );
    for jobs in ["--jobs=1", "--jobs=2"] {
        let started = std::time::Instant::now();
        let out = esdev_in(&dir)
            .args(["test", "--timeout=1500", jobs])
            .output()
            .expect("spawn esdev test");
        let text = format!("{}{}", stdout(&out), stderr(&out));
        assert!(!out.status.success(), "{jobs}: {text}");
        assert!(text.contains("longer than 1500ms"), "{jobs}: {text}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(20),
            "{jobs}: the child outlived its budget — it was abandoned, not ended"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--reporter=json` is one object per line, and every line parses.
#[test]
fn the_json_reporter_emits_one_object_per_line() {
    let dir = build_dir("t_reporter");
    write_in(
        &dir,
        "ok.test.mjs",
        "import { test, expect } from 'runtime:test';\n\
         test('passes', () => expect(1).toBe(1));\n\
         test.skip('skipped', () => {});\n",
    );
    write_in(
        &dir,
        "bad.test.mjs",
        "import { test, expect } from 'runtime:test';\n\
         test('fails \"quoted\"', () => expect(1).toBe(2));\n",
    );
    let out = esdev_in(&dir)
        .args(["test", "--reporter=json"])
        .output()
        .expect("spawn esdev test");
    assert!(!out.status.success());
    let text = stdout(&out);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(lines.len() >= 4, "{lines:?}");
    for line in &lines {
        // A quoted test name and a multi-line stack both have to survive.
        assert!(
            serde_json::from_str::<serde_json::Value>(line).is_ok(),
            "not JSON: {line}"
        );
    }
    let last = lines.last().expect("a summary");
    assert!(last.contains("\"type\":\"summary\""), "{last}");
    assert!(last.contains("\"failed\":1"), "{last}");
    // No human chrome: a filename printed above the objects would have to be
    // skipped by whatever is parsing them.
    assert!(!text.contains("files passed"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The `test` section says the same things the flags say, and a flag wins.
#[test]
fn the_test_section_configures_the_run_and_a_flag_beats_it() {
    let dir = build_dir("t_testcfg");
    write_in(
        &dir,
        "setup.ts",
        "(globalThis as unknown as { __cfg: boolean }).__cfg = true;\n",
    );
    write_in(
        &dir,
        "c.test.ts",
        r#"import { test, expect } from "runtime:test";
test("the file's setup ran", () => {
  expect((globalThis as unknown as { __cfg?: boolean }).__cfg).toBe(true);
});
"#,
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"test": {"setup": "./setup.ts", "reporter": "json", "jobs": 1}, "build": {"targets": {"app": {"entry": "c.test.ts", "out": "dist/app.js"}}}}"#,
    );

    let from_file = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        from_file.status.success(),
        "{}{}",
        stdout(&from_file),
        stderr(&from_file)
    );
    assert!(
        stdout(&from_file).contains("\"type\":\"summary\""),
        "the file's reporter was ignored:\n{}",
        stdout(&from_file)
    );

    let overridden = esdev_in(&dir)
        .args(["test", "--reporter=human"])
        .output()
        .expect("spawn esdev test");
    assert!(overridden.status.success(), "{}", stderr(&overridden));
    assert!(
        stdout(&overridden).contains("1 file passed"),
        "a flag did not beat the file:\n{}",
        stdout(&overridden)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The property that makes a failure actionable: the frame names the line the
/// developer wrote. It used to be the interesting test, because a harness was
/// prepended to the file and had to be folded onto one line to avoid moving
/// every line number. Nothing is injected now — the test API is imported — so
/// this asserts the property still holds with the mechanism gone.
#[test]
fn a_failure_names_the_line_the_developer_wrote() {
    let dir = build_dir("t_lines");
    write_in(
        &dir,
        "lines.test.mjs",
        "import { test, assert } from 'runtime:test';\n\
         test('fails on line four', () => {\n  const x = 1;\n  assert(x === 2, 'nope');\n});\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success());
    assert!(
        text.contains("lines.test.mjs:4:"),
        "the failing line was renumbered:\n{text}"
    );
}

/// The `.ts` counterpart, and the one that has always been at risk: a typed
/// file goes through oxc's codegen, which re-prints it. The old harness was
/// prepended *before* that step and came back out unfolded, reporting line 44
/// for an assertion on line 3 — a bug the `.mjs` sibling above could never
/// catch, because `.mjs` never reaches the printer. There is no harness to
/// unfold now; what is left under test is the stripper itself.
#[test]
fn a_typescript_failure_names_the_line_the_developer_wrote() {
    let dir = build_dir("t_lines_ts");
    write_in(
        &dir,
        "lines.test.ts",
        "import { test, assert } from 'runtime:test';\n\
         test('fails on line four', () => {\n  const x: number = 1;\n  assert(x === 2, 'nope');\n});\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success());
    assert!(
        text.contains("lines.test.ts:4:"),
        "the typed file was renumbered:\n{text}"
    );
}

/// `assertEquals` walks the values rather than stringifying them. The `BigInt`
/// case is the one that forced this: `JSON.stringify` throws on one, so on a
/// runtime with int64 the assertion could not be written at all.
#[test]
fn assert_equals_compares_structurally() {
    let dir = build_dir("t_deep_equal");
    write_in(
        &dir,
        "eq.test.mjs",
        r#"
import { test, assert, assertEquals } from "runtime:test";

const no = (fn, label) => {
  let threw = false;
  try { fn(); } catch { threw = true; }
  assert(threw, label + ": should have failed");
};
test('holds', () => {
  assertEquals(-9223372036854775808n, -9223372036854775808n);
  assertEquals({ id: 1n }, { id: 1n });
  assertEquals(new Uint8Array([1, 2, 3]), new Uint8Array([1, 2, 3]));
  assertEquals(new Uint8Array([9, 1, 2]).subarray(1), new Uint8Array([1, 2]));
  assertEquals(NaN, NaN);
  assertEquals({ a: 1, b: 2 }, { b: 2, a: 1 });
  assertEquals(new Map([['k', 1n]]), new Map([['k', 1n]]));
  assertEquals(new Set([1, 2]), new Set([2, 1]));
  const cyclic = { name: 'x' }; cyclic.self = cyclic;
  const twin = { name: 'x' }; twin.self = twin;
  assertEquals(cyclic, twin);

  no(() => assertEquals(1n, 2n), 'unequal bigint');
  no(() => assertEquals(1n, 1), 'bigint vs number');
  no(() => assertEquals(new Uint8Array([1]), new Uint8Array([2])), 'bytes');
  no(() => assertEquals(new Uint8Array([1]), new Int8Array([1])), 'view type');
  no(() => assertEquals({ a: 1 }, { a: 1, b: 2 }), 'extra key');
  no(() => assertEquals(new Map([['k', 1]]), new Map([['k', 2]])), 'map value');
});
"#,
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
}

/// The second argument is the expectation, not a label. It used to be the
/// latter, which meant `assertThrows(fn, "TypeError")` asserted nothing at all.
#[test]
fn assert_throws_checks_the_error_it_was_given() {
    let dir = build_dir("t_throws");
    write_in(
        &dir,
        "throws.test.mjs",
        r#"
import { test, assert, assertThrows, assertRejects } from "runtime:test";

const no = (fn, label) => {
  let threw = false;
  try { fn(); } catch { threw = true; }
  assert(threw, label + ": should have failed");
};
const boom = () => { throw new TypeError('field number 0 is not allowed'); };
test('holds', () => {
  assertThrows(boom);
  assertThrows(boom, 'TypeError');
  assertThrows(boom, 'field number 0');
  assertThrows(boom, /number 0 is not/);
  assertThrows(boom, TypeError);

  no(() => assertThrows(boom, 'RangeError'), 'wrong name');
  no(() => assertThrows(boom, /depth/), 'wrong pattern');
  no(() => assertThrows(boom, RangeError), 'wrong constructor');
  no(() => assertThrows(() => 1), 'never threw');
  no(() => assertThrows(() => 1, 'TypeError'), 'never threw, name wanted');
});
test('async', async () => {
  const rejects = async () => { throw new RangeError('depth limit'); };
  await assertRejects(rejects);
  await assertRejects(rejects, 'RangeError');
  await assertRejects(rejects, /depth/);
  await assertRejects(rejects, RangeError);

  let threw = false;
  try { await assertRejects(rejects, 'TypeError'); } catch { threw = true; }
  assert(threw, 'a wrong name should have failed');
  threw = false;
  try { await assertRejects(async () => 1); } catch { threw = true; }
  assert(threw, 'a resolving promise should have failed');
});
"#,
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
}

/// What the globals could not do: a **helper module** beside the test file can
/// use the assertions. The harness was injected into the entry only, so a
/// shared `test-helpers.ts` — the one place a suite most wants to share code —
/// had no `assertEquals` to call.
#[test]
fn a_helper_module_can_import_the_assertions() {
    let dir = build_dir("t_helper");
    write_in(
        &dir,
        "helper.mjs",
        "import { assertEquals } from 'runtime:test';\n\
         export const assertSorted = (xs) => assertEquals(xs, [...xs].sort());\n",
    );
    write_in(
        &dir,
        "use.test.mjs",
        "import { test } from 'runtime:test';\n\
         import { assertSorted } from './helper.mjs';\n\
         test('a helper asserts', () => assertSorted([1, 2, 3]));\n",
    );

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("1 passed"), "{}", stdout(&out));
}

/// Cases run one at a time, in the order they were written, with the lifecycle
/// hooks around each. Before this, `test()` *called* the function, so two async
/// cases sharing a variable ran at once and a `beforeEach` had no "before" to
/// happen in.
#[test]
fn tests_run_one_at_a_time_with_their_lifecycle_hooks() {
    let dir = build_dir("t_serial");
    write_in(
        &dir,
        "order.test.mjs",
        r#"
import { test, beforeAll, afterAll, beforeEach, afterEach, assertEquals } from "runtime:test";
const order = [];
let shared = 0;
beforeAll(() => order.push("beforeAll"));
afterAll(() => { order.push("afterAll"); console.log(order.join(" ")); });
beforeEach(() => { shared = 0; order.push("beforeEach"); });
afterEach(() => order.push("afterEach"));
test("slow", async () => {
  await new Promise((r) => setTimeout(r, 30));
  shared += 1;
  order.push("slow");
  assertEquals(shared, 1);
});
test("fast", () => {
  shared += 5;
  order.push("fast");
  assertEquals(shared, 5, "the slow test was still running");
});
"#,
    );

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(
        text.contains("beforeAll beforeEach slow afterEach beforeEach fast afterEach afterAll"),
        "{text}"
    );
    assert!(text.contains("2 passed"), "{text}");
}

/// A group is a **scope**, not a naming convention: a hook written inside one
/// belongs to the tests inside it. Without that, `describe` is a template
/// string, and a file that sets up a database for six of its twenty cases sets
/// it up for the other fourteen too.
#[test]
fn a_group_scopes_its_hooks_and_composes_its_names() {
    let dir = build_dir("t_group");
    write_in(
        &dir,
        "group.test.mjs",
        r#"
import { test, describe, beforeAll, afterAll, beforeEach, afterEach } from "runtime:test";
const order = [];
beforeAll(() => order.push("file:all"));
afterAll(() => { order.push("file:/all"); console.log(order.join(" ")); });
beforeEach(() => order.push("file:each"));
afterEach(() => order.push("file:/each"));

test("top", () => order.push("top"));

describe("db", () => {
  beforeAll(() => order.push("db:all"));
  afterAll(() => order.push("db:/all"));
  beforeEach(() => order.push("db:each"));
  test("inserts", () => order.push("inserts"));
  describe("nested", () => {
    beforeEach(() => order.push("nested:each"));
    test("deep", () => order.push("deep"));
  });
});

test("last", () => order.push("last"));
"#,
    );

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    // The file's hooks wrap every case; the group's wrap only its own, and its
    // `afterAll` runs when its last case has run rather than at the end of the
    // file. `beforeEach` runs outermost-first and `afterEach` innermost-first.
    assert!(
        text.contains(
            "file:all file:each top file:/each \
             db:all file:each db:each inserts file:/each \
             file:each db:each nested:each deep file:/each \
             db:/all file:each last file:/each file:/all"
        ),
        "{text}"
    );
    assert!(text.contains("4 passed"), "{text}");

    // A case is reported under its groups, outermost first, so a failure names
    // where it lives rather than only what it is called.
    write_in(
        &dir,
        "named.test.mjs",
        "import { test, describe, assert } from 'runtime:test';\n         describe('db', () => describe('inserts', () => test('rejects a null', () => assert(false))));\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .arg("named")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(
        text.contains("FAIL db > inserts > rejects a null"),
        "{text}"
    );
}

/// A skipped case is **in the report**, and one held back by somebody else's
/// `.only` is named apart from it. A green run that quietly ran fewer tests
/// than it printed is the failure this whole runner is arranged against.
#[test]
fn a_skipped_case_is_reported_rather_than_left_out() {
    let dir = build_dir("t_skip");
    write_in(
        &dir,
        "skip.test.mjs",
        r#"
import { test, describe } from "runtime:test";
test("runs", () => {});
test.skip("not this one", () => { throw new Error("MUST NOT RUN"); });
describe.skip("nor this group", () => {
  test("nor this", () => { throw new Error("MUST NOT RUN"); });
});
"#,
    );

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("1 passed, 0 failed, 2 skipped"), "{text}");

    // `only` runs one thing and says how much it did not run — a `.only` left
    // in a commit otherwise looks exactly like a suite that got faster. The
    // hooks the one case needs still run.
    write_in(
        &dir,
        "only.test.mjs",
        r#"
import { test, describe, beforeAll } from "runtime:test";
beforeAll(() => console.log("SETUP"));
test("one", () => { throw new Error("MUST NOT RUN"); });
describe("group", () => {
  test.only("the one being worked on", () => {});
  test("two", () => { throw new Error("MUST NOT RUN"); });
});
"#,
    );
    let out = esdev_in(&dir)
        .arg("test")
        .arg("only")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("SETUP"), "{text}");
    assert!(text.contains("only: 2 other tests did not run"), "{text}");
    assert!(text.contains("1 passed, 0 failed, 2 skipped"), "{text}");
}

/// A `describe` body registers and returns. An `async` one would register the
/// part before its first `await` and land the rest after the queue had already
/// drained, so it is refused rather than half-honoured.
#[test]
fn an_async_group_body_is_refused() {
    let dir = build_dir("t_async_group");
    write_in(
        &dir,
        "async.test.mjs",
        "import { test, describe } from 'runtime:test';\n         describe('slow', async () => { test('a', () => {}); });\n",
    );

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("it cannot be async"), "{text}");
}

/// The other half of running in a queue: a case that never got a turn is still
/// in the report, and says so in its own words rather than borrowing the
/// message of the test that is actually stuck.
#[test]
fn a_case_behind_one_that_hangs_says_it_never_started() {
    let dir = build_dir("t_queued");
    write_in(
        &dir,
        "queued.test.mjs",
        "import { test } from 'runtime:test';\n\
         test('hangs', () => new Promise(() => {}));\n\
         test('behind it', () => {});\n",
    );

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("FAIL hangs"), "{text}");
    assert!(text.contains("the test never finished"), "{text}");
    assert!(text.contains("FAIL behind it"), "{text}");
    assert!(text.contains("the test never started"), "{text}");
}

/// A test file is a module like any other, so running one directly is running
/// a module — no subcommand needed, and the same report either way.
#[test]
fn a_test_file_runs_on_its_own() {
    let dir = build_dir("t_direct");
    write_in(
        &dir,
        "direct.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         test('adds', () => assertEquals(2 + 3, 5));\n",
    );

    let out = esdev_in(&dir)
        .arg("direct.test.mjs")
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(
        stdout(&out).contains("1 passed, 0 failed"),
        "{}",
        stdout(&out)
    );
}

/// And `runtime:test` is `esdev`'s, like the other two: a test file is never a
/// production artifact.
#[test]
fn runtime_test_does_not_exist_under_esrun() {
    let Some(esrun) = sibling_binary("esrun") else {
        eprintln!("skipping: esrun is not built in this target dir");
        return;
    };
    let dir = build_dir("t_esrun");
    let app = write_in(&dir, "app.mjs", "import 'runtime:test';\n");

    let out = Command::new(esrun)
        .current_dir(app.parent().expect("fixture dir"))
        .arg(app.file_name().expect("entry"))
        .output()
        .expect("spawn esrun");
    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("unknown built-in module"),
        "{}",
        stderr(&out)
    );
}

/// One process per file: a file that exits must not take the run with it, and
/// the others must still be reported.
#[test]
fn a_file_that_exits_does_not_end_the_run() {
    let dir = build_dir("t_isolation");
    write_in(
        &dir,
        "a_exits.test.mjs",
        "import { test } from 'runtime:test';\n\
         import { exit } from 'runtime:process';\n\
         test('bails', () => exit(3));\n",
    );
    write_in(
        &dir,
        "b_fine.test.mjs",
        "import { test, assert } from 'runtime:test';\n\
         test('fine', () => assert(true));\n",
    );

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(text.contains("b_fine.test.mjs"), "{text}");
    assert!(text.contains("a_exits.test.mjs"), "{text}");
}

#[test]
fn a_filter_selects_by_path() {
    let dir = build_dir("t_filter");
    write_in(
        &dir,
        "alpha.test.mjs",
        "import { test, assert } from 'runtime:test';\ntest('a', () => assert(true));\n",
    );
    write_in(
        &dir,
        "beta.test.mjs",
        "import { test, assert } from 'runtime:test';\ntest('b', () => assert(true));\n",
    );

    let out = esdev_in(&dir)
        .args(["test", "alpha"])
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("alpha.test.mjs"), "{}", stdout(&out));
    assert!(!stdout(&out).contains("beta.test.mjs"), "{}", stdout(&out));
}

/// A rehearsal runs each file under the named grants: `--deny-all` takes
/// everything, and `--allow-*` grants back what the suite needs — the same
/// flags, resolved the same way, as a run. A path the suite covers this way
/// meets its deployment's capabilities before deployment.
#[test]
fn test_permission_flags_rehearse_the_production_grant() {
    let dir = build_dir("t_rehearse");
    write_in(
        &dir,
        "env.test.mjs",
        "import { test, expect } from \"runtime:test\";\n\
         import { env } from \"runtime:process\";\n\
         test(\"reads its grant\", () => { expect(env.GREETING).toBe(\"hi\"); });\n",
    );

    // The grant the suite needs: green.
    let ok = esdev_in(&dir)
        .args(["test", "--deny-all", "--allow-env=GREETING"])
        .env("GREETING", "hi")
        .output()
        .expect("spawn esdev test");
    assert!(ok.status.success(), "{}{}", stdout(&ok), stderr(&ok));

    // Without it: the file fails on the denied capability, not on its logic.
    // The failure detail is the child's harness report on stdout; the
    // parent's own stderr carries only the tally.
    let denied = esdev_in(&dir)
        .args(["test", "--deny-all"])
        .env("GREETING", "hi")
        .output()
        .expect("spawn esdev test");
    assert!(!denied.status.success());
    let denied_output = format!("{}{}", stdout(&denied), stderr(&denied));
    assert!(denied_output.contains("NotAllowedError"), "{denied_output}");

    // A grant without the denial it narrows is refused up front, not once per
    // file in every child.
    let bad = esdev_in(&dir)
        .args(["test", "--allow-env=GREETING"])
        .env("GREETING", "hi")
        .output()
        .expect("spawn esdev test");
    assert!(!bad.status.success());
    assert!(stderr(&bad).contains("--deny-all"), "{}", stderr(&bad));

    let _ = std::fs::remove_dir_all(&dir);
}

/// Filesystem access rehearses the same way: denied by default under
/// `--deny-all`, granted back narrowed to the directory the suite reads.
#[test]
fn test_deny_all_covers_filesystem_access() {
    let dir = build_dir("t_rehearse_fs");
    std::fs::create_dir_all(dir.join("data")).expect("create data");
    write_in(&dir, "data/msg.txt", "hello\n");
    write_in(
        &dir,
        "read.test.mjs",
        "import { test, expect } from \"runtime:test\";\n\
         import { file } from \"runtime:fs\";\n\
         test(\"reads its file\", async () => {\n\
         \x20 const text = await (await file(\"data/msg.txt\")).text();\n\
         \x20 expect(text).toBe(\"hello\\n\");\n\
         });\n",
    );

    let denied = esdev_in(&dir)
        .args(["test", "--deny-all"])
        .output()
        .expect("spawn esdev test");
    assert!(!denied.status.success());
    let denied_output = format!("{}{}", stdout(&denied), stderr(&denied));
    assert!(denied_output.contains("NotAllowedError"), "{denied_output}");

    let ok = esdev_in(&dir)
        .args(["test", "--deny-all", "--allow-read=./data"])
        .output()
        .expect("spawn esdev test");
    assert!(ok.status.success(), "{}{}", stdout(&ok), stderr(&ok));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_test_files_is_an_error_rather_than_a_silent_pass() {
    let dir = build_dir("t_empty");
    write_in(&dir, "notatest.mjs", "export const x = 1;\n");
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        !out.status.success(),
        "an empty run must not look like success"
    );
    let message = stderr(&out);
    assert!(message.contains("no test files"), "{message}");
    // Both discovery conventions, since 0.6.0 looks for either.
    assert!(message.contains("*.test."), "{message}");
    assert!(message.contains("*.spec."), "{message}");
}
