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
