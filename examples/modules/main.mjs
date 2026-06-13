// ES module entry point. Run with:  esrun examples/modules/main.mjs
//
// Demonstrates static import/export, import.meta.url, and top-level await —
// all native to modules (no async wrapper). Imports resolve as local files
// relative to this module.
import { greet, RUNTIME } from "./greet.mjs";

console.log(greet(RUNTIME));
console.log("module url:", import.meta.url);

// Top-level await: no IIFE needed.
const bytes = new TextEncoder().encode("esm");
const digest = await crypto.subtle.digest("SHA-256", bytes);
console.log("SHA-256 first byte of 'esm':", new Uint8Array(digest)[0]);
