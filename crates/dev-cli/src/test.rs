//! `esdev test` — find the test files, run each in its own process, report.
//!
//! **One process per file.** A test suite is the place where isolation matters
//! most: a file that wedges, exhausts its heap, or calls `process.exit()` must
//! not decide the fate of the others, and a global left behind by one file must
//! not be visible to the next. A child process gives all of that for free, and
//! the prelude snapshot makes starting one cheap. It is the same reasoning
//! `--watch` uses, and the same mechanism — `esdev` re-executing itself.
//!
//! **The test file is the entry, not something an injected driver imports.**
//! That is not a stylistic choice: module resolution is jailed to the project
//! root detected from the entry's own directory (D25), so a generated driver
//! living in a temp directory could not import a test file in the project at
//! all. Instead the harness is prepended to the file's own source through the
//! same [`SourceTransform`](es_runtime_cli_common::run::SourceTransform) seam
//! that strips TypeScript — so the file keeps its path, its jail, its relative
//! imports and its `.ts` handling, and simply arrives with `test()` already
//! defined.
//!
//! The API is the one this repository's own conformance suite uses — `test`,
//! `assert`, `assertEquals`, `assertThrows`, `assertRejects` — because a
//! developer reading the runtime's tests and writing their own should not have
//! to learn two vocabularies.
//!
//! **`assertEquals` compares structurally, not through `JSON.stringify`.** It
//! used to do the latter, which was wrong in a way that mattered on this
//! runtime specifically: `JSON.stringify` *throws* on a `BigInt`, so the one
//! assertion an int64 test most needs to make could not be written at all, and
//! a `Uint8Array` stringified to `{"0":1,"1":2}` rather than comparing as
//! bytes. It was also order-sensitive on object keys, which no equality test
//! wants. The comparison here walks the values: `BigInt` and `NaN` through
//! `Object.is`, typed arrays and `ArrayBuffer` byte by byte, `Map`/`Set` by
//! contents, `Date`/`RegExp`/`Error` by what identifies them, objects by their
//! key *set* — and it remembers pairs it is already comparing, so a cyclic
//! structure terminates instead of blowing the stack.
//!
//! **`assertThrows`/`assertRejects` take what they expect, not a label.** The
//! second argument used to be the message printed on failure, which meant the
//! natural thing to write — `assertThrows(fn, "TypeError")` — asserted
//! *nothing* about the error: any throw passed. Every call site in this
//! repository was already written that way, so the argument now matches. A
//! string matches the error's `name` or a substring of its message, a `RegExp`
//! tests the message, and a constructor is an `instanceof` check. The failure
//! message, when one is wanted, moved to the third argument.

use std::path::{Path, PathBuf};

use es_runtime_cli_common::run::SourceTransform;

use crate::transform::TypeStripper;

/// What `esdev test` was asked to do.
pub struct TestConfig {
    /// Run exactly this file, harness installed. This is what the parent
    /// invokes for each child, and it is a supported way to run one file
    /// directly.
    pub file: Option<String>,
    /// Substring filters on the discovered paths; empty means all of them.
    pub filters: Vec<String>,
}

/// Suffixes that make a file a test.
const TEST_SUFFIXES: &[&str] = &[
    ".test.js",
    ".test.mjs",
    ".test.ts",
    ".test.tsx",
    ".test.jsx",
    ".test.mts",
];

/// Directories discovery never descends into.
const SKIP_DIRS: &[&str] = &["node_modules", ".git", "dist", "target", ".cache"];

