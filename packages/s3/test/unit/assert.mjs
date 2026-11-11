// A test harness small enough to own. The unit tests run under `esrun` rather
// than a test runner, because the code under test is written for this runtime —
// checking it somewhere else would be checking a different thing.
let failures = 0;
let checks = 0;

export function is(actual, expected, what) {
  checks++;
  const a = typeof actual === "string" ? actual : JSON.stringify(actual);
  const b = typeof expected === "string" ? expected : JSON.stringify(expected);
  if (a !== b) {
    failures++;
    console.log(`  FAIL ${what}\n       expected ${b}\n       actual   ${a}`);
  }
}

export function ok(condition, what) {
  checks++;
  if (!condition) {
    failures++;
    console.log(`  FAIL ${what}`);
  }
}

export async function throws(fn, what) {
  checks++;
  try {
    await fn();
    failures++;
    console.log(`  FAIL ${what} — nothing was thrown`);
  } catch {
    /* expected */
  }
}

export function report(name) {
  console.log(`${failures === 0 ? "ok" : "FAILED"} ${name} (${checks} checks, ${failures} failed)`);
  return failures;
}
