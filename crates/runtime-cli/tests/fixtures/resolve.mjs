// import.meta.resolve: pure URL resolution against this module, with no I/O
// and no check that the target exists.
const show = (label, specifier) => {
  try {
    console.log(`${label}:${import.meta.resolve(specifier)}`);
  } catch (e) {
    console.log(`${label}!${e.name}`);
  }
};

console.log(`TYPE:${typeof import.meta.resolve}`);
// Relative and parent paths resolve against import.meta.url.
show("REL", "./greet.mjs");
show("PARENT", "../up.mjs");
show("ABS", "/abs/z.mjs");
// An absolute URL resolves to itself, including the runtime: scheme.
show("URL", "file:///q.mjs");
show("BUILTIN", "runtime:process");
// No existence check: resolving a path and importing it are separate questions.
show("MISSING", "./definitely-not-here.mjs");
// A bare specifier needs node_modules — host I/O — and resolve is synchronous.
show("BARE", "some-package");
// A #private specifier needs the referring package's "imports" map, and says so
// rather than calling itself bare.
const message = (specifier) => {
  try {
    import.meta.resolve(specifier);
    return "(resolved)";
  } catch (e) {
    return e.message;
  }
};
console.log(`PRIVATE:${message("#config").includes("private specifier")}`);
console.log(`BAREMSG:${message("some-package").includes("bare specifier")}`);
show("PRIV", "#config");
show("NODE", "node:fs");
