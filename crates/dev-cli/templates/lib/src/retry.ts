/**
 * Retrying something that fails intermittently.
 *
 * The second half of the example, and the one with behaviour worth testing:
 * timing, backoff, and the two ways a retry loop is usually got wrong.
 */

/** How [`retry`] should behave. */
export type RetryOptions = {
  /** How many times to call `fn` in total, including the first. Default 3. */
  attempts?: number;
  /** Milliseconds before the second attempt. Doubles each time. Default 100. */
  delay?: number;
  /** A ceiling on that doubling, so a long retry does not sleep for an hour. */
  maxDelay?: number;
  /**
   * Whether a given failure is worth retrying. Default: everything is.
   *
   * The option that makes this usable: retrying a 400 is pointless, and
   * retrying a 503 is the entire idea.
   */
  retryable?: (error: unknown) => boolean;
  /** Aborts the whole thing, including a sleep already in progress. */
  signal?: AbortSignal;
};

/**
 * Calls `fn` until it succeeds or the attempts run out.
 *
 * Rethrows the **last** error, not the first. A caller shown the first is told
 * about a transient failure that has since been superseded, which is the wrong
 * one to put in a log.
 */
export async function retry<T>(
  fn: () => Promise<T>,
  options: RetryOptions = {},
): Promise<T> {
  const attempts = options.attempts ?? 3;
  const maxDelay = options.maxDelay ?? 30_000;
  const retryable = options.retryable ?? (() => true);
  let delay = options.delay ?? 100;
  let last: unknown;

  for (let attempt = 1; attempt <= attempts; attempt++) {
    options.signal?.throwIfAborted();
    try {
      return await fn();
    } catch (error) {
      last = error;
      // Two reasons to stop early, and both matter: no attempts left, and a
      // failure that will fail the same way however often it is retried.
      if (attempt === attempts || !retryable(error)) {
        throw error;
      }
      await sleep(delay, options.signal);
      delay = Math.min(delay * 2, maxDelay);
    }
  }

  // Unreachable: the loop either returns or throws. Here so the function has a
  // single exit type rather than an implicit `undefined` the caller must widen
  // its own types around.
  throw last;
}

/** A cancellable pause. */
function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(signal.reason);
      return;
    }
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    // Without this, aborting during a backoff waits out the sleep before it is
    // noticed — which on the last, longest delay is most of the total wait.
    const onAbort = () => {
      clearTimeout(timer);
      reject(signal?.reason);
    };
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}
