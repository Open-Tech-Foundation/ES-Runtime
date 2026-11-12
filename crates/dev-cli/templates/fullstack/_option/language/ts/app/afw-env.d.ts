/** AFW compiler macros — provided at build time, not runtime. */
declare const $state: {
  <T>(initial: T): T;
  <T>(): T | undefined;
};
declare function $derived<T>(fn: () => T): T;
declare function $effect(fn: () => void | (() => void)): void;
