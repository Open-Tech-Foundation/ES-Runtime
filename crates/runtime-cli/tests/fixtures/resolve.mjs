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
// A bare specifier resolves synchronously through the module loader (D41).
show("PKG", "greeter");
// And the URL it returns is one import() accepts — same module, same instance.
const viaSpecifier = await import("greeter");
const viaResolved = await import(import.meta.resolve("greeter"));
console.log(`PARITY:${viaSpecifier === viaResolved}`);
// A package that is not installed fails, rather than inventing a URL.
show("MISSINGPKG", "no-such-package");
// #private specifiers resolve through this package's own "imports" map.
show("PRIV", "#local");
const message = (specifier) => {
  try {
    import.meta.resolve(specifier);
    return "(resolved)";
  } catch (e) {
    return e.message;
  }
};
console.log(`PRIVMSG:${message("#nope").includes("does not define")}`);
show("NODE", "node:fs");
