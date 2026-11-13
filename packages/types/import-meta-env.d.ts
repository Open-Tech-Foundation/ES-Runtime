// `import.meta.env`: the values `esdev build`, `esdev start` and the
// `runtime:build` API compile into a bundle (DECISIONS D85), and `esdev test`
// into each module it runs (D226).
//
// Compiled in, never read at run time: code `esrun` runs unbundled, or
// `esdev` runs as a program, has no `import.meta.env`. A deployment's
// configuration and secrets are read at run time through `runtime:process`,
// under `--allow-env`.

/**
 * What a bundle was built with. Only variables named `PUBLIC_*` are compiled
 * in, from the project's `.env` and then the environment; a name the build was
 * not given reads `undefined`.
 *
 * Interfaces merge, so a project can name its own variables:
 *
 * ```ts
 * interface ImportMetaEnv {
 *   readonly PUBLIC_API_URL: string;
 * }
 * ```
 */
interface ImportMetaEnv {
  /** `"development"` under `esdev start`, `"production"` in a release build, `"test"` under `esdev test`. */
  readonly MODE: "development" | "production" | "test";
  /** `true` under `esdev start` and `esdev test`. */
  readonly DEV: boolean;
  /** `true` in a release build. */
  readonly PROD: boolean;
  /** A `PUBLIC_*` variable the build was given, or `undefined`. */
  readonly [name: `PUBLIC_${string}`]: string | undefined;
}

interface ImportMeta {
  /** The values compiled into this bundle ({@link ImportMetaEnv}). */
  readonly env: ImportMetaEnv;
}
