//! `esdev bench`: benchmark files, results and comparisons.

use super::*;

/// `esdev bench` runs `*.bench.*` files, which `esdev test` leaves alone;
/// `bench` in the test context measures and compares, and prints a table.
#[test]
fn esdev_bench_measures_and_compares_in_bench_files_only() {
    let dir = build_dir("t_bench");
    write_in(
        &dir,
        "parse.bench.ts",
        r#"import { clock, expect, test } from "runtime:test";
const input = JSON.stringify({ a: [1, 2, 3] });
const slow = () => { let x = 0; for (let i = 0; i < 5000; i++) x += i; return x; };

test("compare", async ({ bench }) => {
  const results = await bench.compare(
    bench("fast", () => { JSON.parse(input); }),
    bench("slow", () => { slow(); }),
    { time: 100 },
  );
  expect(results.get("fast")).toBeFasterThan(results.get("slow"), { delta: 0.5 });
  expect(results.get("slow")).toBeSlowerThan(results.get("fast"));
  expect(() => expect(results.get("slow")).toBeFasterThan(results.get("fast"))).toThrow(
    /expected slow \(.* ops\/sec\) to be faster than fast/,
  );
});
test("run, with hooks, under a frozen clock", async ({ bench }) => {
  clock.freeze();
  let hooks = 0;
  const result = await bench("async", { beforeEach: () => { hooks++; } }, async () => { await null; })
    .run({ time: 20, iterations: 4 });
  expect(result.samples).toBeGreaterThanOrEqual(4);
  expect(hooks).toBeGreaterThan(4);
  expect(result.throughput.mean).toBeCloseTo(1000 / result.latency.mean, 0);
});
test("misuse", ({ bench }) => {
  expect(() => (bench as any)("x")).toThrow("needs a name and a function");
  expect(() => (bench.compare as any)(1)).toThrow("made with bench()");
  expect(() => expect(1).toBeFasterThan(2 as any)).toThrow("two benchmark results");
});
"#,
    );
    write_in(
        &dir,
        "plain.test.ts",
        r#"import { expect, test } from "runtime:test";
test("no bench here", ({ bench }) => { bench; });
test("ordinary", () => { expect(1).toBe(1); });
"#,
    );
    let out = esdev_in(&dir)
        .arg("bench")
        .output()
        .expect("spawn esdev bench");
    let text = stdout(&out);
    assert!(out.status.success(), "{text}{}", stderr(&out));
    assert!(text.contains("parse.bench.ts"), "{text}");
    assert!(!text.contains("plain.test.ts"), "{text}");
    assert!(text.contains("3 passed, 0 failed"), "{text}");
    assert!(text.contains("ops/sec"), "{text}");
    assert!(text.contains("fastest"), "{text}");
    assert!(text.contains("× slower"), "{text}");

    // `esdev test` runs the test file, not the benchmark, and `bench` there
    // says where it belongs.
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = stdout(&out);
    assert!(!text.contains("parse.bench.ts"), "{text}");
    assert!(text.contains("1 passed, 1 failed"), "{text}");
    assert!(
        text.contains("bench is in the test context of *.bench.* files, which esdev bench runs"),
        "{text}"
    );

    std::fs::remove_file(dir.join("parse.bench.ts")).expect("remove");
    let out = esdev_in(&dir)
        .arg("bench")
        .output()
        .expect("spawn esdev bench");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("*.bench.js/") && stderr(&out).contains("*.benchmark.js/"),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A result written with `writeResult` is read back by `bench.from` and
/// compared against a fresh one; `b.start()`/`b.end()` time part of a call;
/// warnings say what to distrust; `--reporter=json` carries each result. The
/// files go through `runtime:fs`, so the file's own grant decides.
#[test]
fn esdev_bench_stores_results_times_sections_and_warns() {
    let dir = build_dir("t_bench_store");
    write_in(
        &dir,
        "store.bench.ts",
        r#"import { expect, test } from "runtime:test";
import { env } from "runtime:process";
const input = JSON.stringify({ a: [1, 2, 3], b: "x".repeat(200) });

test("stored", async ({ bench }) => {
  const parse = () => JSON.parse(input);
  if (env.WRITE_BENCH) {
    await bench("parse", { writeResult: "./results/parse.json", time: 50 }, parse).run();
    return;
  }
  const results = await bench.compare(
    bench("current", parse),
    bench.from("previous", "./results/parse.json"),
    bench.from("partial", () => ({ samples: 1, latency: { mean: 1, rme: 0 }, throughput: { mean: 1000 } })),
    { time: 50 },
  );
  expect(results.get("previous")!.stored).toBe(true);
  expect(results.get("previous")!.name).toBe("previous");
  expect(results.get("current")!.stored).toBeUndefined();
  expect(results.get("current")).toBeFasterThan(results.get("partial")!);
});
test("a section", async ({ bench }) => {
  let setUp = 0;
  const result = await bench("sort", (b) => {
    const data = Array.from({ length: 30000 }, (_, i) => 30000 - i);
    setUp++;
    b.start();
    data.sort((x, y) => x - y);
    b.end();
  }).run({ time: 50 });
  expect(setUp).toBeGreaterThan(0);
  // Long enough for the clock with room to spare: on a loaded machine the
  // measured clock step grows, and 3 000 elements once fell just under the
  // threshold. Whether it is noisy depends on the machine's load.
  expect(result.warnings.join()).not.toContain("too short");
});
test("warnings", async ({ bench }) => {
  const empty = await bench("empty", () => {}).run({ time: 30 });
  expect(empty.warnings.join()).toContain("as fast as calling an empty function");
  const tiny = await bench("tiny", (b) => { b.start(); b.end(); }).run({ time: 30 });
  expect(tiny.warnings.join()).toContain("too short for a clock");
});
test("misuse", async ({ bench }) => {
  await expect(bench("half", (b) => { b.start(); }).run()).rejects.toThrow("b.start() was called without b.end()");
  await expect(bench("end", (b) => { b.end(); }).run()).rejects.toThrow("b.end() was called without b.start()");
  let n = 0;
  await expect(bench("mixed", (b) => { if (n++ % 2) { b.start(); b.end(); } }).run()).rejects.toThrow("on every call");
  await expect(bench.from("missing", "./nope.json").run()).rejects.toThrow("bench.from cannot read ./nope.json");
  await expect(bench.from("bad", () => ({}) as any).run()).rejects.toThrow("not a benchmark result");
  expect(() => bench("x", { writeResult: 1 as any }, () => {})).toThrow("writeResult is the path");
});
"#,
    );
    std::fs::create_dir_all(dir.join("results")).expect("create results");
    write_in(
        &dir,
        "results/foreign.json",
        r#"{ "latency": { "mean": 1 } }"#,
    );
    let bench = |args: &[&str], env: Option<&str>| {
        let mut command = esdev_in(&dir);
        command.arg("bench").args(args);
        if let Some(value) = env {
            command.env("WRITE_BENCH", value);
        } else {
            command.env_remove("WRITE_BENCH");
        }
        command.output().expect("spawn esdev bench")
    };

    let out = bench(&["-t=stored"], Some("1"));
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let stored = std::fs::read_to_string(dir.join("results/parse.json")).expect("written");
    let stored: serde_json::Value = serde_json::from_str(&stored).expect("json");
    assert_eq!(stored["esdevBench"], 1);
    assert_eq!(stored["name"], "parse");
    assert!(stored["throughput"]["mean"].as_f64().unwrap() > 0.0);

    let out = bench(&[], None);
    let text = stdout(&out);
    assert!(out.status.success(), "{text}{}", stderr(&out));
    assert!(text.contains("4 passed, 0 failed"), "{text}");
    assert!(text.contains("previous (stored)"), "{text}");
    assert!(
        text.contains("! empty: as fast as calling an empty function"),
        "{text}"
    );

    // Each measured result is a `bench` line, before its file's line; the
    // tables go to stderr with the rest of what tests print.
    let out = bench(&["-t=stored", "--reporter=json"], None);
    let lines: Vec<serde_json::Value> = stdout(&out)
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON line"))
        .collect();
    assert_eq!(lines[0]["type"], "bench", "{lines:?}");
    assert_eq!(lines[0]["test"], "stored");
    assert_eq!(lines[0]["result"]["name"], "current");
    assert!(lines[0]["result"]["latency"]["p99"].is_number());
    assert_eq!(
        lines.len(),
        3,
        "only measured results, then file and summary: {lines:?}"
    );
    assert!(stderr(&out).contains("previous (stored)"));

    // A file writeResult did not write is refused.
    write_in(
        &dir,
        "foreign.bench.ts",
        r#"import { expect, test } from "runtime:test";
test("foreign", async ({ bench }) => {
  await expect(bench.from("x", "./results/foreign.json").run()).rejects.toThrow("is not a result that writeResult wrote");
});
"#,
    );
    let out = bench(&["foreign"], None);
    assert!(out.status.success(), "{}", stdout(&out));

    // Without the grant to write, writeResult fails the test, saying why.
    let out = bench(&["-t=stored", "--deny-write"], Some("1"));
    assert!(!out.status.success());
    assert!(
        stdout(&out).contains("writeResult cannot write ./results/parse.json"),
        "{}",
        stdout(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
