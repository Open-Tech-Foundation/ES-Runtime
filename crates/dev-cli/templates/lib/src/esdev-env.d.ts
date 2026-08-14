/**
 * What esdev puts into a file that TypeScript cannot see for itself.
 *
 * Not from `esdev --install-types` — that installs the `runtime:` modules,
 * which are what the *runtime* provides. These come from the build and test
 * tooling, so they are declared with the project that uses them.
 */

/**
 * `process.env.NODE_ENV`, which `esdev build` replaces with a literal before
 * the bundler runs.
 *
 * There is no `process` global on this runtime — `runtime:process` is where the
 * real environment lives. This is a *compile-time constant* and nothing more,
 * which is why it is declared as one.
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
