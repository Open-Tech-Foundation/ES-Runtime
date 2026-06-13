// export type OrderType = 'asc' | 'desc';
/**
 * Sorts an array of items.
 *
 * @param {T[]} arr The source array.
 * @param {string} [order='asc'] The sort order ('asc' or 'desc').
 * @returns {T[]} A new sorted array.
 *
 * @example
 * sort([1, 3, 2]) //=> [1, 2, 3]
 * sort(['x', 'z', 'y'], 'desc') //=> ['z', 'y', 'x']
 */
export default function sort(arr = [], order = "asc") {
  return [...arr].sort((a, b) => {
    const val = a < b ? -1 : a > b ? 1 : 0;

    return order === "asc" ? val : -val;
  });
}
