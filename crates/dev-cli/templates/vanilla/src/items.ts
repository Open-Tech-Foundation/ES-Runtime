/**
 * The state's shape, and the pure functions over it.
 *
 * Separate from `main.ts` because this is the part worth testing: it imports
 * nothing and touches no DOM, so `esdev test` can run it directly. Anything
 * that reaches `document` cannot be tested here — there is no DOM in the
 * runtime — which is a good reason to keep the logic out of the rendering.
 */

export type Item = {
  id: number;
  label: string;
};

/**
 * An id no item in the list is using.
 *
 * `length + 1` looks equivalent and is not: remove the first of two items and
 * it hands out an id the survivor already has, and then two rows are the same
 * row. Taking one past the largest is the version that survives deletion.
 */
export function nextId(items: readonly Item[]): number {
  return items.reduce((highest, item) => Math.max(highest, item.id), 0) + 1;
}

/** The heading above the list. */
export function formatCount(count: number): string {
  if (count === 0) return "Nothing here yet.";
  return `${count} item${count === 1 ? "" : "s"}.`;
}
