/**
 * A result that carries either a value or a failure, without throwing.
 *
 * Stands in for whatever this package is actually for. What is worth keeping is
 * the shape it demonstrates, because `esdev build --lib` cares about it:
 *
 * * **Every export is annotated.** The `.d.ts` files are derived from your own
 *   annotations and never inferred, which is what makes emitting them fast and
 *   exact. A function whose return type is left to inference cannot be emitted,
 *   and the build says so rather than guessing.
 * * **No default export.** A named export can be found by a consumer's editor,
 *   renamed by a refactor, and tree-shaken when unused.
 */

/** A successful result. */
export type Ok<T> = {
  readonly ok: true;
  readonly value: T;
};

/** A failed result. */
export type Err<E> = {
  readonly ok: false;
  readonly error: E;
};

/**
 * Either an [`Ok`] or an [`Err`].
 *
 * A union rather than a class, so narrowing it is `if (result.ok)` — which
 * every editor understands and no documentation has to explain.
 */
export type Result<T, E = Error> = Ok<T> | Err<E>;

/** Wraps a value as a success. */
export function ok<T>(value: T): Ok<T> {
  return { ok: true, value };
}

/** Wraps a failure. */
export function err<E>(error: E): Err<E> {
  return { ok: false, error };
}

/**
 * Runs `fn`, returning its value or whatever it threw.
 *
 * The one place a `throw` becomes a `Result`, so a caller can stay in one style
 * rather than mixing both.
 */
export function attempt<T>(fn: () => T): Result<T> {
  try {
    return ok(fn());
  } catch (error) {
    return err(error instanceof Error ? error : new Error(String(error)));
  }
}
