// What a **server-side** runtime is measured against.
//
// WPT is written for browsers. A test that needs a document, a renderer, a
// browsing context or browser storage is not a failure here — it is not a test
// of this runtime at all, and counting it as a failure would make the score
// measure the wrong thing in both directions: it hides real deviations under
// noise, and it can never reach 100%.
//
// So each entry below is an explicit, reasoned exclusion. The rule for adding
// one: the test must be inapplicable **by design**, traceable to a decision
// already recorded (SPEC non-goals, DECISIONS, API.md "Scope & non-goals") —
// never merely "we do not implement this yet". Anything unimplemented but
// legitimately server-side stays in `runnable` and counts as a failure until it
// is fixed. That is the whole point of the number.
//
// Judgment calls are deliberately **not** here, and are counted as failures:
// `location`/`WorkerLocation`, `WorkerNavigator`, `WorkerGlobalScope` and
// `DedicatedWorkerGlobalScope` (Deno exposes all four in a module worker),
// `EventSource`, `FileReader`, `data:`/`blob:` worker URLs, and growable
// `SharedArrayBuffer`.

// Interfaces that belong to a rendering engine, a document, or browser-local
// storage. None of these exist in Deno's workers either.
const BROWSER_ONLY = [
  // Rendering / canvas
  "ImageData",
  "ImageBitmap",
  "OffscreenCanvas",
  "CanvasGradient",
  "CanvasPattern",
  "TextMetrics",
  "Path2D",
  // The legacy HTTP client, replaced by fetch
  "XMLHttpRequest",
  "XMLHttpRequestEventTarget",
  "XMLHttpRequestUpload",
  // Browser-local databases
  "IDBRequest",
  "IDBOpenDBRequest",
  "IDBVersionChangeEvent",
  "IDBFactory",
  "IDBDatabase",
  "IDBObjectStore",
  "IDBIndex",
  "IDBKeyRange",
  "IDBCursor",
  "IDBCursorWithValue",
  "IDBTransaction",
  // A <input type=file> selection
  "FileList",
];

/** Whole files that cannot apply. Each returns the reason it is excluded. */
export const FILES = [
  {
    match: /^workers\/importscripts_mime(_local)?\.any\.js$/,
    reason: "importScripts: every input is a module, so there is no classic-script path (SPEC §8, D48)",
  },
  {
    match: /^workers\/interfaces\/WorkerUtils\/importScripts\//,
    reason: "importScripts: module-only runtime (SPEC §8, D48)",
  },
  {
    match: /^workers\/nested_worker_importScripts\.worker\.js$/,
    reason: "importScripts: module-only runtime (SPEC §8, D48)",
  },
  {
    match: /^workers\/nested_worker_sync_xhr\.worker\.js$/,
    reason: "synchronous XMLHttpRequest: no XHR, and no synchronous I/O at all (D36)",
  },
  {
    match: /^workers\/WorkerGlobalScope_requestAnimationFrame\.worker\.js$/,
    reason: "requestAnimationFrame: a rendering callback, with no frames to align to",
  },
  {
    match: /user-activation\.tentative\./,
    reason: "user activation: a browser input concept, and the spec text is tentative",
  },
  {
    match: /close-event\/garbage-collected\.tentative\./,
    reason: "tentative spec text, and requires script-visible control over GC",
  },
  {
    match: /^workers\/modules\/dedicated-worker-import-(data|blob)-url\.any\.js$/,
    reason:
      "needs WPT's server: every case appends ?pipe=header(Access-Control-Allow-Origin,*) " +
      "and turns on a worker having a null origin — an HTTP-origin test, not a module-worker one",
  },
];

/**
 * Subtests inside files that otherwise apply. `file` narrows the rule to one
 * path when the same wording means different things elsewhere.
 */
export const SUBTESTS = [
  {
    match: new RegExp(`^The (${BROWSER_ONLY.join("|")}) interface object should (not )?be exposed\\.$`),
    reason: "browser-only interface: rendering, documents or browser-local storage",
  },
  {
    match: new RegExp(`^existence of (${BROWSER_ONLY.join("|")})$`),
    reason: "browser-only interface: rendering, documents or browser-local storage",
  },
  {
    file: /^workers\/semantics\/interface-objects\//,
    match: /^The (FileReaderSync|SharedWorker\w*) interface object should be exposed\.$/,
    reason: "FileReaderSync is synchronous I/O (D36); SharedWorker needs documents to share between",
  },
  {
    match: /^SharedWorker exposure$/,
    reason: "SharedWorker: its purpose is sharing one worker between documents",
  },
  {
    file: /^workers\/constructors\/Worker\/DedicatedWorkerGlobalScope-members\.worker\.js$/,
    match: /^existence of on(offline|online)$/,
    reason: "online/offline events: a browser's connectivity model, not a server's",
  },
  {
    file: /^workers\/modules\//,
    match: /^Static import \(cross-origin\)\.$/,
    reason: "needs a second origin, and the helper is a `.sub.js` the WPT server rewrites",
  },
  {
    file: /^html\/webappapis\/structured-clone\//,
    match: /^(ImageBitmap|OffscreenCanvas)$/,
    reason: "cloning a canvas object, which needs a rendering engine",
  },
  {
    file: /^workers\/worker-performance\.worker\.js$/,
    match: /^(Resource timing seems to work in workers|performance\.(clearResourceTimings|setResourceTimingBufferSize) in workers)$/,
    reason: "Resource Timing instruments a browser's subresource fetches; User Timing is the part implemented (SPEC §2.11)",
  },
];

/** The reason `path` is out of scope, or `null` if it is a test of this runtime. */
export function fileScope(path) {
  for (const rule of FILES) if (rule.match.test(path)) return rule.reason;
  return null;
}

/** The reason subtest `name` in `path` is out of scope, or `null`. */
export function subtestScope(path, name) {
  for (const rule of SUBTESTS) {
    if (rule.file && !rule.file.test(path)) continue;
    if (rule.match.test(name)) return rule.reason;
  }
  return null;
}
