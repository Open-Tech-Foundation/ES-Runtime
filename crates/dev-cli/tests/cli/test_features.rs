//! `esdev test`: matchers, mocks, the clock, snapshots, fixtures, coverage and the run controls.

use super::*;

/// What `clock.freeze` replaces beyond timers — `Date()` as a function,
/// `performance.now`, `Temporal.Now`, `Intl` — and the choice of which.
#[test]
fn test_clock_fakes_what_it_is_asked_to_and_puts_it_back() {
    let dir = build_dir("t_clock_fakes");
    write_in(
        &dir,
        "clock.test.ts",
        r#"import { afterEach, expect, test, clock } from "runtime:test";

afterEach(() => clock.release());

test("Date called without new is a string of the frozen time", () => {
  clock.freeze(new Date("2020-01-01T00:00:00Z"));
  expect(Date()).toBe(new Date(1577836800000).toString());
  expect(new Date()).toBeInstanceOf(Date);
  expect(new Date().getTime()).toBe(1577836800000);
  expect(Date.UTC(2020, 0, 1)).toBe(1577836800000);
});
test("performance.now moves with the clock", () => {
  clock.freeze();
  const before = performance.now();
  clock.advance(250);
  expect(performance.now() - before).toBe(250);
});
test("Temporal.Now reads the frozen clock", () => {
  clock.freeze("2021-06-01T12:00:00Z");
  expect(Temporal.Now.instant().epochMilliseconds).toBe(Date.parse("2021-06-01T12:00:00Z"));
  expect(Temporal.Now.plainDateISO("UTC").toString()).toBe("2021-06-01");
  clock.advance(60_000);
  expect(Temporal.Now.zonedDateTimeISO("UTC").minute).toBe(1);
});
test("Intl formats the frozen now", () => {
  clock.freeze("2021-06-01T12:00:00Z");
  const f = new Intl.DateTimeFormat("en-US", { timeZone: "UTC", year: "numeric" });
  expect(f.format()).toBe("2021");
  expect(Intl.DateTimeFormat("en-US", { timeZone: "UTC", month: "long" }).formatToParts()[0].value).toBe("June");
  expect(f).toBeInstanceOf(Intl.DateTimeFormat);
});
test("toFake picks what is replaced", () => {
  const realSetTimeout = setTimeout;
  clock.freeze({ now: 0, toFake: ["Date"] });
  expect(Date.now()).toBe(0);
  expect(setTimeout).toBe(realSetTimeout);
});
test("toNotFake keeps Date real", () => {
  clock.freeze({ now: 0, toNotFake: ["Date"] });
  expect(Date.now()).toBeGreaterThan(0);
  let ran = false;
  setTimeout(() => (ran = true), 5);
  clock.advance(5);
  expect(ran).toBe(true);
});
test("queueMicrotask only when asked, then run by runMicrotasks", () => {
  clock.freeze({ toFake: ["queueMicrotask"] });
  const seen: number[] = [];
  queueMicrotask(() => { seen.push(1); queueMicrotask(() => seen.push(2)); });
  expect(seen).toEqual([]);
  clock.runMicrotasks();
  expect(seen).toEqual([1, 2]);
});
test("a timer's microtasks run with it", () => {
  clock.freeze({ toFake: ["setTimeout", "queueMicrotask"] });
  const seen: string[] = [];
  setTimeout(() => { queueMicrotask(() => seen.push("job")); seen.push("timer"); }, 1);
  clock.advance(1);
  expect(seen).toEqual(["timer", "job"]);
});
test("bad names and both lists are refused", () => {
  expect(() => clock.freeze({ toFake: ["nextTick" as any] })).toThrow('cannot fake "nextTick"');
  expect(() => clock.freeze({ toFake: ["Date"], toNotFake: ["Date"] })).toThrow("not both");
  expect(() => clock.freeze({ loopLimit: 0 })).toThrow("loopLimit");
  expect(clock.isFrozen()).toBe(false);
});
test("loopLimit", () => {
  clock.freeze({ loopLimit: 5 });
  setInterval(() => {}, 1);
  expect(() => clock.runAll()).toThrow("clock.runAll: 5 timers fired");
});
test("an unparseable time is refused", () => {
  clock.freeze();
  expect(() => clock.setSystemTime("not a date")).toThrow("is not a time");
});
test("release puts everything back", () => {
  const before = [setTimeout, Date, performance.now, Temporal.Now.instant, Intl.DateTimeFormat];
  clock.freeze();
  clock.release();
  expect([setTimeout, Date, performance.now, Temporal.Now.instant, Intl.DateTimeFormat]).toEqual(before);
  expect(Object.hasOwn(performance, "now")).toBe(false);
});
"#,
    );
    let out = esdev_in(&dir)
        .args(["test"])
        .output()
        .expect("spawn esdev test");
    let text = stdout(&out);
    assert!(out.status.success(), "{text}{}", stderr(&out));
    assert!(text.contains("12 passed, 0 failed"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_clock_advances_to_the_next_animation_frame() {
    let dir = build_dir("t_clock_frames");
    write_in(
        &dir,
        "frame.test.js",
        r#"import { afterEach, expect, test, clock } from "runtime:test";
afterEach(() => clock.release());
test("frames fall every 16ms", () => {
  clock.freeze();
  const start = performance.now();
  const stamps = [];
  requestAnimationFrame((t) => stamps.push(t - start));
  clock.advance(5);
  requestAnimationFrame((t) => stamps.push(t - start));
  expect(stamps).toEqual([]);
  clock.advanceToNextFrame();
  expect(stamps).toEqual([16, 16]);
  const id = requestAnimationFrame(() => stamps.push("no"));
  cancelAnimationFrame(id);
  clock.advanceToNextFrame();
  expect(stamps).toEqual([16, 16]);
  expect(Date.now() - clock.realNow()).toBeLessThanOrEqual(32 + 50);
});
"#,
    );
    let out = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom");
    let text = stdout(&out);
    assert!(out.status.success(), "{text}{}", stderr(&out));
    assert!(text.contains("1 passed, 0 failed"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Call order, resolved values, exactly-once calls, `toBeNullable`,
/// `expect.fail`, and the `arrayOf` and `schemaMatching` asymmetric matchers.
#[test]
fn test_matchers_for_call_order_resolved_values_and_schemas() {
    let dir = build_dir("t_matchers_more");
    write_in(
        &dir,
        "m.test.ts",
        r#"import { expect, mock, test } from "runtime:test";

test("call order", () => {
  const a = mock.fn().mockName("a"), b = mock.fn().mockName("b"), never = mock.fn();
  a(); b(); a();
  expect(a).toHaveBeenCalledBefore(b);
  expect(b).toHaveBeenCalledAfter(a);
  expect(b).not.toHaveBeenCalledBefore(a);
  expect(never).not.toHaveBeenCalledBefore(a);
  expect(never).toHaveBeenCalledBefore(a, false);
  expect(a).not.toHaveBeenCalledBefore(never);
  expect(() => expect(b).toHaveBeenCalledBefore(a)).toThrow("expected b to have been called before a");
  expect(() => expect(a).toHaveBeenCalledBefore(() => {})).toThrow("needs another mock");
  expect(a.mock.invocationCallOrder[0]).toBeLessThan(b.mock.invocationCallOrder[0]);
});
test("exactly once with", () => {
  const f = mock.fn();
  f("apples", 10);
  expect(f).toHaveBeenCalledExactlyOnceWith("apples", 10);
  f("apples", 10);
  expect(f).not.toHaveBeenCalledExactlyOnceWith("apples", 10);
});
test("contexts", () => {
  const f = mock.fn();
  const o = { f };
  o.f();
  expect(f.mock.contexts).toEqual([o]);
});
test("resolved", async () => {
  const sell = mock.fn((p: string) => (p === "x" ? Promise.reject(new Error("no")) : Promise.resolve({ p })));
  const pending = sell("apples");
  expect(sell).not.toHaveResolved();
  await pending;
  await sell("bananas");
  await sell("x").catch(() => {});
  expect(sell).toHaveResolved();
  expect(sell).toHaveResolvedTimes(2);
  expect(sell).toHaveResolvedWith({ p: "apples" });
  expect(sell).not.toHaveLastResolvedWith({ p: "bananas" });
  expect(sell).toHaveNthResolvedWith(2, { p: "bananas" });
  expect(sell.mock.settledResults.map((r) => r.type)).toEqual(["fulfilled", "fulfilled", "rejected"]);
  const sync = mock.fn(() => 1);
  sync();
  expect(sync).toHaveResolvedWith(1);
});
test("nullable, fail, arrayOf", () => {
  expect(null).toBeNullable();
  expect(undefined).toBeNullable();
  expect(0).not.toBeNullable();
  expect(() => expect(0).toBeNullable()).toThrow("expected 0 to be null or undefined");
  expect(() => expect.fail("boom")).toThrow("boom");
  expect(["a", "b"]).toEqual(expect.arrayOf(expect.any(String)));
  expect([{ id: 1 }]).toEqual(expect.arrayOf(expect.objectContaining({ id: expect.any(Number) })));
  expect(["a", 1]).toEqual(expect.not.arrayOf(expect.any(String)));
  expect("a").not.toEqual(expect.arrayOf("a"));
});
test("schemaMatching", () => {
  const email = { "~standard": { version: 1, vendor: "mini", validate: (v: unknown) => (typeof v === "string" && v.includes("@") ? { value: v } : { issues: [{ message: "not an email" }] }) } };
  expect({ email: "a@b.c" }).toEqual({ email: expect.schemaMatching(email) });
  expect({ email: "nope" }).not.toEqual({ email: expect.schemaMatching(email) });
  expect("nope").toEqual(expect.not.schemaMatching(email));
  const slow = { "~standard": { version: 1, vendor: "slow", validate: async () => ({ value: 1 }) } };
  expect(() => expect(1).toEqual(expect.schemaMatching(slow))).toThrow("validates asynchronously");
  expect(() => expect.schemaMatching({})).toThrow("needs a Standard Schema");
});
"#,
    );
    let out = esdev_in(&dir)
        .args(["test"])
        .output()
        .expect("spawn esdev test");
    let text = stdout(&out);
    assert!(out.status.success(), "{text}{}", stderr(&out));
    assert!(text.contains("6 passed, 0 failed"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A failure message prints two values the comparison tells apart differently:
/// a Set is not printed as an array, nor an Error as `{}`, nor a Date as its
/// string. Plain data prints as JSON does, as it always has.
#[test]
fn test_failure_messages_keep_the_types_they_compare() {
    let dir = build_dir("t_failure_types");
    write_in(
        &dir,
        "m.test.ts",
        r#"import { expect, test } from "runtime:test";

function message(run: () => void): string {
  try { run(); } catch (error) { return (error as Error).message; }
  throw new Error("the assertion passed");
}
class Point { constructor(public x: number) {} }

test("distinct types print distinctly", () => {
  expect(message(() => expect(new Set([1])).toEqual([1]))).toBe("expected Set {1} to equal [1]");
  expect(message(() => expect(new Map([["a", 1]])).toEqual({ a: 1 })))
    .toBe('expected Map {"a" => 1} to equal {"a":1}');
  expect(message(() => expect(new Date(0)).toEqual("1970-01-01T00:00:00.000Z")))
    .toBe('expected Date("1970-01-01T00:00:00.000Z") to equal "1970-01-01T00:00:00.000Z"');
  expect(message(() => expect(new Error("a")).toEqual(new Error("b"))))
    .toBe('expected Error("a") to equal Error("b")');
  expect(message(() => expect(new Uint8Array([1, 2])).toEqual([1, 2])))
    .toBe("expected Uint8Array [1,2] to equal [1,2]");
  expect(message(() => expect(new Point(1)).toStrictEqual({ x: 1 })))
    .toBe('expected Point {"x":1} to strictly equal {"x":1}');
});

test("what JSON drops is kept", () => {
  expect(message(() => expect({ a: undefined }).toStrictEqual({})))
    .toBe('expected {"a":undefined} to strictly equal {}');
  expect(message(() => expect([NaN]).toEqual([null]))).toBe("expected [NaN] to equal [null]");
  expect(message(() => expect(-0).toBe(0))).toBe("expected -0 to be 0");
  expect(message(() => expect(1n).toBe(2n))).toBe("expected 1n to be 2n");
  const cycle: Record<string, unknown> = { name: "c" };
  cycle.self = cycle;
  expect(message(() => expect(cycle).toBe({}))).toBe('expected {"name":"c","self":[Circular]} to be {}');
  const shared = { n: 1 };
  expect(message(() => expect([shared, shared]).toBe([])))
    .toBe('expected [{"n":1},{"n":1}] to be []');
  function named() {}
  expect(message(() => expect(named).toBe(1))).toBe("expected [Function named] to be 1");
});

test("plain data prints as JSON", () => {
  expect(message(() => expect({ a: 1, b: [1, "x", null, true] }).toEqual({})))
    .toBe('expected {"a":1,"b":[1,"x",null,true]} to equal {}');
});
"#,
    );
    let out = esdev_in(&dir)
        .args(["test"])
        .output()
        .expect("spawn esdev test");
    let text = stdout(&out);
    assert!(out.status.success(), "{text}{}", stderr(&out));
    assert!(text.contains("3 passed, 0 failed"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Equality testers apply at every depth and in every deep comparison, and a
/// serializer prints a class instance a snapshot otherwise refuses.
#[test]
fn test_equality_testers_and_snapshot_serializers_extend_expect() {
    let dir = build_dir("t_expect_extend_utils");
    write_in(
        &dir,
        "x.test.ts",
        r#"import { expect, mock, test } from "runtime:test";

class Volume {
  constructor(public amount: number, public unit: "L" | "mL") {}
  ml() { return this.unit === "L" ? this.amount * 1000 : this.amount; }
}
class Shelf { constructor(public name: string, public volumes: Volume[]) {} }

function areVolumesEqual(a: unknown, b: unknown) {
  const isA = a instanceof Volume, isB = b instanceof Volume;
  if (isA && isB) return a.ml() === b.ml();
  if (isA === isB) return undefined;
  return false;
}
function areShelvesEqual(this: { equals(a: unknown, b: unknown): boolean }, a: unknown, b: unknown) {
  if (a instanceof Shelf && b instanceof Shelf) return a.name === b.name && this.equals(a.volumes, b.volumes);
  return undefined;
}
expect.addEqualityTesters([areVolumesEqual, areShelvesEqual]);

test("testers decide their pairs, at any depth", () => {
  expect(new Volume(1, "L")).toEqual(new Volume(1000, "mL"));
  expect({ v: [new Volume(2, "L")] }).toEqual({ v: [new Volume(2000, "mL")] });
  expect(new Shelf("a", [new Volume(1, "L")])).toEqual(new Shelf("a", [new Volume(1000, "mL")]));
  expect(new Volume(1, "L")).not.toEqual(new Volume(1, "mL"));
  const f = mock.fn();
  f(new Volume(1, "L"));
  expect(f).toHaveBeenCalledWith(new Volume(1000, "mL"));
  expect([new Volume(1, "L")]).toContainEqual(new Volume(1000, "mL"));
});

expect.addSnapshotSerializer({
  test: (v) => v instanceof Volume,
  serialize: (v: Volume) => `Volume<${v.ml()}mL>`,
});
expect.addSnapshotSerializer({
  test: (v) => v instanceof Shelf,
  serialize: (v: Shelf, config: any, indentation: string, depth: number, refs: any, printer: any) =>
    `Shelf ${v.name} ${printer(v.volumes, config, indentation, depth, refs)}`,
});
test("serializers print what they take", () => {
  expect(new Volume(1, "L")).toMatchInlineSnapshot(`Volume<1000mL>`);
  expect({ shelf: new Shelf("top", [new Volume(5, "mL")]) }).toMatchInlineSnapshot(`
    {
      "shelf": Shelf top [
        Volume<5mL>,
      ],
    }
  `);
});
test("a class with no serializer names the fix", () => {
  class Other {}
  expect(() => expect(new Other()).toMatchInlineSnapshot(`x`)).toThrow("expect.addSnapshotSerializer can print one");
});
test("bad arguments", () => {
  expect(() => expect.addEqualityTesters([1 as any])).toThrow("an array of functions");
  expect(() => expect.addSnapshotSerializer({} as any)).toThrow("needs { test(value)");
});
"#,
    );
    let out = esdev_in(&dir)
        .args(["test"])
        .output()
        .expect("spawn esdev test");
    let text = stdout(&out);
    assert!(out.status.success(), "{text}{}", stderr(&out));
    assert!(text.contains("4 passed, 0 failed"), "{text}");
    assert!(
        text.contains("snapshots: 2 matched, 0 failed, 0 written"),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_to_strict_equal_checks_undefined_keys_holes_and_types() {
    let dir = build_dir("t_strict_equal");
    write_in(
        &dir,
        "s.test.ts",
        r#"import { expect, test } from "runtime:test";
class Stock { constructor(public type: string) {} }
test("strict", () => {
  expect(new Stock("apples")).toEqual({ type: "apples" });
  expect(new Stock("apples")).not.toStrictEqual({ type: "apples" });
  expect(new Stock("apples")).toStrictEqual(new Stock("apples"));
  expect({ a: undefined, b: 2 }).toEqual({ b: 2 });
  expect({ a: undefined, b: 2 }).not.toStrictEqual({ b: 2 });
  // eslint-disable-next-line no-sparse-arrays
  expect([, 1]).not.toStrictEqual([undefined, 1]);
  expect([undefined, 1]).toStrictEqual([undefined, 1]);
  expect({ deep: [{ x: undefined }] }).not.toStrictEqual({ deep: [{}] });
  expect(new Map([[1, { a: undefined }]])).not.toStrictEqual(new Map([[1, {}]]));
  expect({ n: 1, when: expect.any(Number) }).toStrictEqual({ n: 1, when: 5 });
  expect(() => expect({ a: undefined }).toStrictEqual({})).toThrow("to strictly equal");
});
"#,
    );
    let out = esdev_in(&dir)
        .args(["test"])
        .output()
        .expect("spawn esdev test");
    let text = stdout(&out);
    assert!(out.status.success(), "{text}{}", stderr(&out));
    assert!(text.contains("1 passed, 0 failed"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--shard` splits the discovered files between runs: every file in exactly
/// one shard, a shard with none still passing with its report, and the
/// combinations that have no meaning refused.
#[test]
fn test_shard_splits_the_files_across_runs() {
    let dir = build_dir("t_shard");
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::create_dir_all(dir.join("b")).unwrap();
    let body = "import { test } from \"runtime:test\";\ntest(\"t\", () => {});\n";
    let names = [
        "a/one.test.js",
        "a/two.test.js",
        "a/three.test.js",
        "b/four.test.js",
        "b/five.test.js",
    ];
    for name in names {
        write_in(&dir, name, body);
    }
    let mut ran: Vec<&&str> = Vec::new();
    for index in 1..=3 {
        let out = esdev_in(&dir)
            .args(["test", &format!("--shard={index}/3")])
            .output()
            .expect("spawn esdev test --shard");
        let (text, err) = (slash_paths(&stdout(&out)), stderr(&out));
        assert!(out.status.success(), "{text}{err}");
        let mine: Vec<_> = names.iter().filter(|name| text.contains(*name)).collect();
        assert!(
            err.contains(&format!("shard {index}/3: {} of 5 files", mine.len())),
            "{err}"
        );
        // Five over three: two, two, one.
        assert_eq!(mine.len(), if index == 3 { 1 } else { 2 }, "{text}");
        ran.extend(mine);
    }
    ran.sort();
    let mut all: Vec<_> = names.iter().collect();
    all.sort();
    assert_eq!(ran, all, "each file runs in exactly one shard");

    // More shards than files: an empty shard passes, and writes its report.
    let out = esdev_in(&dir)
        .args([
            "test",
            "--shard=6/6",
            "--reporter=junit",
            "--reporter-outfile=report.xml",
        ])
        .output()
        .expect("spawn esdev test --shard=6/6");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(
        stderr(&out).contains("shard 6/6: 0 of 5 files"),
        "{}",
        stderr(&out)
    );
    let xml = std::fs::read_to_string(dir.join("report.xml")).expect("report written");
    assert!(
        xml.contains("tests=\"0\"") && xml.contains("time=\"0.000\""),
        "{xml}"
    );

    for (args, says) in [
        (vec!["--shard=0/3"], "--shard=0/3 is not a shard"),
        (vec!["--shard=4/3"], "counting from 1"),
        (vec!["--shard=2"], "Write <index>/<count>"),
        (
            vec!["--shard=1/2", "--watch"],
            "--watch re-runs it on this one",
        ),
        (
            vec!["--shard=1/2", "--file=a/one.test.js"],
            "nothing to shard",
        ),
    ] {
        let out = esdev_in(&dir)
            .arg("test")
            .args(&args)
            .output()
            .expect("spawn esdev test");
        assert!(!out.status.success(), "{args:?}");
        assert!(stderr(&out).contains(says), "{args:?}: {}", stderr(&out));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A project whose global setup is two modules, each noting its teardown in
/// `teardown-order.txt`, and two test files that inject what they provided.
fn global_setup_project(name: &str) -> PathBuf {
    let dir = build_dir(name);
    std::fs::create_dir_all(dir.join("test")).unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    write_in(
        &dir,
        "esdev.json",
        r#"{ "test": { "globalSetup": ["./test/first.ts", "./test/second.ts"] } }"#,
    );
    write_in(
        &dir,
        "test/first.ts",
        r#"import { file, write } from "runtime:fs";
const note = async (line: string) => { const f = file("teardown-order.txt"); const before = (await f.exists()) ? await f.text() : ""; await write("teardown-order.txt", before + line); };
export async function setup({ provide }: { provide(k: string, v: unknown): void }) {
  console.log("first setup");
  provide("port", 4321);
  provide("db", { name: "test", tables: ["a"] });
}
export async function teardown() {
  console.log("first teardown");
  await note("first\n");
}
"#,
    );
    write_in(
        &dir,
        "test/second.ts",
        r#"import { file, write } from "runtime:fs";
const note = async (line: string) => { const f = file("teardown-order.txt"); const before = (await f.exists()) ? await f.text() : ""; await write("teardown-order.txt", before + line); };
export default function (context: { provide(k: string, v: unknown): void }) {
  console.log("second setup");
  context.provide("second", true);
  return async () => {
    console.log("second teardown");
    await note("second\n");
  };
}
"#,
    );
    for file in ["src/a.test.ts", "src/b.test.ts"] {
        write_in(
            &dir,
            file,
            r#"import { expect, inject, test } from "runtime:test";
test("sees what global setup provided", () => {
  expect(inject("port")).toBe(4321);
  expect(inject("db")).toEqual({ name: "test", tables: ["a"] });
  expect(inject("second")).toBe(true);
  expect(inject("missing")).toBeUndefined();
});
"#,
        );
    }
    dir
}

/// Global setup runs once, in order, before any file; every file injects what
/// it provided; teardown runs after the last, in reverse — in each way a run
/// can be made.
#[test]
fn test_global_setup_runs_once_around_the_files_and_provides_to_them() {
    let dir = global_setup_project("t_global_setup");
    let order = || {
        let text = std::fs::read_to_string(dir.join("teardown-order.txt")).unwrap_or_default();
        let _ = std::fs::remove_file(dir.join("teardown-order.txt"));
        text
    };

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = stdout(&out);
    assert!(out.status.success(), "{text}{}", stderr(&out));
    assert_eq!(text.matches("first setup").count(), 1, "{text}");
    assert!(text.find("second setup") < text.find("1 passed"), "{text}");
    assert!(text.contains("2 files passed"), "{text}");
    assert_eq!(order(), "second\nfirst\n");

    // What the setup prints stays off a machine reporter's stdout.
    let out = esdev_in(&dir)
        .args(["test", "--reporter=json"])
        .output()
        .expect("spawn esdev test --reporter=json");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!stdout(&out).contains("first setup"), "{}", stdout(&out));
    assert!(stderr(&out).contains("first setup"), "{}", stderr(&out));
    assert_eq!(order(), "second\nfirst\n");

    let out = esdev_in(&dir)
        .args(["test", "--isolation=none"])
        .output()
        .expect("spawn esdev test --isolation=none");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert_eq!(order(), "second\nfirst\n");

    // A file run on its own gets the setup its suite would.
    let out = esdev_in(&dir)
        .args(["test", "--file=src/a.test.ts"])
        .output()
        .expect("spawn esdev test --file");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert_eq!(order(), "second\nfirst\n");

    // Listing runs no test, so nothing needs setting up.
    let out = esdev_in(&dir)
        .args(["test", "--list"])
        .output()
        .expect("spawn esdev test --list");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!stdout(&out).contains("first setup"), "{}", stdout(&out));
    assert_eq!(order(), "");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A setup that throws runs no test and still tears down what came before it;
/// a teardown that throws fails a run whose tests passed; a value that is not
/// JSON is refused where it is provided.
#[test]
fn test_global_setup_failures_are_the_runs_failures() {
    let dir = global_setup_project("t_global_setup_fails");
    write_in(
        &dir,
        "test/port.ts",
        "export function setup() { throw new Error(\"port in use\"); }\n",
    );
    write_in(
        &dir,
        "test/stop.ts",
        "export function teardown() { throw new Error(\"could not stop\"); }\n",
    );
    write_in(
        &dir,
        "test/fn.ts",
        "export function setup({ provide }) { provide(\"f\", () => 1); }\n",
    );

    let out = esdev_in(&dir)
        .args([
            "test",
            "--global-setup=test/first.ts",
            "--global-setup=test/port.ts",
        ])
        .output()
        .expect("spawn esdev test");
    let (text, err) = (stdout(&out), stderr(&out));
    assert!(!out.status.success());
    assert!(err.contains("Error: port in use"), "{err}");
    assert!(err.contains("global setup failed"), "{err}");
    assert!(err.contains("so no test ran"), "{err}");
    assert!(!text.contains("passed"), "{text}");
    assert_eq!(
        std::fs::read_to_string(dir.join("teardown-order.txt")).unwrap_or_default(),
        "first\n"
    );
    let _ = std::fs::remove_file(dir.join("teardown-order.txt"));

    let out = esdev_in(&dir)
        .args([
            "test",
            "--global-setup=test/first.ts",
            "--global-setup=test/second.ts",
            "--global-setup=test/stop.ts",
        ])
        .output()
        .expect("spawn esdev test");
    assert!(
        !out.status.success(),
        "a teardown that threw passed the run"
    );
    assert!(stdout(&out).contains("2 files passed"), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("Error: could not stop"),
        "{}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("global teardown failed"),
        "{}",
        stderr(&out)
    );
    // The other teardowns still ran.
    assert_eq!(
        std::fs::read_to_string(dir.join("teardown-order.txt")).unwrap_or_default(),
        "second\nfirst\n"
    );

    let out = esdev_in(&dir)
        .args(["test", "--global-setup=test/fn.ts"])
        .output()
        .expect("spawn esdev test");
    assert!(!out.status.success());
    assert!(
        stderr(&out)
            .contains("TypeError: provide(\"f\"): a test file receives JSON, and function is not"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A watched run sets up once, and tears down when ^C ends the session — the
/// interrupt reaches esdev, which ends the setup's process itself.
#[cfg(unix)]
#[test]
fn test_global_setup_tears_down_when_a_watch_is_interrupted() {
    use std::os::unix::process::CommandExt;
    let dir = global_setup_project("t_global_setup_watch");
    let log = dir.join("watch.log");
    let mut command = esdev_in(&dir);
    command
        .args(["test", "--watch"])
        .stdout(std::fs::File::create(&log).unwrap())
        .stderr(std::fs::File::create(dir.join("watch.err")).unwrap())
        .process_group(0);
    let mut child = command.spawn().expect("spawn esdev test --watch");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !std::fs::read_to_string(dir.join("watch.err"))
        .unwrap_or_default()
        .contains("watching for changes")
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the watch never finished its first run"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // As a terminal delivers ^C: to the whole foreground group. Sent here
    // rather than through a `kill` binary, whose spelling of a negative group
    // varies between implementations: a ^C that never arrives leaves the
    // watch waiting, and the wait below is what keeps that from hanging CI.
    let group =
        rustix::process::Pid::from_raw(i32::try_from(child.id()).unwrap()).expect("a child pid");
    rustix::process::kill_process_group(group, rustix::process::Signal::INT)
        .expect("send ^C to the watch's group");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait for the watch") {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "the watch did not end within 5s of ^C.\nstderr:\n{}\nstdout:\n{}",
                std::fs::read_to_string(dir.join("watch.err")).unwrap_or_default(),
                std::fs::read_to_string(&log).unwrap_or_default()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "{status}");
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(text.matches("first setup").count(), 1, "{text}");
    assert_eq!(
        std::fs::read_to_string(dir.join("teardown-order.txt")).unwrap_or_default(),
        "second\nfirst\n",
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A git repository with two test files: `cart.test.ts` reaching
/// `price.ts` through `cart.ts`, and `other.test.ts` reaching `other.ts`. `None`
/// where git is not installed.
fn changed_project(name: &str) -> Option<PathBuf> {
    let dir = build_dir(name);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    write_in(
        &dir,
        "src/price.ts",
        "export const price = (n: number) => n * 2;\n",
    );
    write_in(
        &dir,
        "src/cart.ts",
        "import { price } from \"./price\";\nexport const cart = (xs: number[]) => xs.map(price);\n",
    );
    write_in(&dir, "src/other.ts", "export const other = 1;\n");
    write_in(
        &dir,
        "src/cart.test.ts",
        "import { expect, test } from \"runtime:test\";\nimport { cart } from \"./cart\";\n\
         test(\"cart\", () => expect(cart([1])).toEqual([2]));\n",
    );
    write_in(
        &dir,
        "src/other.test.ts",
        "import { expect, test } from \"runtime:test\";\nimport { other } from \"./other.ts\";\n\
         test(\"other\", () => expect(other).toBe(1));\n",
    );
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .is_ok_and(|out| out.status.success())
    };
    if !(git(&["init", "-q", "."]) && git(&["add", "-A"]) && git(&["commit", "-qm", "init"])) {
        eprintln!("git is not available; the --changed run did not happen");
        return None;
    }
    Some(dir)
}

/// `--changed` runs the test files that reach what git says changed, and
/// `--related` those that reach the files named.
#[test]
fn test_changed_and_related_run_the_files_a_change_reaches() {
    let Some(dir) = changed_project("t_changed") else {
        return;
    };
    let run = |args: &[&str]| {
        let out = esdev_in(&dir)
            .arg("test")
            .args(args)
            .output()
            .expect("spawn esdev test");
        assert!(
            out.status.success(),
            "{args:?}: {}{}",
            stdout(&out),
            stderr(&out)
        );
        (slash_paths(&stdout(&out)), stderr(&out))
    };

    // Nothing changed: nothing to run, and that is a pass.
    let (text, err) = run(&["--changed"]);
    assert!(
        err.contains("changed: 0 files uncommitted; 0 of 2 test files reach them"),
        "{err}"
    );
    assert!(!text.contains(".test.ts"), "{text}");

    // Two imports away.
    write_in(
        &dir,
        "src/price.ts",
        "export const price = (n: number) => n + n;\n",
    );
    let (text, err) = run(&["--changed"]);
    assert!(
        err.contains("changed: 1 file uncommitted; 1 of 2 test files reach it"),
        "{err}"
    );
    assert!(
        text.contains("src/cart.test.ts") && !text.contains("src/other.test.ts"),
        "{text}"
    );

    // A new, untracked test file is a change too.
    write_in(
        &dir,
        "src/new.test.ts",
        "import { test } from \"runtime:test\";\ntest(\"new\", () => {});\n",
    );
    let (text, _) = run(&["--changed"]);
    assert!(text.contains("src/new.test.ts"), "{text}");
    std::fs::remove_file(dir.join("src/new.test.ts")).unwrap();

    // Since a commit: what it and the working tree changed.
    Command::new("git")
        .args(["commit", "-qam", "edit"])
        .current_dir(&dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .status()
        .expect("git commit");
    let (text, err) = run(&["--changed=HEAD~1"]);
    assert!(err.contains("changed: 1 file since HEAD~1"), "{err}");
    assert!(
        text.contains("src/cart.test.ts") && !text.contains("src/other.test.ts"),
        "{text}"
    );

    // What decides every test's dependencies reaches every test.
    write_in(&dir, "package.json", "{}\n");
    let (text, _) = run(&["--changed"]);
    assert!(text.contains("2 files passed"), "{text}");
    std::fs::remove_file(dir.join("package.json")).unwrap();

    let (text, err) = run(&["--related", "src/other.ts"]);
    assert!(
        err.contains("related: 1 file named; 1 of 2 test files reach it"),
        "{err}"
    );
    assert!(
        text.contains("src/other.test.ts") && !text.contains("src/cart.test.ts"),
        "{text}"
    );

    for (args, says) in [
        (vec!["--related"], "none were"),
        (vec!["--changed", "--related", "src/a.ts"], "give one"),
        (
            vec!["--changed", "--watch"],
            "--watch has no such selection",
        ),
        (vec!["--changed=no-such-branch"], "failed"),
    ] {
        let out = esdev_in(&dir)
            .arg("test")
            .args(&args)
            .output()
            .expect("spawn esdev test");
        assert!(!out.status.success(), "{args:?}");
        assert!(stderr(&out).contains(says), "{args:?}: {}", stderr(&out));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A project to measure: TypeScript the transform reprints, JavaScript it runs
/// as written, a file no test loads, and code the tests do not reach.
fn coverage_project(name: &str) -> PathBuf {
    let dir = build_dir(name);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    write_in(
        &dir,
        "src/math.ts",
        "export function add(a: number, b: number): number {\n  return a + b;\n}\n\
         export function sign(n: number): string {\n  if (n > 0) {\n    return \"positive\";\n  } else if (n < 0) {\n    return \"negative\";\n  }\n  return \"zero\";\n}\n\
         export const never = (x: number) => x * 2;\n",
    );
    write_in(
        &dir,
        "src/labels.js",
        "export function label(n) {\n  return n > 10 ? \"big\" : \"small\";\n}\n\
         export const pick = (a, b) => a || b;\n\
         /* v8 ignore next */\nexport function debugOnly() { return 1; }\n",
    );
    write_in(
        &dir,
        "src/untested.ts",
        "export const untouched = () => 1;\n",
    );
    write_in(
        &dir,
        "src/math.test.ts",
        "import { expect, test } from \"runtime:test\";\nimport { add, sign } from \"./math\";\nimport { label, pick } from \"./labels.js\";\n\
         test(\"math\", () => { expect(add(1, 2)).toBe(3); expect(sign(5)).toBe(\"positive\"); });\n\
         test(\"labels\", () => { expect(label(3)).toBe(\"small\"); expect(pick(1, 2)).toBe(1); });\n",
    );
    dir
}

/// Whether this esdev can collect coverage; when it cannot, that it says so.
fn coverage_available(out: &Output) -> bool {
    let err = stderr(out);
    if err.contains("this build has no inspector") {
        // Refused when asked for with `--coverage`; gone without, with a note,
        // when the project turns it on.
        assert!(err.contains("ES_RUNTIME_INSPECTOR=1"), "{err}");
        eprintln!("this esdev has no inspector; the coverage run did not happen");
        return false;
    }
    true
}

/// Statements, branches, functions and lines of the files as written — the
/// TypeScript through its reprint — in the text table and in lcov.
#[test]
fn test_coverage_measures_the_files_as_written() {
    let dir = coverage_project("t_coverage");
    let out = esdev_in(&dir)
        .args(["test", "--coverage"])
        .output()
        .expect("spawn esdev test --coverage");
    if !coverage_available(&out) {
        return;
    }
    let text = stdout(&out);
    assert!(out.status.success(), "{text}{}", stderr(&out));
    let row = |file: &str| {
        text.lines()
            .find(|line| line.starts_with(&format!(" {file} ")))
            .map(|line| line.split('|').map(str::trim).collect::<Vec<_>>())
            .unwrap_or_else(|| panic!("no row for {file}:\n{text}"))
    };
    // add, sign's first branch and return; the else-if, its return and the
    // last return not reached; `never` not called.
    assert_eq!(
        row("src/math.ts"),
        ["src/math.ts", "50", "25", "66.66", "57.14", "7-8,10"]
    );
    // The ignored function is not counted at all.
    assert_eq!(
        row("src/labels.js"),
        ["src/labels.js", "100", "50", "100", "100", ""]
    );
    // Loaded by no test, and not asked for: not reported.
    assert!(!text.contains(" src/untested.ts "), "{text}");
    // Nor are the tests themselves.
    assert!(!text.contains(" src/math.test.ts "), "{text}");

    let lcov = std::fs::read_to_string(dir.join("coverage/lcov.info")).expect("lcov.info written");
    for line in [
        "SF:src/math.ts",
        "FNDA:1,add",
        "FNDA:0,never",
        "DA:8,0",
        "BRDA:5,0,0,1",
        "end_of_record",
    ] {
        assert!(lcov.lines().any(|l| l == line), "{line} in\n{lcov}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `test.coverage` in esdev.json: on without the flag, what to include and
/// exclude, the reporters, and thresholds that fail a passing run.
#[test]
fn test_coverage_settings_come_from_esdev_json() {
    let dir = coverage_project("t_coverage_settings");
    write_in(
        &dir,
        "esdev.json",
        r#"{ "test": { "coverage": {
            "enabled": true,
            "include": ["src"],
            "exclude": ["src/labels.js"],
            "reporter": ["text", "json", "json-summary"],
            "reportsDirectory": "reports/coverage",
            "thresholds": { "lines": 90, "functions": -1 }
        } } }"#,
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    if !coverage_available(&out) {
        return;
    }
    let (text, err) = (stdout(&out), stderr(&out));
    assert!(
        !out.status.success(),
        "thresholds not met, and the run passed:\n{text}{err}"
    );
    assert!(
        text.contains("2 files passed") || text.contains("1 file passed"),
        "{text}"
    );
    // Included though no test loads it, at nothing covered.
    assert!(text.contains(" src/untested.ts | "), "{text}");
    assert!(!text.contains("src/labels.js"), "{text}");
    assert!(
        err.contains("coverage: lines coverage is 50%, below the 90% threshold"),
        "{err}"
    );
    assert!(
        err.contains("coverage: 2 functions are not covered, more than the 1 allowed"),
        "{err}"
    );
    let summary = std::fs::read_to_string(dir.join("reports/coverage/coverage-summary.json"))
        .expect("summary written");
    assert!(summary.contains("\"total\":{"), "{summary}");
    assert!(dir.join("reports/coverage/coverage-final.json").exists());
    assert!(!dir.join("coverage/lcov.info").exists());

    // A run that cannot collect it goes without, and says so.
    let out = esdev_in(&dir)
        .args(["test", "--isolation=none"])
        .output()
        .expect("spawn esdev test --isolation=none");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("coverage: not collected"),
        "{}",
        stderr(&out)
    );
    // Asked for by name, it is refused.
    let out = esdev_in(&dir)
        .args(["test", "--isolation=none", "--coverage"])
        .output()
        .expect("spawn esdev test");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("--isolation=none runs them together"),
        "{}",
        stderr(&out)
    );

    write_in(
        &dir,
        "esdev.json",
        r#"{ "test": { "coverage": { "reporter": "html" } } }"#,
    );
    let out = esdev_in(&dir)
        .args(["test", "--coverage"])
        .output()
        .expect("spawn esdev test");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("`html` is not a coverage reporter"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--detect-async-leaks`: what a file's tests leave running fails the file,
/// named as the test would name it and at the line that started it — and the
/// file then ends rather than waiting on it.
#[test]
fn test_detect_async_leaks_names_what_a_file_left_running() {
    let dir = build_dir("t_async_leaks");
    write_in(
        &dir,
        "timers.test.ts",
        r#"import { test } from "runtime:test";

function poll(): void {
  setInterval(() => {}, 1000);
}
test("starts a poller and forgets it", () => {
  poll();
  setTimeout(() => {}, 5000);
});
test("cleans up", () => {
  const id = setTimeout(() => {}, 10);
  clearTimeout(id);
});
"#,
    );
    write_in(
        &dir,
        "clean.test.js",
        r#"import { test } from "runtime:test";
test("waits properly", async () => { await new Promise((r) => setTimeout(r, 5)); });
"#,
    );
    write_in(
        &dir,
        "server.test.js",
        r#"import { test } from "runtime:test";
import { serve } from "runtime:http";
test("starts a server and forgets it", () => {
  serve({ port: 0 }, () => new Response("hi"));
});
"#,
    );
    // Let go on purpose, so not a leak.
    write_in(
        &dir,
        "beat.test.js",
        "import { test } from \"runtime:test\";\nimport { unrefTimer } from \"runtime:process\";\n\
         test(\"a background heartbeat\", () => { unrefTimer(setInterval(() => {}, 1000)); });\n",
    );
    let out = esdev_in(&dir)
        .args(["test", "--detect-async-leaks", "--timeout=30000"])
        .output()
        .expect("spawn esdev test --detect-async-leaks");
    let text = slash_paths(&stdout(&out));
    assert!(!out.status.success(), "{text}");
    assert!(
        text.contains("  FAIL async leak: a setInterval\n    a setInterval was still pending after the file's tests finished\n        at poll ("),
        "{text}"
    );
    assert!(text.contains("timers.test.ts:4:3)\n"), "{text}");
    assert!(text.contains("  FAIL async leak: a setTimeout\n"), "{text}");
    assert!(text.contains("timers.test.ts:8:3\n"), "{text}");
    // The runner's own frames are not the test's to change.
    assert!(!text.contains("runAttempt"), "{text}");
    assert!(
        text.contains("  FAIL async leak: an HTTP server\n"),
        "{text}"
    );
    assert!(text.contains("close it in afterAll"), "{text}");
    assert!(
        text.contains("clean.test.js\n  1 passed, 0 failed"),
        "{text}"
    );
    assert!(
        text.contains("beat.test.js\n  1 passed, 0 failed"),
        "{text}"
    );
    assert!(text.contains("2 of 4 files failed"), "{text}");

    // Without it, the same file waits on what it left, until the budget ends
    // it — and the budget says where to look.
    let out = esdev_in(&dir)
        .args(["test", "timers", "--timeout=2000"])
        .output()
        .expect("spawn esdev test");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("--detect-async-leaks"),
        "{}",
        stderr(&out)
    );

    let out = esdev_in(&dir)
        .args(["test", "--detect-async-leaks", "--isolation=none"])
        .output()
        .expect("spawn esdev test");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("share one event loop"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `test.extend`: fixtures a test names are set up for it and torn down after
/// its `afterEach`, in both Vitest's builder and Playwright's object syntax;
/// file-scoped ones once, and torn down when the file's tests are done.
#[test]
fn test_fixtures_are_set_up_for_the_tests_that_name_them() {
    let dir = build_dir("t_fixtures");
    write_in(
        &dir,
        "fx.test.ts",
        r#"import { afterAll, afterEach, beforeEach, expect, test } from "runtime:test";

const log: string[] = [];

const withDb = test
  .extend("config", { port: 3000 })
  .extend("db", { scope: "file" }, async ({ config }, { onCleanup }) => {
    log.push(`db up on ${config.port}`);
    onCleanup(() => log.push("db down"));
    return { rows: [] as number[] };
  })
  .extend("user", async ({ db }, { onCleanup }) => {
    db.rows.push(1);
    log.push("user up");
    onCleanup(() => log.push("user down"));
    return { name: "ada" };
  });

const withPage = withDb.extend({
  page: async ({ user }: { user: { name: string } }, use: (v: string) => Promise<void>) => {
    log.push("page up");
    await use(`page for ${user.name}`);
    log.push("page down");
  },
  untouched: async ({}, use: (v: number) => Promise<void>) => {
    log.push("untouched up");
    await use(1);
  },
});

afterEach(() => log.push("afterEach"));

withDb("a fixture is set up for a test that names it", ({ user, config }) => {
  expect(user.name).toBe("ada");
  expect(config.port).toBe(3000);
});
withDb("a test that names none sets none up", () => {
  log.push("bare test");
});
withPage("object syntax hands its value to use, and tears down after", ({ page, task }) => {
  expect(page).toBe("page for ada");
  expect(task.name).toContain("object syntax");
});
withDb("a file fixture is set up once", ({ db }) => {
  expect(db.rows).toEqual([1, 1]);
});
test("context.skip stops and skips", ({ skip }) => {
  skip();
  throw new Error("unreachable");
});
test("context.skip with a false condition runs", ({ skip }) => {
  skip(false);
});
withDb.each([[1], [2]])("each still works with %d", (n) => {
  expect(typeof n).toBe("number");
});
test("the order", () => {
  expect(log).toEqual([
    "db up on 3000", "user up", "afterEach", "user down",
    "bare test", "afterEach",
    "user up", "page up", "afterEach", "page down", "user down",
    // Names only the file's db: no user is set up for it.
    "afterEach",
    "afterEach", "afterEach", "afterEach", "afterEach",
  ]);
});
afterAll(() => {
  // afterAll runs before the file's fixtures are torn down.
  if (log.includes("db down")) throw new Error("db torn down too early");
});
"#,
    );
    write_in(
        &dir,
        "teardown.test.js",
        r#"import { test } from "runtime:test";
import { write } from "runtime:fs";
const fileTest = test.extend("shared", { scope: "file" }, ({}, { onCleanup }) => {
  onCleanup(() => write("torn-down.txt", "yes"));
  return 1;
});
fileTest("uses it", ({ shared }) => { if (shared !== 1) throw new Error("no"); });
fileTest("uses it again", ({ shared }) => { if (shared !== 1) throw new Error("no"); });
"#,
    );
    let out = esdev_in(&dir)
        .args(["test", "fx", "teardown"])
        .output()
        .expect("spawn esdev test");
    let text = stdout(&out);
    assert!(out.status.success(), "{text}{}", stderr(&out));
    assert!(
        text.contains("fx.test.ts\n  8 passed, 0 failed, 1 skipped"),
        "{text}"
    );
    assert!(
        text.contains("teardown.test.js\n  2 passed, 0 failed"),
        "{text}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("torn-down.txt")).unwrap_or_default(),
        "yes",
        "the file fixture was not torn down"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A fixture that cannot be set up says why, in the test that needed it.
#[test]
fn test_fixture_mistakes_are_named() {
    let dir = build_dir("t_fixture_mistakes");
    write_in(
        &dir,
        "bad.test.js",
        r#"import { expect, test } from "runtime:test";
const loop = test.extend("a", ({ b }) => b).extend("b", ({ a }) => a);
loop("a cycle is named", ({ a }) => {});
const forgot = test.extend({ x: async ({}, use) => {} });
forgot("use() never called", ({ x }) => {});
const twice = test.extend("y", ({}, { onCleanup }) => { onCleanup(() => {}); onCleanup(() => {}); });
twice("onCleanup twice", ({ y }) => {});
const cross = test.extend("perTest", () => 1).extend("shared", { scope: "file" }, ({ perTest }) => perTest);
cross("file needs test", ({ shared }) => {});
"#,
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = stdout(&out);
    assert!(!out.status.success());
    for says in [
        "FAIL a cycle is named\n    Error: fixture a needs itself: a → b → a",
        "Error: fixture x returned without calling use(value)",
        "Error: fixture y: onCleanup can be called once; split it into two fixtures",
        "Error: the file-scoped fixture shared needs perTest, which is set up for each test",
    ] {
        assert!(text.contains(says), "{says} in\n{text}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Tags: from `test.tags`, a `describe`, a test's options and `@module-tag`;
/// selected by `--tags-filter`, giving their options, strict about names.
#[test]
fn test_tags_select_tests_and_give_them_options() {
    let dir = build_dir("t_tags");
    write_in(
        &dir,
        "esdev.json",
        r#"{ "test": { "tags": [
            { "name": "frontend", "description": "Tests written for frontend." },
            { "name": "db", "timeout": 5000 },
            { "name": "flaky", "retry": 2, "timeout": 30, "priority": 1 },
            { "name": "unit/components" },
            { "name": "acceptance" }
        ] } }"#,
    );
    write_in(
        &dir,
        "tagged.test.ts",
        r#"/**
 * @module-tag acceptance
 */
import { describe, expect, matchesTags, test } from "runtime:test";
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
test("renders", { tags: "frontend" }, () => {});
describe("queries", { tags: ["db"] }, () => {
  test("reads", () => {});
  // flaky's priority gives it flaky's 30ms, not db's 5000.
  test("flaky read", { tags: ["flaky"] }, async () => { await sleep(100); });
});
test("component", { tags: "unit/components" }, () => {});
let tries = 0;
test("retried by its tag", { tags: "flaky", timeout: 1000 }, () => { tries += 1; expect(tries).toBe(3); });
test("untagged", () => { console.log(`flaky selected: ${matchesTags(["flaky"])}`); });
"#,
    );
    let listed = |filter: &str| {
        let out = esdev_in(&dir)
            .args(["test", "--list", &format!("--tags-filter={filter}")])
            .output()
            .expect("spawn esdev test --list");
        assert!(out.status.success(), "{}", stderr(&out));
        stdout(&out)
            .lines()
            .filter(|line| {
                line.starts_with("  ") && !line.ends_with(" tests") && !line.ends_with(" test")
            })
            .map(|line| line.trim().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(listed("db"), ["queries > reads", "queries > flaky read"]);
    assert_eq!(listed("db && !flaky"), ["queries > reads"]);
    assert_eq!(listed("unit/*"), ["component"]);
    assert_eq!(
        listed("(frontend or unit/*) and acceptance"),
        ["renders", "component"]
    );
    assert!(
        listed("!acceptance").is_empty(),
        "the module tag is every test's"
    );

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = stdout(&out);
    assert!(!out.status.success(), "{text}");
    assert!(
        // flaky's retries too: three attempts, each held to its 30ms.
        text.contains("FAIL queries > flaky read\n    failed 3 attempts; the last:\n    Error: the test did not finish within 30ms"),
        "{text}"
    );
    assert!(text.contains("flaky selected: true"), "{text}");
    assert!(text.contains("5 passed, 1 failed"), "{text}");

    let out = esdev_in(&dir)
        .args(["test", "--tags-filter=!flaky"])
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}", stdout(&out));
    assert!(
        stdout(&out).contains("flaky selected: false"),
        "{}",
        stdout(&out)
    );
    assert!(
        stdout(&out).contains("filter: 2 other tests did not match"),
        "{}",
        stdout(&out)
    );

    let out = esdev_in(&dir)
        .args(["test", "--list-tags"])
        .output()
        .expect("spawn");
    assert_eq!(
        stdout(&out),
        "frontend: Tests written for frontend.\ndb\nflaky\nunit/components\nacceptance\n"
    );
    let out = esdev_in(&dir)
        .args(["test", "--list-tags=json"])
        .output()
        .expect("spawn");
    assert!(
        stdout(&out).starts_with(r#"{"tags":[{"name":"frontend""#),
        "{}",
        stdout(&out)
    );

    let out = esdev_in(&dir)
        .args(["test", "--tags-filter=db flaky"])
        .output()
        .expect("spawn");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("two tags need `and` or `or` between them"),
        "{}",
        stderr(&out)
    );

    write_in(
        &dir,
        "typo.test.ts",
        "import { test } from \"runtime:test\";\ntest(\"typo\", { tags: \"fronted\" }, () => {});\n",
    );
    let out = esdev_in(&dir)
        .args(["test", "typo"])
        .output()
        .expect("spawn");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains(
            r#"the tag "fronted" is not defined — test.tags in esdev.json defines frontend, db"#
        ),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `esdev test --typecheck` runs the project's `tsc --noEmit` before the
/// tests, and its failure fails a run whose tests pass. A fake `npm` stands in
/// for the package manager, as in `esdev typecheck`'s own test.
#[cfg(unix)]
#[test]
fn test_typecheck_fails_a_run_whose_types_do_not_check() {
    use std::os::unix::fs::PermissionsExt;

    let dir = build_dir("t_typecheck");
    write_in(
        &dir,
        "package.json",
        r#"{ "packageManager": "npm@1.0.0", "devDependencies": { "typescript": "^5" } }"#,
    );
    std::fs::create_dir_all(dir.join("node_modules/typescript")).expect("create ts");
    write_in(&dir, "node_modules/typescript/package.json", "{}");
    write_in(
        &dir,
        "types.test.ts",
        "import { expectTypeOf, test } from \"runtime:test\";\n\
         test(\"runs\", () => { expectTypeOf(1).toBeNumber(); });\n",
    );
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("create bin");
    let npm = bin.join("npm");
    std::fs::write(
        &npm,
        "#!/bin/sh\necho \"types.test.ts(2,1): error TS2349\"\nexit \"${ESDEV_CHECK_EXIT:-0}\"\n",
    )
    .expect("write fake npm");
    std::fs::set_permissions(&npm, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let run = |exit: &str, args: &[&str]| {
        esdev_in(&dir)
            .arg("test")
            .arg("--typecheck")
            .args(args)
            .env("PATH", &path)
            .env("ESDEV_CHECK_EXIT", exit)
            .output()
            .expect("spawn esdev test --typecheck")
    };

    let out = run("0", &[]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(
        stderr(&out).contains("typecheck: tsc --noEmit"),
        "{}",
        stderr(&out)
    );

    let out = run("2", &[]);
    assert!(!out.status.success(), "a type error passed the run");
    assert!(
        stdout(&out).contains("1 passed, 0 failed"),
        "the tests ran too: {}",
        stdout(&out)
    );
    assert!(
        stderr(&out).contains("typecheck: tsc failed (exit code 2)"),
        "{}",
        stderr(&out)
    );

    // Under a machine reporter, what tsc prints stays off stdout.
    let out = run("2", &["--reporter=json"]);
    assert!(!out.status.success());
    assert!(!stdout(&out).contains("TS2349"), "{}", stdout(&out));
    assert!(stderr(&out).contains("TS2349"), "{}", stderr(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

/// `test.concurrent`: consecutive concurrent cases overlap, at most
/// `maxConcurrency` at once, and each keeps its own assertions, snapshots and
/// `beforeAll` — even through the global `expect` after an await.
#[test]
fn test_concurrent_cases_overlap_and_keep_their_own_state() {
    let dir = build_dir("t_concurrent");
    write_in(
        &dir,
        "conc.test.ts",
        r#"import { beforeAll, describe, expect, test } from "runtime:test";
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
let inFlight = 0;
let most = 0;
const track = async (ms: number) => {
  inFlight += 1;
  most = Math.max(most, inFlight);
  await sleep(ms);
  inFlight -= 1;
};

describe.concurrent("overlapping", () => {
  let opened = 0;
  beforeAll(async () => { await sleep(20); opened += 1; });
  for (let i = 0; i < 8; i++) {
    test(`case ${i}`, async () => {
      expect(opened).toBe(1);
      expect.assertions(3);
      await track(50);
      // Global expect, after an await, still counts for this test.
      expect(i).toBeGreaterThanOrEqual(0);
      expect(i).toMatchSnapshot();
    });
  }
});
test("at most five ran at once, and more than one did", () => {
  expect(most).toBe(5);
});
let order: string[] = [];
test("sequential one", async () => { order.push("a start"); await sleep(10); order.push("a end"); });
test("sequential two", async () => { order.push("b start"); order.push("b end"); });
test("sequential cases still do not overlap", () => {
  expect(order).toEqual(["a start", "a end", "b start", "b end"]);
});
test.concurrent("a lone concurrent case fails on its own", async () => {
  await sleep(5);
  expect.assertions(2);
  expect(1).toBe(1);
});
"#,
    );
    write_in(
        &dir,
        "most.test.ts",
        r#"import { expect, test } from "runtime:test";
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
let inFlight = 0, most = 0;
for (let i = 0; i < 6; i++) test.concurrent(`c${i}`, async () => { inFlight++; most = Math.max(most, inFlight); await sleep(30); inFlight--; });
test("limit", () => { console.log("most at once:", most); });
"#,
    );
    let out = esdev_in(&dir)
        .args(["test", "conc"])
        .output()
        .expect("spawn esdev test");
    let text = stdout(&out);
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("12 passed, 1 failed"), "{text}");
    assert!(
        text.contains("FAIL a lone concurrent case fails on its own\n    Error: expected 2 assertions, and 1 ran"),
        "{text}"
    );
    let snap =
        std::fs::read_to_string(dir.join("__snapshots__/conc.test.ts.snap")).expect("snapshots");
    for i in 0..8 {
        assert!(
            snap.contains(&format!(
                "=== overlapping > case {i}: snapshot 1 [value]\n{i}\n"
            )),
            "{snap}"
        );
    }

    let most = |args: &[&str]| {
        let out = esdev_in(&dir)
            .args(["test", "most"])
            .args(args)
            .output()
            .expect("spawn");
        assert!(out.status.success(), "{}", stdout(&out));
        stdout(&out)
    };
    assert!(most(&[]).contains("most at once: 5"));
    assert!(most(&["--max-concurrency=2"]).contains("most at once: 2"));
    write_in(&dir, "esdev.json", r#"{ "test": { "maxConcurrency": 3 } }"#);
    assert!(most(&[]).contains("most at once: 3"));
    let out = esdev_in(&dir)
        .args(["test", "--max-concurrency=0"])
        .output()
        .expect("spawn");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("a whole number above zero"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The answers a mock gives beyond one implementation: throwing, for the
/// length of a callback, by argument, and for a block with `using` — and the
/// environment put back after `mock.env`.
#[test]
fn test_mock_answers_by_argument_and_puts_back_what_it_changed() {
    let dir = build_dir("t_mock_when");
    write_in(
        &dir,
        "w.test.ts",
        r#"import { expect, mock, test } from "runtime:test";
import { env } from "runtime:process";

test("mockThrow", () => {
  const f = mock.fn().mockThrowOnce(new Error("once")).mockThrow(new Error("always"));
  expect(() => f()).toThrow("once");
  expect(() => f()).toThrow("always");
  expect(f.mock.results.map((r) => r.type)).toEqual(["throw", "throw"]);
});
test("withImplementation", async () => {
  const f = mock.fn(() => "a");
  f.withImplementation(() => "b", () => expect(f()).toBe("b"));
  expect(f()).toBe("a");
  await f.withImplementation(() => "c", async () => { await null; expect(f()).toBe("c"); });
  expect(f()).toBe("a");
  expect(f.getMockImplementation()!()).toBe("a");
});
test("using restores a spy", () => {
  const o = { m: () => "real" };
  {
    using spy = mock.spyOn(o, "m").mockReturnValue("fake");
    expect(o.m()).toBe("fake");
    expect(spy).toHaveBeenCalledOnce();
  }
  expect(o.m()).toBe("real");
});
test("when", () => {
  const spy = mock.fn((k: string) => `orig ${k}`);
  const w = mock.when(spy).calledWith("theme").thenReturn("light").thenReturn("dark", { times: 2 })
    .calledWith(expect.any(Number)).thenReturnOnce("num");
  expect(w).not.toHaveBeenExhausted();
  expect(spy("theme")).toBe("dark");
  expect(spy("theme")).toBe("dark");
  expect(spy("theme")).toBe("light");
  expect(spy(4)).toBe("num");
  expect(spy(4)).toBe("orig 4");
  expect(w).toHaveBeenExhausted();
  w[Symbol.dispose]();
  expect(spy("theme")).toBe("orig theme");
});
test("when throw", async () => {
  const spy = mock.fn();
  using w = mock.when(spy, { onUnmatched: "throw" }).calledWith(1).thenResolve("one").calledWith(2).thenThrow(new Error("two"));
  await expect(spy(1)).resolves.toBe("one");
  expect(() => spy(2)).toThrow("two");
  expect(() => spy(3)).toThrow("has no answer for the arguments (3)");
});
test("exhausted failure message", () => {
  const spy = mock.fn().mockName("fetchUser");
  const w = mock.when(spy).calledWith(1, "a").thenReturnOnce("x");
  expect(() => expect(w).toHaveBeenExhausted()).toThrow('expected fetchUser to have used every answer; left: calledWith(1, "a").thenReturn (0 of 1)');
  expect(() => expect(mock.when(mock.fn())).toHaveBeenExhausted()).toThrow("to have answers, and it has none");
  expect(() => expect(spy).toHaveBeenExhausted()).toThrow("needs a mock.when");
});
test("env", () => {
  env.KEEP = "kept";
  // A host without HOME (Windows runners) would leave nothing to restore;
  // the mechanics under test are mock, delete and restore, not the host.
  if (env.HOME === undefined) env.HOME = "home";
  mock.env("KEEP", "changed").env("NEW_ONE", "1").env("HOME", undefined).env("MY_TOKEN", "abc");
  expect(env.KEEP).toBe("changed");
  expect(env.NEW_ONE).toBe("1");
  expect("HOME" in env).toBe(false);
  mock.restoreAll();
  expect(env.KEEP).toBe("kept");
  expect("NEW_ONE" in env).toBe(false);
  expect(typeof env.HOME).toBe("string");
  expect("MY_TOKEN" in env).toBe(false);
});
"#,
    );
    let out = esdev_in(&dir)
        .args(["test"])
        .output()
        .expect("spawn esdev test");
    let text = stdout(&out);
    assert!(out.status.success(), "{text}{}", stderr(&out));
    assert!(text.contains("7 passed, 0 failed"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_mock_env_needs_the_env_capability() {
    let dir = build_dir("t_mock_env_denied");
    write_in(
        &dir,
        "env.test.js",
        "import { mock, test } from \"runtime:test\";\n\
         test(\"stubs\", () => { mock.env(\"PORT\", \"1\"); });\n",
    );
    let out = esdev_in(&dir)
        .args(["test", "--deny-all"])
        .output()
        .expect("spawn esdev test");
    let text = stdout(&out);
    assert!(!out.status.success(), "{text}");
    assert!(
        text.contains("NotAllowedError: capability denied: Env"),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_mock_module_at_the_top_replaces_the_files_own_imports() {
    let dir = module_mock_project("t_mock_module_hoisted");
    write_in(
        &dir,
        "src/signup.test.ts",
        "import { expect, mock, test } from \"runtime:test\";\n\
         import { signup } from \"./signup.ts\";\n\
         import greet from \"greeting\";\n\
         import * as mail from \"./mail.ts\";\n\
         \n\
         mock.module(\"./mail.ts\", async (importOriginal) => ({\n\
           ...(await importOriginal()),\n\
           send: mock.fn(() => \"mocked\"),\n\
         }));\n\
         mock.module(\"greeting\", () => ({ default: () => \"mocked greeting\" }));\n\
         \n\
         test(\"an import written above the mock is the mock\", () => {\n\
           expect(signup(\"ada\")).toBe(\"mocked\");\n\
           expect(mail.send).toHaveBeenCalledWith(\"ada\");\n\
         });\n\
         test(\"importOriginal keeps what the factory spreads\", () => {\n\
           expect(mail.provider).toBe(\"smtp\");\n\
         });\n\
         test(\"a package is mocked by its name\", () => {\n\
           expect(greet()).toBe(\"mocked greeting\");\n\
         });\n\
         test(\"importActual is the real module\", async () => {\n\
           const real = await mock.importActual<typeof import(\"./mail.ts\")>(\"./mail.ts\");\n\
           expect(real.send(\"ada\")).toBe(\"sent to ada\");\n\
         });\n\
         test(\"the line numbers are the file's\", () => {\n\
           expect(1).toBe(2);\n\
         });\n",
    );
    // A file that mocks nothing gets the real module: mocks are per file.
    write_in(
        &dir,
        "src/real.test.ts",
        "import { expect, test } from \"runtime:test\";\n\
         import { signup } from \"./signup.ts\";\n\
         test(\"unmocked\", () => { expect(signup(\"bo\")).toBe(\"sent to bo\"); });\n",
    );
    let out = esdev_in(&dir)
        .args(["test", "--jobs=1"])
        .output()
        .expect("spawn esdev test");
    let text = slash_paths(&stdout(&out));
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("4 passed, 1 failed"), "{text}");
    assert!(
        text.contains("FAIL the line numbers are the file's\n    Error: expected 1 to be 2"),
        "{text}"
    );
    assert!(text.contains("src/signup.test.ts:27:11"), "{text}");
    assert!(text.contains("src/real.test.ts\n"), "{text}");
    assert!(text.contains("1 passed, 0 failed"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_mock_module_in_a_test_applies_to_what_is_imported_after_it() {
    let dir = module_mock_project("t_mock_module_later");
    write_in(
        &dir,
        "src/later.test.js",
        "import { expect, mock, test } from \"runtime:test\";\n\
         \n\
         test(\"a dynamic import after the mock gets it\", async () => {\n\
           await mock.module(\"./signup.ts\", () => ({ signup: () => \"stubbed\" }));\n\
           const { signup } = await import(\"./signup.ts\");\n\
           expect(signup(\"ada\")).toBe(\"stubbed\");\n\
         });\n\
         test(\"an async factory is waited for\", async () => {\n\
           await mock.module(\"./mail.ts\", async () => ({ send: () => \"later\" }));\n\
           const { send } = await import(\"./mail.ts\");\n\
           expect(send(\"ada\")).toBe(\"later\");\n\
         });\n",
    );
    let out = esdev_in(&dir)
        .args(["test"])
        .output()
        .expect("spawn esdev test");
    let text = stdout(&out);
    assert!(out.status.success(), "{text}{}", stderr(&out));
    assert!(text.contains("2 passed, 0 failed"), "{text}");
    // Nothing loaded the real module.
    assert!(!text.contains("real mail evaluated"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_mock_module_says_what_is_wrong_with_a_call() {
    let dir = module_mock_project("t_mock_module_errors");
    write_in(
        &dir,
        "src/errors.test.js",
        "import { expect, mock, test } from \"runtime:test\";\n\
         \n\
         test(\"a factory is required\", () => {\n\
           expect(() => mock.module(\"./mail.ts\")).toThrow(\"needs a factory\");\n\
         });\n\
         test(\"the factory returns exports\", () => {\n\
           expect(() => mock.module(\"./mail.ts\", () => 42)).toThrow(\"must return an object of exports\");\n\
         });\n\
         test(\"a call not made by name has no file to resolve from\", () => {\n\
           const call = mock.module;\n\
           expect(() => call(\"./mail.ts\", () => ({}))).toThrow(\"must be called by name in the file\");\n\
         });\n\
         test(\"an unknown module is named\", () => {\n\
           expect(() => mock.module(\"./missing.ts\", () => ({}))).toThrow(\"missing.ts\");\n\
         });\n",
    );
    let out = esdev_in(&dir)
        .args(["test"])
        .output()
        .expect("spawn esdev test");
    let text = stdout(&out);
    assert!(out.status.success(), "{text}{}", stderr(&out));
    assert!(text.contains("4 passed, 0 failed"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The utilities around the matchers: custom matchers, counted and soft
/// assertions, polling, per-test options, `test.fails`, and cleanup that belongs
/// to one test. Each is exercised both ways — a case that must pass, and a case
/// that must fail with its own message — because a utility that never fails
/// looks exactly like one that works.
#[test]
fn test_utilities_hold_and_fail_where_they_should() {
    let dir = build_dir("t_utilities");
    write_in(
        &dir,
        "utils.test.js",
        r#"
import { test, expect, onTestFinished, onTestFailed, waitFor, clock } from "runtime:test";

expect.extend({
  toBeWithin(received, lo, hi) {
    return { pass: received >= lo && received <= hi, message: () => `expected ${received} to be within ${lo}..${hi}` };
  },
  async toResolveTo(received, want) {
    return { pass: (await received) === want, message: () => `did not resolve to ${want}` };
  },
});

test("extend: direct, negated, asymmetric, async", async () => {
  expect(5).toBeWithin(1, 10);
  expect(50).not.toBeWithin(1, 10);
  expect({ n: 3 }).toEqual({ n: expect.toBeWithin(1, 5) });
  expect({ n: 30 }).toEqual({ n: expect.not.toBeWithin(1, 5) });
  await expect(Promise.resolve(2)).toResolveTo(2);
  await expect(Promise.resolve(4)).resolves.toBeWithin(1, 5);
});

test("assertions are counted once each", () => {
  expect.assertions(3);
  expect(1).toBe(1);
  expect(2).toStrictEqual(2);
  expect(3).not.toBe(4);
});

test("toSatisfy, toBeOneOf, closeTo, expect.not", () => {
  expect(4).toSatisfy((n) => n % 2 === 0);
  expect("b").toBeOneOf(["a", "b"]);
  expect({ x: 0.1 + 0.2 }).toEqual({ x: expect.closeTo(0.3, 5) });
  expect("hello").toEqual(expect.not.stringContaining("bye"));
});

test("poll and waitFor run on the real clock while time is frozen", async () => {
  clock.freeze();
  let n = 0;
  let ready = false;
  queueMicrotask(() => { ready = true; });
  await expect.poll(() => ++n, { interval: 1 }).toBeGreaterThan(3);
  expect(await waitFor(() => { if (!ready) throw new Error("not yet"); return "ok"; })).toBe("ok");
  clock.release();
});

let flaky = 0;
test("retry runs the case again", () => { flaky++; expect(flaky).toBe(3); }, { retry: 2 });

test.fails("a known failure passes", () => { expect(1).toBe(2); });

const order = [];
test("onTestFinished runs newest first", () => {
  onTestFinished(() => order.push("a"));
  onTestFinished(() => order.push("b"));
});
test("after the test it belonged to", () => { expect(order).toEqual(["b", "a"]); });

test("MUSTFAIL soft", () => {
  expect.soft(1).toBe(2);
  expect.soft("a").toBe("b");
  expect(true).toBe(true);
});
test("MUSTFAIL assertions", () => { expect.assertions(3); expect(1).toBe(1); });
test("MUSTFAIL hasAssertions", () => { expect.hasAssertions(); });
test("MUSTFAIL timeout", async () => { await new Promise(() => {}); }, 50);
test.fails("MUSTFAIL fixed", () => {});
test("MUSTFAIL onTestFailed", () => {
  onTestFailed((err) => console.log(`onTestFailed saw: ${err.message}`));
  throw new Error("boom");
});
test("MUSTFAIL poll", async () => { await expect.poll(() => 1, { timeout: 30, interval: 5 }).toBe(2); });
test("MUSTFAIL retry", () => { throw new Error("always"); }, { retry: 1 });
test("MUSTFAIL unreachable", () => { expect.unreachable("nope"); });
test("MUSTFAIL custom", () => { expect(0).toBeWithin(1, 2); });
"#,
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("8 passed, 10 failed"), "{text}");
    for (case, want) in [
        (
            "soft",
            "2 assertions failed:\n      1. Error: expected 1 to be 2\n      2. Error: expected \"a\" to be \"b\"",
        ),
        ("assertions", "expected 3 assertions, and 1 ran"),
        (
            "hasAssertions",
            "expected at least one assertion, and none ran",
        ),
        ("timeout", "the test did not finish within 50ms"),
        ("fixed", "expected this test to fail, and it passed"),
        ("onTestFailed", "Error: boom"),
        (
            "poll",
            "expected 1 to be 2 (still failing after 30ms of polling)",
        ),
        ("retry", "failed 2 attempts; the last:\n    Error: always"),
        ("unreachable", "Error: nope"),
        ("custom", "expected 0 to be within 1..2"),
    ] {
        let heading = format!("  FAIL MUSTFAIL {case}\n");
        let at = text
            .find(&heading)
            .unwrap_or_else(|| panic!("{case} did not fail:\n{text}"));
        let rest = &text[at + heading.len()..];
        let section = &rest[..rest.find("  FAIL ").unwrap_or(rest.len())];
        assert!(
            section.contains(want),
            "{case} failed with the wrong message:\n{text}"
        );
    }
    // Said once, not repeated by the stack under it.
    assert_eq!(
        text.matches("still failing after 30ms").count(),
        1,
        "{text}"
    );
    assert!(text.contains("onTestFailed saw: boom"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The DOM matchers, against esdev's DOM — the browser run of the same file is
/// covered by the real-browser test. Every one is used, and a failure names the
/// element it was about.
#[test]
fn dom_matchers_read_the_dom() {
    let dir = build_dir("t_dom_matchers");
    write_in(
        &dir,
        "dom.test.js",
        r##"
import { test, expect, beforeEach } from "runtime:test";

beforeEach(() => {
  document.body.innerHTML = `
    <form>
      <input id="name" class="field big" value="Ada" required>
      <input id="age" type="number" value="36">
      <input id="ok" type="checkbox" checked>
      <select id="multi" multiple><option value="a" selected>A</option><option value="b">B</option><option value="c" selected>C</option></select>
      <fieldset disabled><legend><button id="inlegend">L</button></legend><button id="off">Off</button></fieldset>
      <div role="switch" aria-checked="true" id="sw"></div>
      <p id="text">  Hello,
         world  </p>
      <div id="empty"><!-- note --></div>
      <div id="gone" style="display: none"><span id="inner">x</span></div>
      <div id="styled" style="color: rgb(255, 0, 0); margin-top: 4px"></div>
    </form>`;
});

const $ = (id) => document.getElementById(id);

test("presence and visibility", () => {
  expect($("name")).toBeInTheDocument();
  expect(document.querySelector("#missing")).not.toBeInTheDocument();
  expect(document.createElement("div")).not.toBeInTheDocument();
  expect($("text")).toBeVisible();
  expect($("inner")).not.toBeVisible();
});

test("text, attributes, classes", () => {
  expect($("text")).toHaveTextContent("Hello, world");
  expect($("text")).toHaveTextContent(/^Hello/);
  expect($("name")).toHaveAttribute("required");
  expect($("name")).toHaveAttribute("value", "Ada");
  expect($("name")).not.toHaveAttribute("disabled");
  expect($("name")).toHaveClass("field");
  expect($("name")).toHaveClass("big field", { exact: true });
  expect($("name")).not.toHaveClass("small");
});

test("values and state", () => {
  expect($("name")).toHaveValue("Ada");
  expect($("age")).toHaveValue(36);
  expect($("multi")).toHaveValue(["a", "c"]);
  expect($("ok")).toBeChecked();
  expect($("sw")).toBeChecked();
  expect($("off")).toBeDisabled();
  expect($("inlegend")).toBeEnabled();
  expect($("name")).toBeRequired();
  $("name").focus();
  expect($("name")).toHaveFocus();
  expect($("age")).not.toHaveFocus();
});

test("structure and style", () => {
  expect($("empty")).toBeEmptyDOMElement();
  expect($("text")).not.toBeEmptyDOMElement();
  expect($("gone")).toContainElement($("inner"));
  expect($("gone")).toContainHTML('<span id="inner">x</span>');
  expect($("styled")).toHaveStyle({ color: "rgb(255, 0, 0)", marginTop: "4px" });
  expect($("styled")).toHaveStyle("margin-top: 4px");
  expect($("gone")).toHaveStyle({ display: "none" });
});

test("MUSTFAIL named", () => { expect($("name")).toHaveClass("small"); });
test("MUSTFAIL not a node", () => { expect(null).toHaveTextContent("x"); });
test("MUSTFAIL checkbox value", () => { expect($("ok")).toHaveValue("on"); });
"##,
    );
    let out = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(text.contains("4 passed, 3 failed"), "{text}");
    assert!(
        text.contains(
            r#"expected <input id="name" class="field big"> to have the class small — found class="field big""#
        ),
        "{text}"
    );
    assert!(
        text.contains("expect(...).toHaveTextContent needs a DOM node, and null is not one"),
        "{text}"
    );
    assert!(
        text.contains("a checkbox or radio's state is toBeChecked()"),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Snapshot keys that stay put: a line break in a test's name, a named
/// snapshot after another is added before it, two tests sharing a name, a
/// skipped test's entries, and a retried attempt's result.
#[test]
fn snapshot_keys_survive_names_retries_and_skips() {
    let dir = build_dir("t_snapshot_stable_keys");
    let file = |body: &str| {
        write_in(
            &dir,
            "keys.test.js",
            &format!("import {{ test, expect }} from \"runtime:test\";\n{body}"),
        );
    };
    file(
        r#"test("named", () => { expect({ v: 1 }).toMatchSnapshot("body"); });
test("multi\nline", () => { expect(1).toMatchSnapshot(); });
test("later skipped", () => { expect(2).toMatchSnapshot(); });
test("retried", () => { expect("stable").toMatchSnapshot(); });
test("dup", () => { expect("first").toMatchSnapshot(); });
test("dup", () => { expect("second").toMatchSnapshot(); });
"#,
    );
    let first = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&first), stderr(&first));
    // The second test with a taken name is refused rather than sharing.
    assert!(
        text.contains("another test in this file is also named \"dup\""),
        "{text}"
    );
    assert!(text.contains("5 passed, 1 failed"), "{text}");
    let stored = std::fs::read_to_string(dir.join("__snapshots__/keys.test.js.snap"))
        .expect("snapshots written");
    // A line break in a name is escaped, so the heading stays one line.
    assert!(
        stored.contains("=== multi\\nline: snapshot 1 [value]\n"),
        "{stored}"
    );

    file(
        r#"test("named", () => { expect(0).toMatchSnapshot(); expect({ v: 1 }).toMatchSnapshot("body"); });
test("multi\nline", () => { expect(1).toMatchSnapshot(); });
test.skip("later skipped", () => { expect(2).toMatchSnapshot(); });
let n = 0;
test("retried", () => { n++; expect(n === 1 ? "flaky" : "stable").toMatchSnapshot(); }, { retry: 1 });
test("dup", () => { expect("first").toMatchSnapshot(); });
"#,
    );
    let second = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&second), stderr(&second));
    assert!(second.status.success(), "{text}");
    // The named snapshot kept its key; the new unnamed one is written beside it.
    // The escaped name was read back. The retried case's first attempt is not
    // counted. The skipped test's entry is not called obsolete.
    assert!(
        text.contains("snapshots: 4 matched, 0 failed, 1 written"),
        "{text}"
    );
    assert!(text.contains("named: snapshot 1"), "{text}");
    assert!(!text.contains("obsolete"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Inline snapshots, as Jest and Vitest write them: taken into the source on a
/// local run, matched after, refused in `--ci`, updated by
/// `--update-snapshots` — in a TypeScript file, whose lines the transform
/// reflows, so finding the call depends on the frame being mapped back.
#[test]
fn inline_snapshots_are_written_into_the_source_and_matched() {
    let dir = build_dir("t_inline_snapshots");
    let test = dir.join("inline.test.ts");
    write_in(
        &dir,
        "inline.test.ts",
        r#"import { test, expect } from "runtime:test";

type User = { id: number; name: string };

test("taken", () => {
  const user: User = { id: 7, name: "ada" };
  expect(user.id).toMatchInlineSnapshot();
  expect(user).toMatchInlineSnapshot({ id: expect.any(Number) });
});

test("written by another runner", () => {
  expect({ name: "ada", tags: ["a", "b"] }).toMatchInlineSnapshot(`
    {
      "name": "ada",
      "tags": [
        "a",
        "b",
      ],
    }
  `);
});
"#,
    );
    let ci = esdev_in(&dir)
        .args(["test", "--ci"])
        .output()
        .expect("spawn esdev test --ci");
    assert!(!ci.status.success());
    assert!(
        stdout(&ci).contains("no inline snapshot; --ci does not write them"),
        "{}",
        stdout(&ci)
    );
    assert!(
        std::fs::read_to_string(&test)
            .unwrap()
            .contains("toMatchInlineSnapshot();"),
        "--ci wrote into the source"
    );

    let first = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&first), stderr(&first));
    assert!(first.status.success(), "{text}");
    // The literal written the way Jest writes one matched as it stands.
    assert!(
        text.contains("snapshots: 1 matched, 0 failed, 2 written"),
        "{text}"
    );
    let source = std::fs::read_to_string(&test).unwrap();
    assert!(
        source.contains("expect(user.id).toMatchInlineSnapshot(`7`);"),
        "{source}"
    );
    assert!(
        source.contains(
            "expect(user).toMatchInlineSnapshot({ id: expect.any(Number) }, `\n    {\n      \"id\": any(Number),\n      \"name\": \"ada\",\n    }\n  `);"
        ),
        "{source}"
    );

    let again = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        stdout(&again).contains("snapshots: 3 matched, 0 failed, 0 written"),
        "{}",
        stdout(&again)
    );

    std::fs::write(
        &test,
        source.replace("name: \"ada\" };", "name: \"bob\" };"),
    )
    .unwrap();
    let changed = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let out = stdout(&changed);
    assert!(!changed.status.success(), "{out}");
    assert!(out.contains("inline snapshot changed"), "{out}");
    assert!(
        out.contains("inline.test.ts:8:"),
        "the call, on the line it was written: {out}"
    );
    assert!(
        out.contains("-   \"name\": \"ada\",\n    +   \"name\": \"bob\","),
        "{out}"
    );

    let updated = esdev_in(&dir)
        .args(["test", "--update-snapshots"])
        .output()
        .expect("spawn esdev test -u");
    assert!(updated.status.success(), "{}", stdout(&updated));
    assert!(
        std::fs::read_to_string(&test)
            .unwrap()
            .contains("\"name\": \"bob\",")
    );

    write_in(
        &dir,
        "loop.test.js",
        "import { test, expect } from \"runtime:test\";\n\
         test(\"loop\", () => { for (const n of [1, 2]) expect(n).toMatchInlineSnapshot(); });\n",
    );
    let looped = esdev_in(&dir)
        .args(["test", "loop"])
        .output()
        .expect("spawn esdev test");
    assert!(!looped.status.success());
    assert!(
        stdout(&looped).contains("cannot be taken in a loop"),
        "{}",
        stdout(&looped)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `-t` / `--test-name-pattern` and `--test-skip-pattern`: matched against a
/// test's full name, the tests left out counted rather than hidden, and a group
/// with nothing selected never set up.
#[test]
fn test_name_patterns_select_by_full_name() {
    let dir = build_dir("t_name_pattern");
    write_in(
        &dir,
        "f.test.js",
        r#"import { test, describe, beforeAll } from "runtime:test";
describe("auth", () => {
  beforeAll(() => console.log("auth setup ran"));
  test("logs in", () => {});
  test("logs out", () => {});
});
describe("billing", () => {
  beforeAll(() => console.log("billing setup ran"));
  test("charges", () => { throw new Error("not selected, never run"); });
});
"#,
    );
    let out = esdev_in(&dir)
        .args(["test", "-t=auth"])
        .output()
        .expect("spawn esdev test -t");
    let text = stdout(&out);
    assert!(out.status.success(), "{text}{}", stderr(&out));
    assert!(text.contains("auth setup ran"), "{text}");
    assert!(!text.contains("billing setup ran"), "{text}");
    assert!(
        text.contains("  filter: 1 other test did not match\n  2 passed, 0 failed, 1 skipped"),
        "{text}"
    );

    let out = esdev_in(&dir)
        .args([
            "test",
            "--test-name-pattern=auth > logs",
            "--test-skip-pattern=out$",
        ])
        .output()
        .expect("spawn esdev test");
    assert!(
        stdout(&out).contains("1 passed, 0 failed, 2 skipped"),
        "{}",
        stdout(&out)
    );

    let out = esdev_in(&dir)
        .args(["test", "-t=auth", "--reporter=json"])
        .output()
        .expect("spawn esdev test --reporter=json");
    assert!(
        stdout(&out).contains(r#""passed":2,"failed":0,"skipped":1"#),
        "{}",
        stdout(&out)
    );

    let out = esdev_in(&dir)
        .args(["test", "-t=("])
        .output()
        .expect("spawn esdev test");
    assert!(!out.status.success());
    assert!(
        format!("{}{}", stdout(&out), stderr(&out))
            .contains("--test-name-pattern: Invalid regular expression"),
        "{}{}",
        stdout(&out),
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--bail[=<n>]`: after `n` failed tests the rest are not run — the tests left
/// in the file counted as skipped, and the files not started said out loud —
/// with failures added up across files.
#[test]
fn bail_stops_after_the_failure_limit_across_files() {
    let dir = build_dir("t_bail");
    write_in(
        &dir,
        "a.test.js",
        "import { test } from \"runtime:test\";\n\
         test(\"a1\", () => { throw new Error(\"x\"); });\n\
         test(\"a2\", () => { throw new Error(\"y\"); });\n\
         test(\"a3\", () => {});\n",
    );
    write_in(
        &dir,
        "b.test.js",
        "import { test } from \"runtime:test\";\n\
         test(\"b1\", () => { throw new Error(\"z\"); });\n\
         test(\"b2\", () => {});\n",
    );
    write_in(
        &dir,
        "c.test.js",
        "import { test } from \"runtime:test\";\ntest(\"c1\", () => {});\n",
    );

    let out = esdev_in(&dir)
        .args(["test", "--bail", "--jobs=1"])
        .output()
        .expect("spawn esdev test --bail");
    let text = stdout(&out);
    assert!(!out.status.success(), "{text}");
    assert!(
        text.contains(
            "  bail: 2 tests did not run after the failure limit\n  0 passed, 1 failed, 2 skipped"
        ),
        "{text}"
    );
    assert!(
        text.contains("bail: 2 files did not run after the failure limit"),
        "{text}"
    );
    assert!(!text.contains("b.test.js"), "{text}");

    // Failures add up across files: a's two, then b's one reaches three.
    let out = esdev_in(&dir)
        .args(["test", "--bail=3", "--jobs=1"])
        .output()
        .expect("spawn esdev test --bail=3");
    let text = stdout(&out);
    assert!(text.contains("  1 passed, 2 failed\n"), "{text}");
    assert!(
        text.contains(
            "  bail: 1 test did not run after the failure limit\n  0 passed, 1 failed, 1 skipped"
        ),
        "{text}"
    );
    assert!(
        text.contains("bail: 1 file did not run after the failure limit"),
        "{text}"
    );

    let out = esdev_in(&dir)
        .args(["test", "--bail=2", "--isolation=none"])
        .output()
        .expect("spawn esdev test --bail=2 --isolation=none");
    assert!(
        stdout(&out).contains("bail: 4 tests did not run"),
        "{}",
        stdout(&out)
    );

    let out = esdev_in(&dir)
        .args(["test", "--bail=0"])
        .output()
        .expect("spawn");
    assert!(
        stderr(&out).contains("--bail=0 is not a number of failed tests"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--randomize` / `--seed`: a shuffled order that a seed repeats exactly, that
/// the run prints so it can be repeated, and that never splits a group from
/// the hooks around it.
#[test]
fn randomize_shuffles_within_groups_and_a_seed_repeats_it() {
    let dir = build_dir("t_randomize");
    write_in(
        &dir,
        "r.test.js",
        r#"import { test, describe, beforeAll, afterAll } from "runtime:test";
const log = (s) => console.log(`order ${s}`);
describe("g1", () => {
  beforeAll(() => log("g1 open"));
  afterAll(() => log("g1 close"));
  for (const n of [1, 2, 3, 4, 5]) test(`g1-${n}`, () => log(`g1-${n}`));
});
describe("g2", () => {
  beforeAll(() => log("g2 open"));
  afterAll(() => log("g2 close"));
  for (const n of [1, 2, 3, 4, 5]) test(`g2-${n}`, () => log(`g2-${n}`));
});
"#,
    );
    let order = |seed: &str| {
        let out = esdev_in(&dir)
            .args(["test", &format!("--seed={seed}")])
            .output()
            .expect("spawn esdev test --seed");
        assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
        assert!(
            stderr(&out).contains(&format!(
                "randomize: seed={seed} (run this order again with --seed={seed})"
            )),
            "{}",
            stderr(&out)
        );
        stdout(&out)
            .lines()
            .filter_map(|line| line.strip_prefix("order "))
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let first = order("7");
    assert_eq!(first, order("7"), "one seed is one order");
    assert_ne!(first, order("99"), "another seed is another order");
    let written: Vec<String> = ["g1", "g2"]
        .iter()
        .flat_map(|g| (1..=5).map(move |n| format!("{g}-{n}")))
        .collect();
    let tests: Vec<&String> = first.iter().filter(|s| !s.contains(' ')).collect();
    assert_ne!(
        tests,
        written.iter().collect::<Vec<_>>(),
        "not the written order: {first:?}"
    );
    // Each group's tests sit between its own open and close.
    for g in ["g1", "g2"] {
        let open = first
            .iter()
            .position(|s| *s == format!("{g} open"))
            .unwrap();
        let close = first
            .iter()
            .position(|s| *s == format!("{g} close"))
            .unwrap();
        assert_eq!(close - open, 6, "{first:?}");
        assert!(
            first[open + 1..close]
                .iter()
                .all(|s| s.starts_with(&format!("{g}-"))),
            "{first:?}"
        );
    }

    let out = esdev_in(&dir)
        .args(["test", "--randomize"])
        .output()
        .expect("spawn");
    assert!(
        stderr(&out).contains("randomize: seed="),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `repeats` (and `--repeats`): `n` more runs after the first, as Vitest and
/// Bun count them, each with its hooks; the first failing run fails the test
/// and is named.
#[test]
fn repeats_run_a_test_again_and_name_the_failing_run() {
    let dir = build_dir("t_repeats");
    write_in(
        &dir,
        "rep.test.js",
        r#"import { test, beforeEach } from "runtime:test";
let each = 0, a = 0, b = 0;
beforeEach(() => { each++; });
test("stable", () => { a++; }, { repeats: 4 });
test("counted", () => { console.log(`stable ran ${a} times, beforeEach ${each}`); });
test("flaky", () => { b++; if (b === 3) throw new Error("third run fails"); }, { repeats: 5 });
"#,
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = stdout(&out);
    assert!(text.contains("stable ran 5 times, beforeEach 6"), "{text}");
    assert!(
        text.contains("  FAIL flaky\n    failed on run 3 of 6\n    Error: third run fails"),
        "{text}"
    );

    write_in(
        &dir,
        "d.test.js",
        "import { test } from \"runtime:test\";\nlet n = 0;\n\
         test(\"x\", () => { n++; });\n\
         test(\"y\", () => { console.log(`x ran ${n} times`); }, { repeats: 0 });\n",
    );
    let out = esdev_in(&dir)
        .args(["test", "--repeats=2", "d"])
        .output()
        .expect("spawn");
    assert!(stdout(&out).contains("x ran 3 times"), "{}", stdout(&out));
    let out = esdev_in(&dir)
        .args(["test", "--repeats=-1"])
        .output()
        .expect("spawn");
    assert!(
        stderr(&out).contains("is not a number of repeats"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--list`: every file's tests by full name, nothing run — no test, no hook —
/// and a filter narrowing what is named.
#[test]
fn list_names_the_tests_and_runs_none() {
    let dir = build_dir("t_list");
    write_in(
        &dir,
        "a.test.js",
        "import { test, describe, beforeAll } from \"runtime:test\";\n\
         beforeAll(() => console.log(\"hook ran\"));\n\
         describe(\"auth\", () => { test(\"logs in\", () => { throw new Error(\"ran\"); }); });\n\
         test.each([1, 2])(\"row %d\", () => {});\n",
    );
    write_in(
        &dir,
        "b.test.js",
        "import { test } from \"runtime:test\";\ntest(\"b one\", () => {});\n",
    );
    let out = esdev_in(&dir)
        .args(["test", "--list", "--jobs=1"])
        .output()
        .expect("spawn esdev test --list");
    let text = stdout(&out);
    assert!(out.status.success(), "{text}{}", stderr(&out));
    assert!(
        text.contains("a.test.js\n  auth > logs in\n  row 1\n  row 2\n  3 tests\n"),
        "{text}"
    );
    assert!(text.contains("b.test.js\n  b one\n  1 test\n"), "{text}");
    assert!(text.contains("2 files listed"), "{text}");
    assert!(
        !text.contains("hook ran") && !text.contains("FAIL"),
        "{text}"
    );

    let out = esdev_in(&dir)
        .args(["test", "--list", "-t=auth", "--jobs=1"])
        .output()
        .expect("spawn esdev test --list -t");
    let text = stdout(&out);
    assert!(text.contains("  auth > logs in\n  1 test\n"), "{text}");
    assert!(!text.contains("row 1") && !text.contains("b one"), "{text}");

    let out = esdev_in(&dir)
        .args(["test", "--list", "--watch"])
        .output()
        .expect("spawn");
    assert!(
        stderr(&out).contains("nothing to watch"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The machine reporters: stdout holds the report and nothing else — what a
/// test prints goes to stderr — and `--reporter-outfile` writes the report to a
/// file while the terminal keeps the human one.
#[test]
fn reporters_write_json_junit_tap_and_dots() {
    let dir = build_dir("t_reporters");
    write_in(
        &dir,
        "a.test.js",
        "import { test, describe } from \"runtime:test\";\n\
         describe(\"math\", () => {\n\
           test(\"adds\", () => { console.log(\"noise from a test\"); });\n\
           test(\"fails <b>\", () => { throw new TypeError(\"bad & worse\"); });\n\
           test.skip(\"later\", () => {});\n\
         });\n",
    );
    let run = |args: &[&str]| {
        esdev_in(&dir)
            .arg("test")
            .args(args)
            .output()
            .expect("spawn esdev test")
    };

    let json = run(&["--reporter=json"]);
    let out = stdout(&json);
    for line in out.lines() {
        let _: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|err| panic!("not JSON ({err}): {line}\n{out}"));
    }
    assert!(
        out.contains(r#""passed":1,"failed":1,"skipped":1"#),
        "{out}"
    );
    assert!(
        out.ends_with("{\"type\":\"summary\",\"files\":1,\"failed\":1}\n"),
        "{out}"
    );
    assert!(
        stderr(&json).contains("noise from a test"),
        "{}",
        stderr(&json)
    );

    let junit = stdout(&run(&["--reporter=junit"]));
    assert!(
        junit.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n"),
        "{junit}"
    );
    assert!(
        junit.contains(
            "<testsuites name=\"esdev test\" tests=\"3\" failures=\"1\" errors=\"0\" skipped=\"1\""
        ),
        "{junit}"
    );
    assert!(
        junit.contains("<testcase classname=\"a.test.js\" name=\"math &gt; fails &lt;b&gt;\""),
        "{junit}"
    );
    assert!(
        junit.contains("<failure message=\"TypeError: bad &amp; worse\" type=\"TypeError\">"),
        "{junit}"
    );
    assert!(!junit.contains("noise"), "{junit}");

    let tap = stdout(&run(&["--reporter=tap"]));
    assert!(tap.starts_with("TAP version 13\n1..3\nok 1 - a.test.js > math > adds\nnot ok 2 - a.test.js > math > fails <b>\n  ---\n  error: |-\n    TypeError: bad & worse\n"), "{tap}");
    assert!(
        tap.contains("ok 3 - a.test.js > math > later # SKIP"),
        "{tap}"
    );

    let dots = stdout(&run(&["--reporter=dots"]));
    assert!(dots.starts_with(".x-\n"), "{dots}");
    assert!(dots.ends_with("1 passed, 1 failed, 1 skipped\n"), "{dots}");

    let file = run(&["--reporter=junit", "--reporter-outfile=reports/junit.xml"]);
    let terminal = stdout(&file);
    assert!(
        terminal.contains("  1 passed, 1 failed, 1 skipped"),
        "{terminal}"
    );
    assert!(terminal.contains("noise from a test"), "{terminal}");
    let written = std::fs::read_to_string(dir.join("reports/junit.xml")).expect("junit written");
    assert!(written.contains("tests=\"3\" failures=\"1\""), "{written}");

    let refused = run(&["--reporter=junit", "--isolation=none"]);
    assert!(
        stderr(&refused).contains("no per-file results for a machine reporter"),
        "{}",
        stderr(&refused)
    );
    let refused = run(&["--reporter=tap", "--list"]);
    assert!(
        stderr(&refused).contains("--list names tests"),
        "{}",
        stderr(&refused)
    );
    let refused = run(&["--reporter=xml"]);
    assert!(stderr(&refused).contains("junit"), "{}", stderr(&refused));
    let _ = std::fs::remove_dir_all(&dir);
}

/// `@permissions` gives a file its own grant, written as its deployment's
/// esrun flags: what it grants is reachable, nothing else is, and the command
/// line's rehearsal applies only to files that declare none. A mistake in it
/// fails that file; `--isolation=none` and browser runs refuse it.
#[test]
fn test_files_run_under_the_grant_they_declare() {
    let dir = build_dir("t_file_permissions");
    std::fs::create_dir_all(dir.join("config")).unwrap();
    write_in(&dir, "config/app.json", r#"{ "port": 3000 }"#);
    write_in(&dir, "secret.txt", "secret");
    write_in(&dir, "port.ts", "export const fallback = 8080;\n");
    write_in(
        &dir,
        "config.test.ts",
        r#"/**
 * The config loader reads its directory and nothing else.
 * @permissions --allow-imports --allow-read=./config
 */
import { expect, test } from "runtime:test";
import { file } from "runtime:fs";
import { env } from "runtime:process";
import { fallback } from "./port.ts";
test("reads its config", async () => {
  expect((await file("config/app.json").json()).port).not.toBe(fallback);
});
test("reads nothing else", async () => {
  await expect(file("secret.txt").text()).rejects.toThrow("read");
});
test("has no environment", () => {
  expect(() => env.HOME).toThrow();
});
"#,
    );
    write_in(
        &dir,
        "open.test.ts",
        r#"import { expect, test } from "runtime:test";
import { file } from "runtime:fs";
test("reads anything", async () => {
  expect(await file("secret.txt").text()).toBe("secret");
});
"#,
    );
    let run = |args: &[&str]| {
        let out = esdev_in(&dir)
            .arg("test")
            .args(args)
            .output()
            .expect("spawn esdev test");
        (out.status.success(), stdout(&out), stderr(&out))
    };
    let (ok, out, err) = run(&[]);
    assert!(ok, "{out}{err}");
    assert!(
        out.contains("config.test.ts\n  3 passed, 0 failed"),
        "{out}"
    );

    // The command line's rehearsal narrows the file that declares nothing,
    // and leaves the one that does as it declared.
    let (ok, out, _) = run(&["--deny-read"]);
    assert!(!ok);
    assert!(
        out.contains("config.test.ts\n  3 passed, 0 failed"),
        "{out}"
    );
    assert!(out.contains("FAIL reads anything"), "{out}");

    write_in(
        &dir,
        "typo.test.ts",
        "/** @permissions --allow-imports --allow-reed=./x */\n\
         import { test } from \"runtime:test\";\ntest(\"x\", () => {});\n",
    );
    let (ok, out, err) = run(&["typo"]);
    assert!(!ok);
    assert!(
        format!("{out}{err}").contains("@permissions: unknown option: --allow-reed"),
        "{out}{err}"
    );
    std::fs::remove_file(dir.join("typo.test.ts")).unwrap();

    let (ok, _, err) = run(&["--isolation=none"]);
    assert!(!ok);
    assert!(
        err.contains("config.test.ts declares @permissions"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `test.dom` gives the DOM to the files its globs name — a folder, a naming
/// convention — and to no others; `--isolation=none`, one realm for every
/// file, refuses a set that disagrees (D225).
#[test]
fn test_dom_globs_give_the_dom_to_the_files_they_name() {
    let dir = build_dir("test_dom_globs");
    for sub in ["src/ui", "src/lib"] {
        std::fs::create_dir_all(dir.join(sub)).expect("mkdir");
    }
    write_in(
        &dir,
        "esdev.json",
        r#"{ "test": { "dom": ["src/ui/**", "**/*.dom.test.js"] } }"#,
    );
    let with = "import { test, assertEquals } from 'runtime:test';\n\
                test('has a document', () => { assertEquals(typeof document, 'object'); });\n";
    let without = "import { test, assertEquals } from 'runtime:test';\n\
                   test('has no document', () => { assertEquals(typeof document, 'undefined'); });\n";
    write_in(&dir, "src/ui/button.test.js", with);
    write_in(&dir, "src/lib/form.dom.test.js", with);
    write_in(&dir, "src/lib/math.test.js", without);
    let ran = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(ran.status.success(), "{}{}", stdout(&ran), stderr(&ran));
    let report = format!("{}{}", stdout(&ran), stderr(&ran));
    assert!(report.contains("3 files passed"), "{report}");

    let shared = esdev_in(&dir)
        .args(["test", "--isolation=none"])
        .output()
        .expect("spawn esdev test --isolation=none");
    assert!(!shared.status.success(), "a split set shared one realm");
    assert!(
        stderr(&shared).contains("--isolation=none runs every file in one realm"),
        "{}",
        stderr(&shared)
    );

    write_in(&dir, "esdev.json", r#"{ "test": { "dom": ["src/[ui"] } }"#);
    let bad = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(!bad.status.success());
    assert!(stderr(&bad).contains("`test`'s `dom`"), "{}", stderr(&bad));
    let _ = std::fs::remove_dir_all(&dir);
}
