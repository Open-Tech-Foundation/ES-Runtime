// Dynamic import(): load a module on demand. Run with:
//   esrun examples/modules/dynamic.mjs
//
// import() returns a promise for the module namespace; it resolves after the
// module (and any top-level await in it) has fully evaluated.
console.log("loading greet module on demand…");
const { greet, RUNTIME } = await import("./greet.mjs");
console.log(greet(RUNTIME));

// A conditionally-loaded module, the common use of import():
if (RUNTIME === "ES-Runtime") {
  const extra = await import("./greet.mjs");
  console.log("re-imported is the same instance:", extra.greet === greet);
}
