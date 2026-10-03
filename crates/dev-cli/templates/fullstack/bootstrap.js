// Server bootstrap: the framework's shared runtime defines custom elements
// at module scope, so the shim must run before any other module evaluates.
// Static imports evaluate before a module body, so this lives in the entry
// ahead of a dynamic import — not in server.js itself. Remove this file and
// point the server target at server.js once the shared runtime no longer
// evaluates Custom Elements on import (framework limitation, verified
// 2026-10: `@opentf/web/server` re-exports registration, but bundlers inline
// the Custom Element modules behind it into the server output).
globalThis.HTMLElement ??= class {};

await import("./server.js");