/// The globals a test file is handed, prepended to its own source.
///
/// Kept deliberately small and synchronous to define: it runs at the top of the
/// user's module body, before their first `test(...)` call.
///
/// **Written without line comments on purpose.** It is injected as a single
/// line (see [`one_line`]) so that the user's line 1 stays line 1 — a stack
/// frame in a failing test has to point at the line they wrote, and prepending
/// thirty lines of harness would move every one of them. A `//` comment would
/// swallow the rest of the file once the newlines are gone.
const HARNESS: &str = r#"
globalThis.__esdev = { pending: [], pass: 0, fail: 0, failures: [] };
globalThis.test = (name, fn) => {
  const run = (async () => {
    try {
      await fn();
      globalThis.__esdev.pass++;
    } catch (e) {
      globalThis.__esdev.fail++;
      globalThis.__esdev.failures.push([name, e && e.stack ? e.stack : String(e)]);
    }
  })();
  globalThis.__esdev.pending.push(run);
};
globalThis.assert = (cond, msg) => {
  if (!cond) throw new Error(msg || "assertion failed");
};
const __esdevKeys = (o) => Object.keys(o).filter((k) => o[k] !== undefined);
const __esdevEq = (a, b, seen) => {
  if (Object.is(a, b)) return true;
  if (typeof a !== typeof b) return false;
  if (a === null || b === null || typeof a !== "object") return false;
  const tag = Object.prototype.toString.call(a);
  if (tag !== Object.prototype.toString.call(b)) return false;
  if (a instanceof Date) return a.getTime() === b.getTime();
  if (a instanceof RegExp) return a.source === b.source && a.flags === b.flags;
  if (a instanceof Error) return a.name === b.name && a.message === b.message;
  for (const pair of seen) if (pair[0] === a && pair[1] === b) return true;
  seen.push([a, b]);
  if (a instanceof ArrayBuffer) return __esdevBytes(new Uint8Array(a), new Uint8Array(b));
  if (ArrayBuffer.isView(a)) {
    return __esdevBytes(
      new Uint8Array(a.buffer, a.byteOffset, a.byteLength),
      new Uint8Array(b.buffer, b.byteOffset, b.byteLength),
    );
  }
  if (a instanceof Map) {
    if (a.size !== b.size) return false;
    for (const [k, v] of a) {
      if (b.has(k)) {
        if (!__esdevEq(v, b.get(k), seen)) return false;
        continue;
      }
      let found = false;
      for (const [k2, v2] of b) {
        if (__esdevEq(k, k2, seen) && __esdevEq(v, v2, seen)) { found = true; break; }
      }
      if (!found) return false;
    }
    return true;
  }
  if (a instanceof Set) {
    if (a.size !== b.size) return false;
    for (const v of a) {
      if (b.has(v)) continue;
      let found = false;
      for (const v2 of b) if (__esdevEq(v, v2, seen)) { found = true; break; }
      if (!found) return false;
    }
    return true;
  }
  if (Array.isArray(a)) {
    if (a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (!__esdevEq(a[i], b[i], seen)) return false;
    return true;
  }
  const ka = __esdevKeys(a);
  const kb = __esdevKeys(b);
  if (ka.length !== kb.length) return false;
  for (const k of ka) {
    if (!Object.prototype.hasOwnProperty.call(b, k)) return false;
    if (!__esdevEq(a[k], b[k], seen)) return false;
  }
  return true;
};
const __esdevBytes = (a, b) => {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
};
const __esdevShow = (v) => {
  if (typeof v === "bigint") return String(v) + "n";
  if (v === undefined || typeof v === "symbol" || typeof v === "function") return String(v);
  try {
    const s = JSON.stringify(v, (_k, x) => {
      if (typeof x === "bigint") return String(x) + "n";
      if (ArrayBuffer.isView(x) && !(x instanceof DataView)) return Array.from(x);
      if (x instanceof Map) return Array.from(x);
      if (x instanceof Set) return Array.from(x);
      return x;
    });
    return s === undefined ? String(v) : s;
  } catch {
    return String(v);
  }
};
const __esdevErr = (e) =>
  e && e.name && e.message !== undefined ? e.name + ": " + e.message : String(e);
const __esdevWant = (want) =>
  typeof want === "function" ? want.name || "the expected error" : String(want);
const __esdevMatches = (e, want) => {
  if (want === undefined || want === null) return true;
  const message = e && e.message !== undefined ? String(e.message) : String(e);
  const name = e && e.name ? String(e.name) : "";
  if (typeof want === "string") return name === want || message.includes(want);
  if (want instanceof RegExp) return want.test(message) || want.test(__esdevErr(e));
  if (typeof want === "function") return e instanceof want;
  return false;
};
const __esdevThrew = (e, want, msg, verb, conn) => {
  if (__esdevMatches(e, want)) return;
  throw new Error(
    (msg ? msg + ": " : "") + "expected it to " + verb + " " + conn + __esdevWant(want) +
    ", got " + __esdevErr(e),
  );
};
const __esdevNever = (want, msg, verb, conn) => {
  throw new Error(
    (msg ? msg + ": " : "") + "expected it to " + verb +
    (want === undefined ? "" : " " + conn + __esdevWant(want)) + ", but it did not",
  );
};
globalThis.assertEquals = (actual, expected, msg) => {
  if (__esdevEq(actual, expected, [])) return;
  throw new Error(
    (msg ? msg + ": " : "") +
    "expected " + __esdevShow(expected) + ", got " + __esdevShow(actual),
  );
};
globalThis.assertThrows = (fn, want, msg) => {
  let threw;
  let caught = false;
  try {
    fn();
  } catch (e) {
    threw = e;
    caught = true;
  }
  if (!caught) __esdevNever(want, msg, "throw", "");
  __esdevThrew(threw, want, msg, "throw", "");
};
globalThis.assertRejects = async (fn, want, msg) => {
  let threw;
  let caught = false;
  try {
    await fn();
  } catch (e) {
    threw = e;
    caught = true;
  }
  if (!caught) __esdevNever(want, msg, "reject", "with ");
  __esdevThrew(threw, want, msg, "reject", "with ");
};
"#;

/// Appended after the file's own body: every `test()` registered a promise, and
/// this is where they are awaited and counted. Top-level await is native to
/// modules, so no wrapper is needed.
const EPILOGUE: &str = r#"
await Promise.all(globalThis.__esdev.pending);
for (const [name, stack] of globalThis.__esdev.failures) {
  console.log("  FAIL " + name);
  for (const line of String(stack).split("\n")) console.log("    " + line);
}
console.log(
  "  " + globalThis.__esdev.pass + " passed, " + globalThis.__esdev.fail + " failed",
);
if (globalThis.__esdev.fail > 0) {
  const { exit } = await import("runtime:process");
  exit(1);
}
"#;

/// The harness as one physical line, so injecting it does not renumber the
/// file it is injected into.
fn one_line(source: &str) -> String {
    debug_assert!(
        !source.contains("//"),
        "the harness must carry no line comments: it is injected as one line, \
         and a `//` would comment out the file"
    );
    source.replace('\n', " ")
}

/// Wraps the entry file in the harness, and strips types from everything.
///
/// Only the entry is wrapped: a module the test file imports is ordinary code
/// and must not grow a second copy of the globals or a second epilogue.
pub struct TestTransform {
    /// The entry's canonical `file:` specifier.
    entry: String,
    inner: TypeStripper,
}

impl TestTransform {
    /// Builds a transform that wraps `entry` — given as a filesystem path,
    /// converted here to the `file:` URL the loader will report.
    pub fn new(entry: &Path) -> Self {
        let entry = entry
            .canonicalize()
            .ok()
            .and_then(|p| url_of(&p))
            .unwrap_or_default();
        Self {
            entry,
            inner: TypeStripper,
        }
    }
}

/// A `file:` URL for `path`, or `None` if it cannot be expressed as one.
fn url_of(path: &Path) -> Option<String> {
    url_from_path(path)
}

#[cfg(unix)]
fn url_from_path(path: &Path) -> Option<String> {
    Some(format!("file://{}", path.to_str()?))
}

#[cfg(not(unix))]
fn url_from_path(path: &Path) -> Option<String> {
    Some(format!("file:///{}", path.to_str()?.replace('\\', "/")))
}

impl SourceTransform for TestTransform {
    fn transform(&self, specifier: &str, source: String) -> Result<String, String> {
        // Strip first, wrap second. The order matters and used to be the other
        // way round: the stripper re-prints the AST through oxc's codegen, which
        // does not preserve line positions, so a harness folded onto one line
        // came back out as one statement per line — and every line number in a
        // `.ts` stack trace was pushed down by the length of the harness. With
        // the wrap applied afterwards the harness never reaches the printer, and
        // stays the single line it was folded into.
        let source = self.inner.transform(specifier, source)?;
        if specifier != self.entry {
            return Ok(source);
        }
        // The harness goes at the top of the *body*. Imports are hoisted and
        // their modules evaluate first regardless, so a `test(...)` call in the
        // body still sees the globals.
        // No newline between the harness and the source: the user's first line
        // must remain line 1. Only its columns shift, and the epilogue goes
        // after everything they wrote, so every other line number is the one
        // the printer produced for their code alone.
        Ok(format!("{}{source}\n{EPILOGUE}", one_line(HARNESS)))
    }
}

/// Every test file under `root`, sorted, honouring `filters`.
pub fn discover(root: &Path, filters: &[String]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect(root, &mut found);
    found.retain(|p| {
        let text = p.to_string_lossy().into_owned();
        filters.is_empty() || filters.iter().any(|f| text.contains(f.as_str()))
    });
    found.sort();
    found
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                collect(&path, out);
            }
        } else if is_test_file(&name) {
            out.push(path);
        }
    }
}

