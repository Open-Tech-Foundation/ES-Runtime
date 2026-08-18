/**
 * Stands in for whatever this package is actually for.
 *
 * What is worth keeping is the shape, because `esdev build --lib` cares about
 * it: **every export is annotated**, since the `.d.ts` files are derived from
 * your annotations and never inferred — which is what makes emitting them fast
 * and exact. A return type left to inference is refused rather than guessed.
 *
 * And **no default export**: a named export can be found by a consumer's
 * editor, renamed by a refactor, and dropped by a bundler when unused.
 */

/** Greets somebody by name. */
export function greet(name: string): string {
  return `Hello, ${name}!`;
}
