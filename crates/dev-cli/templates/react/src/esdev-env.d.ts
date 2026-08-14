/**
 * What esdev puts into a file that TypeScript cannot see for itself.
 *
 * Two kinds of thing, neither of which comes from `esdev --install-types` —
 * that installs the `runtime:` modules, which are what the *runtime* provides.
 * These are what the *build and test tooling* provides, so they are declared
 * with the project that uses them.
 */

/**
 * `process.env.NODE_ENV`, which `esdev build` replaces with a literal before
 * the bundler runs.
 *
 * There is no `process` global on this runtime — reading `process.env.PORT` at
 * runtime would be `undefined` forever, and `runtime:process` is where the real
 * environment lives. This is a *compile-time constant* and nothing more, which
 * is why it is declared as one: a `const`, not a mutable object.
 *
 * `esdev build` defines it as `"production"` and `esdev start` as
 * `"development"`, so a branch on it is dead code the bundler removes rather
 * than a check that runs.
 */
declare const process: {
  readonly env: {
    readonly NODE_ENV: "development" | "production";
  };
};

/**
 * The globals `esdev test` injects into every `*.test.ts` file.
 *
 * There is no import: the runner wraps each file with these already defined,
 * which is what lets a test file be an ordinary module that runs unbundled.
 */
declare function test(name: string, fn: () => void | Promise<void>): void;

/** Fails the test unless `condition` is truthy. */
declare function assert(condition: unknown, message?: string): asserts condition;

/** Fails the test unless `actual` deep-equals `expected`. */
declare function assertEquals<T>(actual: T, expected: T, message?: string): void;

/** Fails the test unless `fn` throws. */
declare function assertThrows(fn: () => unknown, message?: string): void;

/** Fails the test unless `fn`'s promise rejects. */
declare function assertRejects(fn: () => Promise<unknown>, message?: string): Promise<void>;
