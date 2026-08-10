// Builds src/ into dist/ — plain ESM plus declarations, module for module.
//
// Not bundled. A library that ships its module structure is easier to read a
// stack trace from, and `runtime:` specifiers pass through untouched because
// nothing tries to follow them: they are resolved by the runtime, and a bundler
// asked to look would fail rather than leave them alone.
//
// It also means the protocol modules can be imported one at a time, which is
// what the unit tests do — the alternative was exporting the internals from the
// package's public surface so a test could reach them, which is a poor reason to
// widen an API.
import { $ } from "bun";

await $`tsc`;
console.log("built dist/");
