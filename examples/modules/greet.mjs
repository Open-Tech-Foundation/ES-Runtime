// A dependency module: named exports consumed by main.mjs.
export const RUNTIME = "ES-Runtime";

export function greet(name) {
  return `Hello, ${name}, from an ES module 👋`;
}
