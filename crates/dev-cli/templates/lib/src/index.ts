/**
 * The package's public surface.
 *
 * Everything a consumer can reach is re-exported from here, and `package.json`
 * points `exports` at this file alone. That is the boundary: a module not named
 * here is internal, and can be renamed or deleted without a major version,
 * because nothing could have imported it.
 *
 * Types are re-exported with `export type`, which `verbatimModuleSyntax`
 * requires and which is right anyway: it says at the import site that nothing
 * is emitted, so a bundler need not resolve the module to know it can be
 * erased.
 */
export { attempt, err, ok } from "./result.ts";
export type { Err, Ok, Result } from "./result.ts";

export { retry } from "./retry.ts";
export type { RetryOptions } from "./retry.ts";
