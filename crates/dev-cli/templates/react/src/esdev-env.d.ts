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
 * A CSS Module — `import styles from "./Button.module.css"`.
 *
 * The build reads the stylesheet, rewrites its class names to be unique to that
 * file, and replaces the import with the mapping. `styles.button` is the real
 * class name to put in `className`.
 *
 * The value type is `string` rather than a per-file union of the names actually
 * declared: generating that would mean running the CSS pipeline from the type
 * checker, and `esdev` erases types rather than participating in them. A typo
 * gives `undefined` at runtime and an element with no class.
 */
declare module "*.module.css" {
  const styles: Readonly<Record<string, string>>;
  export default styles;
}

/**
 * The globals `esdev test` injects into every `*.test.ts` file.
 *
 * There is no import: the runner wraps each file with these already defined,
 * which is what lets a test file be an ordinary module that runs unbundled.
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

/**
 * React's Fast Refresh runtime, which ships no types of its own.
 *
 * `import.meta.hot` is **not** declared here — it comes from
 * `@opentf/esrun-types`, which this project already lists in `types`. Declaring
 * it twice is a conflict, not a duplicate.
 *
 * Only what `src/refresh.ts` uses: the transform's output reaches the rest
 * through globals rather than by importing them.
 */
declare module "react-refresh/runtime" {
  export function injectIntoGlobalHook(global: unknown): void;
  export function createSignatureFunctionForTransform(): (type: unknown) => unknown;
  export function register(type: unknown, id: string): void;
  export function performReactRefresh(): void;
}
