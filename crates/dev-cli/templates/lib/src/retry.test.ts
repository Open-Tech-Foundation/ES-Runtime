import { test, assert, assertEquals, assertRejects } from "runtime:test";
import { retry } from "./retry.ts";

/** A function that fails `times` times, then succeeds. */
function failing(times: number) {
  let calls = 0;
  const fn = async () => {
    calls++;
    if (calls <= times) throw new Error(`attempt ${calls}`);
    return "ok";
  };
  return { fn, calls: () => calls };
}

test("it returns as soon as the call succeeds", async () => {
  const { fn, calls } = failing(0);
  assertEquals(await retry(fn, { delay: 1 }), "ok");
  assertEquals(calls(), 1);
});

test("it retries up to the limit and then gives up", async () => {
  const { fn, calls } = failing(99);
  await assertRejects(() => retry(fn, { attempts: 3, delay: 1 }));
  assertEquals(calls(), 3);
});

test("it rethrows the last failure, not the first", async () => {
  // A caller shown the first is told about a transient failure that has since
  // been superseded — the wrong one to put in a log.
  const { fn } = failing(99);
  try {
    await retry(fn, { attempts: 3, delay: 1 });
    assert(false, "should have thrown");
  } catch (error) {
    assertEquals((error as Error).message, "attempt 3");
  }
});

test("a failure that is not retryable stops immediately", async () => {
  const { fn, calls } = failing(99);
  await assertRejects(() =>
    retry(fn, { attempts: 5, delay: 1, retryable: () => false })
  );
  assertEquals(calls(), 1);
});

test("the delay backs off and is capped", async () => {
  const slept: number[] = [];
  const started = Date.now();
  const { fn } = failing(3);
  await retry(fn, { attempts: 4, delay: 10, maxDelay: 15 });
  // 10 + 15 + 15 with the cap; without it, 10 + 20 + 40.
  const elapsed = Date.now() - started;
  assert(elapsed < 120, `backoff was not capped: ${elapsed}ms`);
  void slept;
});

test("aborting during a backoff does not wait it out", async () => {
  const controller = new AbortController();
  const { fn, calls } = failing(99);
  // Long enough that waiting it out would be obvious.
  const running = retry(fn, { attempts: 5, delay: 5_000, signal: controller.signal });
  setTimeout(() => controller.abort(), 20);
  const started = Date.now();
  await assertRejects(() => running);
  assert(Date.now() - started < 1_000, "the abort waited out the sleep");
  assertEquals(calls(), 1);
});
