import { greet, NAME } from "./greet.mjs";

console.log(greet(NAME));
console.log("URL:" + import.meta.url);

// Top-level await is native to modules.
const v = await Promise.resolve(42);
console.log("AWAITED:" + v);
