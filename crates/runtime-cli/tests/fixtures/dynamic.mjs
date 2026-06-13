// Dynamic import() of a relative module, with top-level await.
const m = await import("./greet.mjs");
console.log(m.greet(m.NAME));

// And a node_modules package imported dynamically.
const pkg = await import("greeter");
console.log(pkg.hi("dynamic"));