/// Whether a filename names a test.
pub fn is_test_file(name: &str) -> bool {
    TEST_SUFFIXES.iter().any(|s| name.ends_with(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_files_are_recognised_by_suffix() {
        assert!(is_test_file("app.test.ts"));
        assert!(is_test_file("app.test.mjs"));
        assert!(is_test_file("deep.name.test.tsx"));
        assert!(!is_test_file("app.ts"));
        assert!(!is_test_file("testing.ts"));
        // `test.ts` alone is a module named "test", not a test file: the suffix
        // is `.test.<ext>`, and treating a bare name as a test would sweep in
        // ordinary source.
        assert!(!is_test_file("test.ts"));
    }

    /// Only the entry gets the harness. A helper the test file imports is
    /// ordinary code, and a second epilogue in it would report a second time.
    #[test]
    fn only_the_entry_is_wrapped() {
        let t = TestTransform {
            entry: "file:///p/a.test.mjs".to_string(),
            inner: TypeStripper,
        };
        let wrapped = t
            .transform("file:///p/a.test.mjs", "test('x', () => {});".into())
            .unwrap();
        assert!(wrapped.contains("globalThis.test"), "{wrapped}");
        assert!(wrapped.contains("__esdev.pending"), "{wrapped}");

        let plain = t
            .transform("file:///p/helper.mjs", "export const x = 1;".into())
            .unwrap();
        assert!(!plain.contains("globalThis.test"), "{plain}");
        assert_eq!(plain, "export const x = 1;");
    }

    /// The wrapped entry still goes through the type stripper — a `.test.ts`
    /// file is the ordinary case, not an exception.
    #[test]
    fn a_typescript_test_file_is_still_stripped() {
        let t = TestTransform {
            entry: "file:///p/a.test.ts".to_string(),
            inner: TypeStripper,
        };
        let out = t
            .transform(
                "file:///p/a.test.ts",
                "const n: number = 1;\ntest('x', () => assert(n === 1));".into(),
            )
            .unwrap();
        assert!(!out.contains(": number"), "{out}");
        assert!(out.contains("globalThis.test"), "{out}");
    }

    /// A failing assertion has to name the line the developer wrote. The
    /// harness is injected ahead of their code, so this is the property that
    /// keeps it from moving every line number in the file.
    #[test]
    fn wrapping_does_not_renumber_the_users_lines() {
        let t = TestTransform {
            entry: "file:///p/a.test.mjs".to_string(),
            inner: TypeStripper,
        };
        let source = "test('x', () => {\n  assert(false);\n});\n";
        let wrapped = t.transform("file:///p/a.test.mjs", source.into()).unwrap();

        // `assert(false)` is on line 2 of the source, and must still be on line
        // 2 of what the engine compiles.
        let line_of = |needle: &str, text: &str| {
            text.lines()
                .position(|l| l.contains(needle))
                .map(|i| i + 1)
                .expect("found")
        };
        assert_eq!(line_of("assert(false)", source), 2);
        assert_eq!(line_of("assert(false)", &wrapped), 2);
    }

    #[test]
    fn the_harness_has_no_line_comments() {
        // It is injected as one line; a `//` would comment out the file.
        assert!(
            !HARNESS.contains("//"),
            "the harness must have no // comments"
        );
    }

    #[test]
    fn discovery_skips_machine_written_directories() {
        let dir = std::env::temp_dir().join(format!("esdev-discover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("mkdir");
        std::fs::create_dir_all(dir.join("node_modules/pkg")).expect("mkdir");
        std::fs::write(dir.join("src/a.test.mjs"), "").expect("write");
        std::fs::write(dir.join("src/b.mjs"), "").expect("write");
        std::fs::write(dir.join("node_modules/pkg/c.test.mjs"), "").expect("write");

        let found = discover(&dir, &[]);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].ends_with("a.test.mjs"), "{found:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn filters_match_on_the_path() {
        let dir = std::env::temp_dir().join(format!("esdev-filter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("alpha.test.mjs"), "").expect("write");
        std::fs::write(dir.join("beta.test.mjs"), "").expect("write");

        assert_eq!(discover(&dir, &["alpha".to_string()]).len(), 1);
        assert_eq!(discover(&dir, &["nope".to_string()]).len(), 0);
        assert_eq!(discover(&dir, &[]).len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
