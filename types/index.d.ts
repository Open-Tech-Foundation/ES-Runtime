// Type definitions for ES Runtime (esrun) — the `runtime:` standard modules.
// Ambient `declare module` blocks: include this package and editors resolve
// `import … from "runtime:fs"` (and process/path) with full completion + types.
//
// Setup (tsconfig.json):
//   { "compilerOptions": { "types": ["@opentf/esrun-types"] } }
// or a triple-slash reference in one file:
//   /// <reference types="@opentf/esrun-types" />

/// <reference path="./runtime-process.d.ts" />
/// <reference path="./runtime-path.d.ts" />
/// <reference path="./runtime-fs.d.ts" />
/// <reference path="./runtime-net.d.ts" />
/// <reference path="./runtime-http.d.ts" />
/// <reference path="./runtime-serialization.d.ts" />
