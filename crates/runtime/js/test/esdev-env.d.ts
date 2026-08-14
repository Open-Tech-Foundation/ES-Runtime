/**
 * The globals `esdev test` injects into every `*.test.ts` file.
 *
 * There is no import: the runner prepends these to the file's own source, which
 * is what lets a test file stay an ordinary module that runs unbundled and
 * keeps its own path and module resolution.
 *
 * The same declarations `esdev create` writes into a new project's
 * `src/esdev-env.d.ts`. They live here rather than coming from a package
 * because this directory is not a published package — it is the source of the
 * `runtime:serialization` module, bundled into the runtime binary.
 */

declare function test(name: string, fn: () => void | Promise<void>): void;

/** Fails the test unless `condition` is truthy. */
declare function assert(condition: unknown, message?: string): asserts condition;

/**
 * Fails the test unless `actual` deep-equals `expected`.
 *
 * Structural, not `JSON.stringify`: `BigInt` and `NaN` compare, typed arrays
 * and `ArrayBuffer` compare as bytes, `Map` and `Set` by contents, objects by
 * their key set rather than key order, and a cyclic structure terminates.
 */
declare function assertEquals<T>(actual: T, expected: T, message?: string): void;

/**
 * What a thrown error is checked against: a string (the error's `name`, or a
 * substring of its message), a `RegExp` tested against the message, or a
 * constructor for an `instanceof` check.
 */
type ExpectedError = string | RegExp | (new (...args: never[]) => Error);

/** Fails the test unless `fn` throws, and unless the error matches `expected`. */
declare function assertThrows(fn: () => unknown, expected?: ExpectedError, message?: string): void;

/** Fails the test unless `fn`'s promise rejects, and unless the error matches `expected`. */
declare function assertRejects(
  fn: () => Promise<unknown>,
  expected?: ExpectedError,
  message?: string,
): Promise<void>;
