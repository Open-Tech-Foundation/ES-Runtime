# API Reference

The canonical reference for ES-Runtime's public host APIs. This is the
source of truth (DECISIONS [D27]); the marketing site under `website/app/docs/**`
mirrors it for the web. **A change to any public API updates both.**

ES-Runtime is ESM-only and deny-by-default. Host functionality is exposed as
ES modules under the `runtime:` scheme — never as ambient globals — and each
module's operations are gated on an explicit [`Capability`](#capabilities).

## Contents

- [Scope & non-goals](#scope--non-goals)
- [Web-standard globals](#web-standard-globals)
- [Module resolution](#module-resolution)
- [`WebAssembly`](#webassembly)
- [The `runtime:` scheme](#the-runtime-scheme)
- [Capabilities](#capabilities)
- [`runtime:process`](#runtimeprocess)
- [`runtime:path`](#runtimepath)
- [`runtime:fs`](#runtimefs)
- [`runtime:db`](#runtimedb)
- [`runtime:net`](#runtimenet)
- [`runtime:http`](#runtimehttp)
- [`runtime:websocket`](#runtimewebsocket)
- [`runtime:serialization`](#runtimeserialization)
- [`runtime:hashing`](#runtimehashing)
- [`runtime:wasi`](#runtimewasi)
- [`runtime:system`](#runtimesystem)
- [`runtime:build`](#runtimebuild) — `esdev` only
- [`runtime:test`](#runtimetest) — `esdev` only
- [`runtime:watch`](#runtimewatch) — `esdev` only
- [Error codes](#error-codes)

---

## Scope & non-goals

ES-Runtime is a runtime, not a toolchain, and is **not** a Node.js drop-in.
The following are deliberate, durable boundaries — not unimplemented features:

| Not supported            | Notes                                                              |
| ------------------------ | ------------------------------------------------------------------ |
| Node.js compatibility    | No `node:` builtins, no Node globals (`process`/`Buffer`/`require`). |
| CommonJS                 | ES Modules only — no `require`/`module.exports`, no CJS↔ESM interop. |
| TypeScript               | Runs JavaScript; transpile types ahead of time.                    |
| JSX                      | Not a JS standard; compile ahead of time.                          |
| JSON module imports      | `import x from "./x.json" with { type: "json" }` supported via transpilation. |
| Package installer        | Resolves an existing `node_modules`; does not install.             |
| Bundler / linter / formatter / test runner | Left to dedicated tools.                         |
| Watch mode               | No built-in file watcher / auto-restart.                           |
| FFI / native addons      | Host extends via injected providers + ops (Rust), not FFI.         |
| Node.js `worker_threads` | `Worker` is the HTML interface (module workers only); there is no `isMainThread`. |

See `website/app/docs/scope` for the rendered version.

## Web-standard globals

The global scope tracks the WinterTC Minimum Common Web Platform API. Host
capabilities (filesystem, process, network) are **not** globals — they live in
[`runtime:` modules](#the-runtime-scheme).

- **Core:** `globalThis`, `self`, `console` ([full method set](#console)), `queueMicrotask`, `structuredClone`, `reportError`, `navigator` (`userAgent` — `"ES-Runtime/<version>"` — and `hardwareConcurrency`)
- **Modules:** `import.meta.url`, `import.meta.resolve(specifier)` — pure URL resolution against the current module, with no I/O and no existence check. **Bare and `#private` specifiers resolve too**, through the module loader — useful for locating a file *inside* a dependency (a migration, a `.proto`, a template) whose install path you cannot hardcode. That reads `package.json` files, so it needs the same `FileSystem` grant an import does, and it obeys the same root jail and import policy; a denied run gets a `NotAllowedError` rather than a location.
- **Timers:** `setTimeout`, `clearTimeout`, `setInterval`, `clearInterval`
- **URL:** `URL` (incl. `canParse`, `parse`, and `createObjectURL`/`revokeObjectURL` for in-process `blob:` URLs), `URLSearchParams`, `URLPattern`
- **Fetch:** `fetch`, `Request`, `Response` (incl. `Response.json`/`error`/`redirect`), `Headers` (incl. `getSetCookie`) — a `ReadableStream` request body streams as a chunked upload (response bodies stream too), and all three [redirect modes](#redirects) are honoured
- **Encoding:** `TextEncoder`, `TextDecoder`, `TextEncoderStream`, `TextDecoderStream`, `atob`, `btoa` — `TextDecoder` accepts every label the WHATWG Encoding Standard defines (`utf-8`, `utf-16le`/`be`, `windows-1252`, `shift_jis`, `gb18030`, …), with `fatal`, `ignoreBOM` and streaming decode
- **Streams:** `ReadableStream` (default + byte/BYOB, `ReadableStream.from`, async iteration), `WritableStream`, `TransformStream`, `ByteLengthQueuingStrategy`, `CountQueuingStrategy` (+ controllers/readers)
- **Compression:** `CompressionStream`, `DecompressionStream` — all four spec formats: `"brotli"`, `"gzip"`, `"deflate"` (zlib), `"deflate-raw"`; corrupt/trailing-junk input errors at write, truncated input at close, all as `TypeError`
- **Crypto:** `crypto` (`getRandomValues`, `randomUUID`), `CryptoKey`, `crypto.subtle` — [algorithms below](#cryptosubtle-algorithms)
- **Events:** `Event`, `EventTarget`, `CustomEvent`, `MessageEvent`, `CloseEvent`, `ErrorEvent`, `ProgressEvent`, `PromiseRejectionEvent`, `AbortController`, `AbortSignal` — plus `addEventListener`/`removeEventListener`/`dispatchEvent` on the global scope itself
- **Network:** `WebSocket`, `WebSocketStream`, `WebSocketError` (capability-gated — see below)
- **Data:** `Blob`, `File`, `FormData`, `DOMException`
- **Messaging:** `MessageChannel`, `MessagePort`, `BroadcastChannel` — messages are structured-cloned at `postMessage`, delivered asynchronously and in order, and a port buffers until `start()` (which assigning `onmessage` does implicitly). A `MessagePort` can be **transferred**, including into a worker, which is how you hand one a private channel; it cannot be *cloned* (two ends of a channel cannot become three). A `BroadcastChannel` reaches every agent, as the spec's agent-cluster scope requires, and never its own sender.
- **Workers:** `Worker` — see [Workers](#workers) below.
- **Performance:** `performance` — `now()`, `timeOrigin`, and User Timing (`mark`, `measure`, `getEntries`/`getEntriesByName`/`getEntriesByType`, `clearMarks`, `clearMeasures`), with `PerformanceEntry`, `PerformanceMark`, `PerformanceMeasure`
- **WebAssembly:** `WebAssembly` — `validate`, `compile`, `instantiate`, `compileStreaming`, `instantiateStreaming`, `Module`, `Instance`, `Memory`, `Table`, `Global`, `CompileError`, `LinkError`, `RuntimeError`

### Redirects

`fetch` honours all three `RequestRedirect` modes; the mode reaches the
transport, so a redirect the caller refused is never walked.

| `redirect` | Behaviour |
| --- | --- |
| `"follow"` (default) | Follow the chain, up to the specification's cap of 20. Past it: `ERR_TOO_MANY_REDIRECTS`. |
| `"manual"` | Resolve with the `3xx` itself — status, `Location`, headers intact — and follow nothing. |
| `"error"` | Reject with a `TypeError`. |

An unrecognized value is a `TypeError` from the `Request` constructor rather
than a silent fallback: `redirect` decides whether a `3xx` is followed, so a
typo must not quietly become `"follow"`.

```js
const r = await fetch(url, { redirect: "manual" });
if (r.status === 302) console.log("would go to", r.headers.get("location"));
```

`response.redirected` reports whether at least one redirect was followed, and
`response.url` is where the request actually ended up. Both come from the
transport — a script cannot construct a `Response` that claims either.

**Deviation:** under `"manual"` the specification returns an *opaque-redirect
filtered* response (status `0`, no headers, null body). That exists so a browser
can hand a redirect back to its navigation machinery without leaking
cross-origin data; here there is no navigation and no origin to protect, and it
would make the mode useless — the reason to ask for `"manual"` server-side is to
read `Location`. The real response is returned instead, as Node, Deno and Bun
all do.

### Timeouts

The default transport bounds the **connect** phase — DNS, TCP and TLS — at 30
seconds, failing with `ERR_TIMED_OUT`; pooled connections carry a 60-second TCP
keepalive, so a peer that vanishes without a FIN is not handed to a later
request.

There is deliberately **no cap on the request as a whole**. Fetch defines none,
and a response body may be long-lived by design — server-sent events, a log
tail, a large download — so a total deadline would break correct programs.
Bounding the whole operation is the caller's call, and Fetch already gives them
the tool:

```js
// Rejects with a TimeoutError DOMException if the whole thing takes too long.
await fetch(url, { signal: AbortSignal.timeout(5000) });
```

### Compressed responses

The default transport negotiates and decodes content-codings, so a compressed
response arrives as a body you can read:

| | |
| --- | --- |
| Sent | `Accept-Encoding: gzip, br, deflate` — the same set `CompressionStream` implements |
| Decoded | `gzip`, `br`, `deflate`, keyed off the response's `Content-Encoding` rather than off who asked, so a server that compresses unbidden is still handled |
| Stripped | `Content-Encoding` and `Content-Length`, which described the compressed bytes |
| Passed through | any other coding (`zstd`, …) — body and headers untouched, since claiming to have decoded it would be a lie |

`zstd` is deliberately not implemented: nothing else in the runtime speaks it,
and advertising a coding means carrying a codec that exists for no other reason.

Outbound requests carry `User-Agent: ES-Runtime/<version>` — the same string as
`navigator.userAgent` — unless the request sets its own.

Both are properties of the default `ReqwestTransport`. An embedder that installs
its own `NetTransport` decides for itself.

### `console`

The whole Console Standard method set, routed to the injected `Console` provider
rather than to stdout — an embedder decides where guest output goes.

| Group | Methods |
| --- | --- |
| Output | `log`, `info`, `warn`, `error`, `debug`, `dir`, `dirxml`, `trace` |
| Grouping | `group`, `groupCollapsed`, `groupEnd` — indent subsequent output |
| Counting | `count`, `countReset` |
| Timing | `time`, `timeLog`, `timeEnd` |
| Other | `assert`, `table` (rendered as a table), `clear` |

Format specifiers work as the standard defines them: `%s`, `%d`/`%i`, `%f`,
`%o`/`%O`, `%j`, `%%`, and `%c` (whose argument is consumed and discarded —
there is no styling to apply to a provider sink). Values are inspected
structurally: functions as `[Function: name]`, `Map`/`Set`/`Date`/`RegExp` in
their own notation, cycles as `[Circular]`.

### `crypto.subtle` algorithms

| Operation | Algorithms |
| --- | --- |
| `digest` | SHA-1, SHA-256, SHA-384, SHA-512 |
| `sign` / `verify` | HMAC, Ed25519, ECDSA (P-256/384/521), RSASSA-PKCS1-v1_5, RSA-PSS |
| `encrypt` / `decrypt` | AES-GCM, AES-CBC, AES-CTR, RSA-OAEP |
| `wrapKey` / `unwrapKey` | AES-KW, AES-GCM, AES-CBC, AES-CTR, RSA-OAEP |
| `deriveBits` / `deriveKey` | HKDF, PBKDF2, ECDH, X25519 |
| key formats | `raw`, `spki`, `pkcs8`, `jwk` (symmetric keys as `kty: "oct"`, Ed25519/X25519 as `kty: "OKP"`) |

`AES-KW` wraps key material only — it is not reachable from `encrypt`/`decrypt`,
and its integrity check makes an unwrap of tampered ciphertext fail rather than
return wrong key material. Wrapping a key still requires `extractable: true`
(wrapping *is* an export) and the wrapping key's `wrapKey` usage.

Ed25519 and X25519 are the WebCrypto Secure Curves: one 32-byte key each, with
no curve to choose. X25519 rejects a low-order peer key rather than returning the
all-zero shared secret it would otherwise produce.

### Failures that reach the global scope

A failure with no code left to catch it is reported to the global scope before
the host sees it. A listener that calls `preventDefault()` has taken
responsibility, and the host stays quiet; otherwise `esrun` prints it and exits
non-zero.

| Event | Fired when | Interface | Cancelable |
| --- | --- | --- | --- |
| `error` | an exception escapes a timer callback, or `reportError()` is called | `ErrorEvent` | yes |
| `unhandledrejection` | a promise rejection is still unhandled at the end of a tick | `PromiseRejectionEvent` | yes |
| `rejectionhandled` | a handler is attached to a rejection already reported | `PromiseRejectionEvent` | no |

```js
globalThis.addEventListener("unhandledrejection", (event) => {
  logger.warn("unhandled", event.reason);
  event.preventDefault(); // mine now — do not fail the process
});

globalThis.onerror = (event) => {
  logger.error(event.message, event.error);
  event.preventDefault();
};
```

`onerror`, `onunhandledrejection` and `onrejectionhandled` are single-handler
slots over the same events: assigning twice replaces rather than accumulates.

`rejectionhandled` does **not** retract the report. The report goes out when the
rejection is unclaimed at the end of a tick; attaching a handler afterwards tells
you the report has been superseded, but the process still fails — a rejection
that was unhandled when it mattered is a bug worth surfacing.
**Not available:** `process`/`Buffer`/`require` (Node), `localStorage`/`window`
(browser). `navigator` carries `userAgent` and `hardwareConcurrency` only: the
rest of the browser `Navigator` is document, device and permission surface, and
answering those with plausible constants would make a feature check pass and
then lie.

---

## Module resolution

Specifiers resolve as ES modules only. Relative, absolute-path and `file:`
specifiers are resolved strictly — the extension is part of the name, and nothing
is guessed. Bare specifiers resolve through `node_modules`, honouring the
package's `exports` map; symlinks resolve to their real path (pnpm's store works
as-is), and every resolved module is confined to the project root.

### Conditions

The conditions asserted are **`import` and `default`** — the standard ones, and
only those. No `node`, no `browser`, and no ES-Runtime-specific key: a package
that needs to know which runtime it is on is not a package this runtime is trying
to run, and `default` is the path everything else reaches it by.

Condition keys are matched **in the order the package author wrote them**, not in
a fixed order of our own, and nested condition objects are walked the same way —
a matched branch that resolves to nothing falls through to the next key.

```json
{
  "exports": {
    ".": {
      "node": "./node.js",
      "import": "./esm.mjs",
      "default": "./fallback.mjs"
    }
  }
}
```

`"."` resolves to `./esm.mjs`: `node` is not asserted, `import` is. A package
offering only `require` is CommonJS and is rejected saying so.

Also supported: subpath patterns (`"./fn/*": "./fns/*.mjs"`, longest prefix
wins), array fallbacks (`["./a.mjs", "./b.mjs"]` — the first *valid* target is
used), and `null` targets, which withdraw a subpath. Importing a withdrawn
subpath reports that the author withdrew it, rather than a bare "not found".

### `imports` and self-reference

A `#specifier` resolves through the nearest `package.json`'s `imports` map, whose
targets may be a path in that package, a subpath pattern, or another package. A
package that declares `exports` may also import **itself** by its own `name`, so
an intra-package import resolves to what a consumer would get.

```json
{
  "name": "my-app",
  "type": "module",
  "exports": { "./util": "./src/util.js" },
  "imports": {
    "#config": "./src/config.js",
    "#feat/*": "./src/feat/*.js",
    "#dep": "lodash-es"
  }
}
```

```js
import config from "#config";
import { one } from "#feat/one";
import { chunk } from "#dep";
import { util } from "my-app/util";
```

### What a target may not do

A target (after any `*` substitution) may not contain a `..`, `.` or
`node_modules` path segment, may not be a bare specifier in `exports`, and may
not be a trailing-slash directory mapping. A subpath pattern therefore cannot be
used to walk out of the package that declares it. This is a package-boundary
check; the [project-root jail](#capabilities) applies underneath it either way. A
malformed manifest — an invalid target, or an `exports` object mixing subpath
keys with condition keys — is an error naming the `package.json`, not a silent
resolution failure.

**Not supported:** CommonJS packages, `node:` builtins, remote (`http:`) modules,
and installing anything.

---

## `WebAssembly`

The full [JS API](https://webassembly.github.io/spec/js-api/), needing no
capability — WebAssembly executes inside the isolate and reaches the host only
through imports you pass it, so a module is exactly as privileged as the import
object you hand it, and no more.

```js
const { instance } = await WebAssembly.instantiate(bytes, {
  env: { log: (n) => console.log(n) },
});
instance.exports.add(2, 3); // 5
```

Both the synchronous constructors (`new WebAssembly.Module`,
`new WebAssembly.Instance`) and the promise-returning `compile` / `instantiate`
are available; the async forms compile off-thread and settle on the event loop,
so they need the loop to be running (as `esrun` and any driver do).

`compileStreaming` / `instantiateStreaming` take a `Response` or a promise for
one, requiring a `Content-Type` of `application/wasm` and an ok status —
otherwise the promise rejects with a `TypeError`:

```js
const { instance } = await WebAssembly.instantiateStreaming(
  fetch("https://example.com/add.wasm"),
);
```

They currently buffer the response before compiling rather than compiling as
bytes arrive. Behaviour is identical; only peak memory and time-to-first-byte
differ on large modules.

### ES-module integration

A `.wasm` file can be imported directly; its exports are the module's exports:

```js
import { add } from "./add.wasm";
add(2, 3); // 5
```

An export name that is not a JS identifier still round-trips — reach it off the
namespace:

```js
import * as m from "./add.wasm";
m["weird-name"](1, 1);
```

A wasm *import*'s module half is an ordinary module specifier, resolved through
the same graph as any `import` — so `(import "./env.js" "log" …)` takes `log`
from that file's namespace:

```js
// env.js
export const log = (v) => console.log(v);
```

The module is compiled once per graph, so static and dynamic imports of the same
file share one instance. A malformed `.wasm` fails at load with V8's own
diagnostic, like a syntax error.

**Not yet supported:** source-phase imports (`import source m from "./m.wasm"`)
and the component model.

`SharedArrayBuffer` and `shared: true` memories cross to a worker as **one
allocation**, so `Atomics` between two agents arbitrate the same bytes.
`Atomics.wait` blocks inside a worker, which owns its thread, and throws a
`TypeError` on the agent driving the loop — the ECMAScript agent record's
`[[CanBlock]]`, and the same split HTML makes.

---

## Workers

The HTML [dedicated worker](https://html.spec.whatwg.org/multipage/workers.html):
its own thread, its own isolate, no shared scope. Not part of the WinterTC
Minimum Common API — this follows the HTML Standard, as Deno and Bun do.

```js
// main.js
const worker = new Worker(new URL("./worker.js", import.meta.url), {
  name: "resize",
  permissions: ["net"],                    // see Capabilities below
  env: { API_BASE: "https://example.test" }, // see Environment below
});
worker.onmessage = (e) => console.log(e.data);
worker.onerror = (e) => { console.error(e.error); e.preventDefault(); }; // see Failure
worker.postMessage({ job: 42 });

// worker.js
self.onmessage = (e) => postMessage(`${self.name} did ${e.data.job}`);
```

Inside a worker the global scope really is a `DedicatedWorkerGlobalScope`:
`postMessage`, `onmessage`, `onmessageerror`, `close()` and `name`, on a
prototype chain of `DedicatedWorkerGlobalScope` → `WorkerGlobalScope` →
`EventTarget`. That is also how you tell where you are — there is no
`isMainThread`, which is a Node-ism; HTML, Deno and Bun all distinguish the two
by the shape of the global:

```js
if ("DedicatedWorkerGlobalScope" in self && self instanceof DedicatedWorkerGlobalScope) {
  // in a worker
}
```

`self` is a readonly attribute of that interface, so it cannot be reassigned.

| In a worker | On the agent driving the process |
| --- | --- |
| `WorkerGlobalScope`, `DedicatedWorkerGlobalScope` | — |
| `navigator` is a `WorkerNavigator` | `navigator` is a `Navigator` |
| `location` — a `WorkerLocation` over the worker's own module URL | no `location`: no one script is *the* script |

`location` is read-only in every part, since there is nothing to navigate. It is
the honest way to resolve a sibling file — `new URL("./data.bin", location)` —
and the same set Deno exposes in a module worker.

### Options

| Option | | Description |
| --- | --- | --- |
| `type` | standard | `"module"` only; `"classic"` throws (see below) |
| `name` | standard | The worker's `self.name` |
| `permissions` | **ours** | `"inherit"` or capability names to grant — see [Capabilities](#capabilities-1) |
| `env` | **ours** | `"inherit"` or an object — see [Environment](#environment) |
| `memory` | **ours** | Heap ceiling in **megabytes** — see [Memory](#memory) |
| `credentials` | — | Not supported: it governs how a *classic* script is fetched over HTTP, and a module here comes from a file |

Three of the six are non-standard, and necessarily so: HTML has no notion of a
capability, a worker that could read the whole environment because its parent
could would make deny-by-default stop at the first `new Worker`, and a browser
has no per-agent memory budget to divide because it is not the process.

**Module workers only.** `type: "classic"` throws a `TypeError`. This runtime
evaluates every input as a module, so there is no classic-script path for a
classic worker to use — the same reason `require` is absent.

**Resolving the URL.** A relative string resolves against the *entry* module.
Prefer `new URL("./worker.js", import.meta.url)`, which is exact wherever it is
written, and what Vite, webpack and Deno all recommend.

### Capabilities

A worker starts with **nothing** and is granted capabilities explicitly, by the
[denial vocabulary](#capabilities) names:

```js
new Worker(url, { permissions: ["net", "read"] })
new Worker(url, { permissions: "inherit" })      // everything the parent holds
```

| `permissions` | The worker gets |
| --- | --- |
| omitted | nothing |
| an array | exactly those, still bounded by the parent's set |
| `"inherit"` | everything the parent holds |

Omitting it is **not** `"inherit"` — unlike `env`, where omitting and
`"inherit"` are the same thing. Passing data is not granting authority: a parent
can only hand over values it could already read, whereas a capability the parent
did not name is one it did not mean to give.

An unknown name **throws** rather than being skipped:

```js
new Worker(url, { permissions: ["nett"] })
// TypeError: unknown Worker permission "nett" — expected one of: read, write,
// imports, net, listen, env, run, signals, workers
```

A dropped typo fails closed, which sounds harmless right up until the worker
takes the degraded path forever and the denial surfaces three layers away. This
is the rule `permissions.has()` already follows for the same reason. Deno
accepts an unknown name in `deno: { permissions }` silently.

It can never be granted what its parent lacks, so no chain of spawns widens the
original grant. Spawning at all requires `workers` (`--deny-workers` refuses
it) **and `imports`**, because starting a worker means reading its entry module
and reading a module is what `imports` grants — so `--allow-workers` alone
is refused, and the refusal says which flag to add. Node needs
`--allow-fs-read` alongside `--allow-worker` for the same reason; Deno needs
`--allow-read`. A worker's own **static** `import`s still load — under the parent's authority to
read them, resolved before any of the worker's code runs — so a worker granted
nothing is not limited to a single file.

Dynamic `import()` is different, and needs `imports` at the spawn:

```js
new Worker(url, { permissions: ["imports"] })
```

The two are not the same operation. A static graph is literal specifiers, in
source the parent already read, instantiated with no guest code running.
`import()` computes its specifier at runtime — from a message, from input — so
it reads and *executes* a file chosen while the worker is running, on the
worker's own authority. Gating it is what makes "a worker starts with nothing"
mean anything; the refusal names the spawn, since `--allow-imports` grants it to
the wrong agent.

> This is stricter than Deno, which gives a worker its parent's permissions
> unmodified. Under `esrun`, where the parent normally holds everything, a
> worker holds nothing until you say otherwise.

Nesting is allowed: a worker holding `workers` may start its own, and may only
pass on what it holds. Every level re-applies the same rule against its **own**
set, so a grandchild cannot be handed something its parent lacks even if the
agent driving the process holds it.

### Environment

`env` decides what the worker's `runtime:process` [`env`](#runtimeprocess)
reports:

| `env` | The worker sees | Needs `"env"`? |
| --- | --- | --- |
| omitted, or `"inherit"` | the host environment, still narrowed by the deployment's `--allow-env=<names>` | **yes** |
| `{ … }` | exactly those variables | **no** |
| `{}` | nothing | **no** |

```js
new Worker(url, { env: { DATABASE_URL: unmask(env.DATABASE_URL) } });
```

A handed environment needs no permission because nothing is being granted: a
parent can only pass values it could already read, so this **attenuates** — the
same move `permissions` makes, applied to data rather than authority. It is also
the only way to say "this variable and no other", since `--allow-env` is set by
the deployment rather than at the spawn.

A handed environment **wins** over the host's: a worker holding `env` and given
one reads what it was given. Secret-looking names are re-masked on arrival by
the same convention, so a `Secret` from the parent's own `env` can be passed
straight through and stays a `Secret` on the other side.

Deliberately absent: Node's `SHARE_ENV`. A shared, mutable environment is an
undeclared side channel between agents, and there is a declared one —
`postMessage`. For the same reason a parent's own `env.X = …` is not visible to
its workers: each agent's `env` is seeded from the host snapshot, not from the
parent's object.

### Memory

Each worker gets its own isolate, so each has its own heap ceiling:

```js
new Worker(url, { memory: 64 })   // megabytes, as Node's maxOldGenerationSizeMb is
```

Omitted, it takes the ceiling of the agent that started it. Named, it may only
**lower** that — a worker able to raise its own would leave the parent's meaning
nothing, since anything holding `workers` could step over it by doing the work in
a worker. Exceeding it ends that worker and no other: the parent's `error` fires
with `e.error.name === "ERR_WORKER_OUT_OF_MEMORY"` and a message naming the limit.

The ceiling every agent starts from is `--max-heap=<mb>`, and by default it is
sized from the machine — the container's memory limit when there is one, else the
host's memory. Node and Deno both read physical memory here and miss the cgroup,
which is why deploying either means hardcoding `--max-old-space-size` in a
Dockerfile.

Deno and Bun have no per-worker memory limit at all, so a runaway job there takes
the whole process with it.

### Lifetime

A live worker keeps the process running, as in Node and Deno. It ends when it
calls `close()`, when its work finishes, or when the parent calls `terminate()`.

`unref()` gives up that claim without ending anything:

```js
const w = new Worker(url);
w.unref();          // still running, still delivering; no longer a reason to stay up
w.ref();            // back to keeping the process alive
```

Which is what a pool needs: four idle workers waiting for the next job would
otherwise be four reasons the process never exits. Node and Bun both have this;
Deno has neither.
`terminate()` interrupts the isolate, so it stops a worker spinning in a
synchronous loop or parked in `Atomics.wait`.

`exit()` from `runtime:process` inside a worker ends **that worker**, and does
not set the code the process exits with. `onSignal` is refused inside a worker:
a signal is delivered to the process, and watching one suppresses the default
action, so that belongs to the agent that owns the process.

### Failure

An uncaught exception or unhandled rejection inside a worker fires `error` on
the parent's `Worker` **the tick it happens**, and ends the worker — so an
`error` means "this one is gone", which is the fact a pool restarting on failure
needs. The worker takes responsibility instead by claiming the failure in its
own handler, which is then neither reported nor fatal:

```js
self.addEventListener("error", (e) => {
  postMessage({ failed: currentJob, reason: e.message });
  e.preventDefault();          // absorbed; this worker keeps its next job
});
```

The event the parent gets carries the failure in pieces:

| | |
| --- | --- |
| `e.message` | the message alone — no class prefix, no stack |
| `e.filename`, `e.lineno`, `e.colno` | the throw site; empty and `0` when unknown |
| `e.error` | an `Error` with the worker's `name`, `message` and `stack` |

`e.error` is rebuilt rather than moved — the failure crossed a thread — so it is
the class it was thrown as for the standard classes, and an `Error` carrying the
right `name` for anything else. That is the discriminator that survives regardless:
a `DOMException` is told apart by `"AbortError"`, not by its constructor.

A worker that merely *hears* about a child's failure has not failed itself, so an
unclaimed `error` on a `Worker` goes to the console rather than escalating —
otherwise one leaf failure would take down every ancestor without an `onerror`.

### One port for `https:` and `wss:`

`serve()` above binds a WebSocket server on a port of its own, which is the
simple case. A service that already has an HTTP server does not need a second
port: `upgradeWebSocket(request)` takes over the connection a request arrived
on, the way Node, Deno and Bun all do it.

```js
import { serve } from "runtime:http";
import { upgradeWebSocket, broadcast } from "runtime:websocket";

const room = new Set();

serve({ port: 443, secureTransport: "on", cert, key }, (request) => {
  if (request.headers.get("upgrade") === "websocket") {
    const { response, socket } = upgradeWebSocket(request);
    room.add(socket);
    socket.onmessage = (e) => broadcast(room, e.data);
    socket.onclose = () => room.delete(socket);
    return response;
  }
  return new Response("api");
});
```

Return the `response` to accept the upgrade; return anything else to decline it
and answer the request normally. The `socket` is usable immediately — the
handover completes once the `101` is on the wire, and sends before then are
queued, exactly as they are on a socket that is still connecting.

The connection is an ordinary one: the same `WebSocketConnection` `serve()`
yields, so `broadcast()` reaches upgraded and dedicated-port sockets alike, and
`maxBufferedAmount` applies at its default (8 MiB). The handshake headers are the
host's — `Sec-WebSocket-Accept` is a digest of a key the handler never sees.

`upgradeWebSocket` throws a `TypeError` for a `Request` that did not come from a
`serve()` handler; there is no connection behind it to take over.

**Subprotocols.** The client's offer is an ordinary header, so read it and answer
with one of them:

```js
const offered = request.headers.get("sec-websocket-protocol") ?? "";
const { response, socket } = upgradeWebSocket(request, {
  protocol: offered.split(",").map((s) => s.trim()).includes("chat.v2") ? "chat.v2" : undefined,
});
socket.protocol; // "chat.v2"
```

Naming one the client did not offer is a `TypeError` here rather than a
handshake the client rejects — the latter surfaces as a socket that opened and
immediately died, a long way from the line that chose the wrong string.
`socket.protocol` carries the result, on upgraded and `serve()`-accepted
connections alike.

> **Over TLS the client must negotiate `http/1.1`.** Browsers do this for `wss:`.
> WebSocket over HTTP/2 requires RFC 8441 extended CONNECT, which this runtime
> does not implement, so a client that forces `h2` receives whatever the handler
> returns for a non-upgrade request rather than a `101`.

### Backpressure

`postMessage` never refuses a message and never throws for queue depth — HTML
does not permit it to, and Node, Deno and Bun all queue without limit. So a
producer that outruns its worker grows memory, and `queued` is what it can pace
against:

```js
for (const job of jobs) {
  w.postMessage(job);
  if (w.queued > 1000) await drain();     // your choice, not the runtime's
}
```

`queued` is the number posted and not yet taken. Inside a worker, `self.queued`
is the mirror — results sent to the parent that the parent has not taken yet.
Both are advisory, like a socket's `bufferedAmount`. No other runtime exposes
either.

### What crosses

`postMessage` uses the structured clone algorithm, so `Map`, `Set`, `Date`,
`RegExp`, `BigInt`, typed arrays, `Blob`, `File`, `DOMException` and cyclic
graphs all survive — this is not JSON. Transferable in a transfer list:

| Type | On transfer |
| --- | --- |
| `ArrayBuffer` | Sender detaches; receiver holds the data |
| `SharedArrayBuffer` | Not transferred — *shared*, as one allocation in both agents |
| `MessagePort` | Moves to the receiver, with anything already queued for it |
| `ReadableStream` / `WritableStream` / `TransformStream` | Original locks; chunks then flow across, with backpressure |

A `MessagePort` and a stream may be transferred but not cloned; either outside
the transfer list is a `DataCloneError`.

**Not planned:** a `blob:` URL minted on one agent does not resolve on another —
the object-URL store is per-isolate, where the spec scopes it to the agent
cluster — and a worker's URL must name a file: `new Worker("data:…")` and
`new Worker("blob:…")` are refused. Both schemes exist to move code and data
around inside a page; here the file is already on disk, and bytes cross by
`postMessage`.

---

## `WebSocket`

The classic WHATWG [`WebSocket`](https://websockets.spec.whatwg.org/#the-websocket-interface)
interface — a global (like `fetch`), not a `runtime:` module. Opening a
connection requires the **`Net`** capability; with no `Net` (or no WebSocket
provider installed) the socket fails with an `error` then a `close` (code 1006).
`ws:` and `wss:` are both supported (`wss:` reuses the same rustls TLS stack as
`fetch`/`runtime:net`).

```js
const ws = new WebSocket("wss://example.com/socket", ["chat"]);
ws.binaryType = "arraybuffer"; // or "blob" (default)

ws.addEventListener("open", () => ws.send("hello"));
ws.addEventListener("message", (e) => {
  // e.data is a string (text), or ArrayBuffer/Blob (binary, per binaryType)
  console.log(e.data, e.origin);
});
ws.addEventListener("close", (e) => console.log(e.code, e.reason, e.wasClean));
ws.addEventListener("error", () => {});

ws.close(1000, "done"); // code 1000 or 3000–4999; reason ≤ 123 UTF-8 bytes
```

| Member               | Type                                          | Notes                                                                 |
| -------------------- | --------------------------------------------- | --------------------------------------------------------------------- |
| `new WebSocket(url, protocols?)` | `(url, string \| string[]) => WebSocket` | `url` must be `ws:`/`wss:` with no fragment; protocols are RFC 6455 tokens. `Net`. |
| `readyState`         | `0 \| 1 \| 2 \| 3`                            | `CONNECTING`/`OPEN`/`CLOSING`/`CLOSED` (constants on the instance + interface). |
| `send(data)`         | `(BufferSource \| Blob \| USVString) => void` | Throws `InvalidStateError` while `CONNECTING`; dropped after close.    |
| `close(code?, reason?)` | `(number?, string?) => void`               | `code` = `1000` or `3000–4999` (`InvalidAccessError`); `reason` ≤ 123 UTF-8 bytes (`SyntaxError`). |
| `binaryType`         | `"blob" \| "arraybuffer"`                     | How binary messages surface in `message` events (default `"blob"`).   |
| `bufferedAmount`     | `number`                                      | Best-effort bytes queued by `send` but not yet flushed.               |
| `protocol` / `extensions` / `url` | `string`                         | Negotiated subprotocol / extensions (`""` — none negotiated) / the resolved URL. |
| `on{open,message,error,close}` | `EventHandler`                      | Also via `addEventListener`. `message` → `MessageEvent`; `close` → `CloseEvent`. |

**Not yet:** permessage-deflate (`extensions` is always `""`). See DECISIONS D29.

## `WebSocketStream`

The promise/stream-based interface from the same
[WHATWG spec](https://websockets.spec.whatwg.org/#the-websocketstream-interface) —
also a global, over the same connection seam and `Net` gate as `WebSocket`.
Reads are pull-based (real receive backpressure) and each write resolves when
the host has taken the frame (send backpressure).

```js
const wss = new WebSocketStream("wss://example.com/socket", {
  protocols: ["chat"],   // optional
  signal: controller.signal, // optional AbortSignal
});
const { readable, writable, protocol, extensions } = await wss.opened;

const writer = writable.getWriter();
await writer.write("hello");            // string ⇒ text frame
await writer.write(new Uint8Array([1])); // BufferSource ⇒ binary frame

for await (const chunk of readable) {
  // chunk is a string (text) or Uint8Array (binary)
}

wss.close({ closeCode: 1000, reason: "done" });
const { closeCode, reason } = await wss.closed;
```

| Member | Type | Notes |
| --- | --- | --- |
| `new WebSocketStream(url, options?)` | `(url, { protocols?, signal? }) => WebSocketStream` | Same URL/protocol validation as `WebSocket`. `Net`. |
| `opened` | `Promise<{ readable, writable, protocol, extensions }>` | Rejects with `WebSocketError` if the connection fails. |
| `closed` | `Promise<{ closeCode, reason }>` | Resolves on a clean close; rejects with `WebSocketError` on an abnormal one. |
| `close(closeInfo?)` | `({ closeCode?, reason? }) => void` | Same code/reason validation as `WebSocket#close`. |
| `WebSocketError` | `DOMException` subclass (global) | `name === "WebSocketError"`, plus `closeCode`/`reason`. |

Receipt is pull-driven (the embedder's tick, no owned loop), so a
server-initiated close is observed while reading — or, after a local
`close()`/writable close, by an internal drain that keeps receiving until the
peer's close frame settles `closed`.

---

## The `runtime:` scheme

Built-in modules are imported with a `runtime:` specifier:

```js
import { env, args } from "runtime:process";
```

These specifiers are intercepted by the runtime *before* any injected
`ModuleLoader` and served from a baked, in-binary source registry. They exist
regardless of which loader (or none) an embedder installs, and they never touch
the filesystem. Each built-in is a real ES module compiled through the normal
pipeline (`import.meta.url === "runtime:<name>"`) and deduplicated via the realm
module map.

The security boundary is the **op**, not the JavaScript module (DECISIONS D7):
importing a `runtime:` module always succeeds, but its operations throw unless
the required capability has been granted.

| Module            | Status      | Capability | Reference                     |
| ----------------- | ----------- | ---------- | ----------------------------- |
| `runtime:process` | Available   | `Env` / `Signals` | [↓](#runtimeprocess)   |
| `runtime:path`    | Available   | `Env`*     | [↓](#runtimepath)             |
| `runtime:fs`      | Available   | `FileRead` / `FileWrite` | [↓](#runtimefs) |
| `runtime:db`      | Available   | `FileRead` / `FileWrite` — **none** for `sqlite::memory:` | [↓](#runtimedb) |
| `runtime:net`     | Available   | `Net` / `NetListen` | [↓](#runtimenet)     |
| `runtime:http`    | Available   | `NetListen` | [↓](#runtimehttp)               |
| `runtime:websocket` | Available | `NetListen` | [↓](#runtimewebsocket)         |
| `runtime:serialization` | Available   | None       | [↓](#runtimeserialization)           |
| `runtime:hashing` | Available   | None — `Entropy` for `password.hash` only | [↓](#runtimehashing) |
| `runtime:build`   | Available — **`esdev` only** | `FileRead` (+ `FileWrite` to `write()`) | [↓](#runtimebuild) |
| `runtime:test`    | Available — **`esdev` only** | none | [↓](#runtimetest) |
| `runtime:watch`   | Available — **`esdev` only** | `FileRead` | [↓](#runtimewatch) |

---

## Capabilities

ES-Runtime is deny-by-default: a fresh runtime can compute but cannot reach the
host environment, filesystem, or network until the embedder grants the relevant
capability. **The standalone `esrun` CLI is the same** — it grants nothing until
you [name what a run may reach](#granting-capabilities-in-esrun) (DECISIONS
D65). `esdev`, the development binary, is the one exception: it grants
everything, so an inner loop needs no flags.
The check lives on the native op, so it cannot be bypassed by reaching a
different module path, and **importing** a `runtime:` module never needs a
capability — only its operations do.

| Capability  | Grants                                                              |
| ----------- | ------------------------------------------------------------------- |
| `Env`       | Environment and cwd — backs `runtime:process`. Not `args` or `platform`: those describe the invocation and the binary, not host state. |
| `FileRead`  | Read files within the configured root jail.                         |
| `FileWrite` | Write files within the configured root jail.                        |
| `Net`       | Open outbound network connections (`fetch`, `runtime:net` `connect`, a UDP `send`). |
| `NetListen` | Bind a listening socket and accept inbound connections (`runtime:net` `listen` and `bind`, `runtime:http` `serve`). |
| `Signals`   | Watch OS signals — `runtime:process` `onSignal`. Separate from `Env` because a watch **suppresses the signal's default action**: it is the privilege to decline to die on request, not a read of process state. |
| `Run`       | Spawn a child process — `runtime:system`. Never implied by another capability: a child runs **outside** every confinement here (no capability check, no root jail, no execution deadline), so granting it to guest code grants everything the host user can do. |
| `HrTime`    | Access high-resolution timing.                                      |

Filesystem access (including module resolution) is confined to a project **root
jail**, on by default and not currently optional (DECISIONS D25). Paths are
canonicalized to their real location before the check, so a symlink cannot
escape the jail.

### Granting capabilities in `esrun`

**`esrun` grants nothing by default** (DECISIONS D65). Two modes widen a run, and
they cannot be combined:

```sh
esrun --allow-imports --allow-net app.js    # nothing, plus these
esrun --allow-all --deny-run app.js         # everything, minus these
```

| Mode | Baseline | Direction |
| ---- | -------- | --------- |
| `--allow-<name>` | nothing granted (**the default**) | additive only |
| `--allow-all --deny-<name>` | everything granted | subtractive only |

`--deny-<name>` requires `--allow-all` — with nothing granted there is nothing
for it to take away. Neither mode mixes directions, so **no flag overrides
another**: read the list top to bottom and that is the answer.

`--deny-all` is accepted and restates the default; it is worth writing on a
deploy line, so a reader need not know which way a binary defaults. `esdev`
defaults the other way, and there `--allow-all` is the no-op — the flags, the
scopes and the rules are otherwise identical.

| Flag | Capability | Grants |
| ---- | ---------- | ------ |
| `--allow-read` | `FileRead` | `runtime:fs` / `runtime:wasi` reads |
| `--allow-write` | `FileWrite` | `runtime:fs` / `runtime:wasi` mutations |
| `--allow-imports` | `FileSystem` | `import "./x.js"`, `import "pkg"`, dynamic `import()` |
| `--allow-net` | `Net` | `fetch`, `WebSocket`, `runtime:net` `connect`, a UDP `send` |
| `--allow-listen` | `NetListen` | `runtime:net` `listen` and `bind`, `runtime:http` `serve` |
| `--allow-env` | `Env` | `runtime:process` `env` / `cwd()` |
| `--allow-run` | `Run` | `runtime:system` child processes |
| `--allow-signals` | `Signals` | `runtime:process` `onSignal` |
| `--allow-workers` | `Worker` | `new Worker(url)` |

Each name takes both prefixes: `--allow-net` and `--deny-net`.

#### Scoped grants

**Seven of the nine** can be granted narrowed to a list rather than whole. The
two exceptions are all-or-nothing: `imports`, because what may be *loaded* is
[its own mechanism](#import-policy--what-may-be-loaded), and `workers`, because
a worker's own grant is set at the spawn (`new Worker(url, { permissions })`)
rather than on the command line. `--allow-workers=<list>` is **rejected** rather
than ignored, so a run is never narrower on the command line than in reality:

```sh
esrun --allow-imports --allow-env=PORT,DATABASE_URL \
      --allow-net=db.internal:5432 --allow-listen=8080 \
      --allow-read=./data --allow-write=./out --allow-run=git \
      --allow-signals=SIGTERM server.js
```

| Flag | Grants | Everything else |
| ---- | ------ | --------------- |
| `--allow-read=<paths>` | reading those paths and their subtrees | fails with `ERR_PERMISSION_DENIED` |
| `--allow-write=<paths>` | writing those paths and their subtrees | fails before anything is created |
| `--allow-net=<hosts>` | reaching those addresses (`fetch`, `runtime:net` `connect`, `WebSocket`, every UDP destination) | fails with `ERR_PERMISSION_DENIED`, before any packet |
| `--allow-listen=<addresses>` | binding those addresses (`runtime:net` `listen` and `bind`, `runtime:http` `serve`) | fails before the port is claimed |
| `--allow-env=<names>` | those environment variables | absent from `env` — unreadable *and* unlistable |
| `--allow-run=<programs>` | spawning those programs | fails with `ERR_PERMISSION_DENIED` |
| `--allow-signals=<names>` | watching those signals | refused, and absent from `signals()` |

`--allow-run` matches on the **real path** a program resolves to, not on its
name: each entry is resolved once (a bare name through `PATH`, a path as
written) and a spawn is admitted only if it lands on the same file. So
`--allow-run=git` admits `git` and `/usr/bin/git` — the same program by two
spellings — and refuses `/tmp/git`, which is a different program that happens to
share a name.

**An address** is a host (any port), a `host:port`, or a bare port (any
interface — usually what a `--allow-listen` wants). Bracket an IPv6 literal that
carries a port: `[::1]:8080`. Matching is exact and never widens:
`--allow-net=example.com` does not admit `api.example.com`, and there are no
wildcards. Hosts are judged **as written, before resolution** — an IP entry
never silently admits a name that resolves to it, and DNS is not part of the
policy. A name entry therefore bounds the *name*: whoever controls the zone
chooses which address answers it, and chooses again on every connection. Where
the machine is what matters — a host outside your control — write the address.

`net` and `listen` keep separate lists: reaching out and being reachable are
separate capabilities, and an address allowed for one says nothing about the
other.

**`--allow-net` is enforced on every redirect hop**, not just the URL the
program wrote. A `302` from an allowed host to a denied one fails the request
with `ERR_PERMISSION_DENIED`; an allowlist checked only at the front door would
follow it transparently and hand back the denied host's body.

A scoped denial is a different fact from a capability denial, and reports as
one: an unlisted program throws `ERR_PERMISSION_DENIED` ("you have `run`, but
not this program"), where `--deny-run` throws `ERR_CAPABILITY_DENIED` ("you
never had `run`"). For the same reason a scoped grant still reports
`permissions.has("env") === true` — the capability opens the door; the list is
what the provider declines to hand over.

`permissions.has()` takes **one** argument, deliberately: `has("read", "/etc/passwd")`
throws a `TypeError` rather than answering about the capability and ignoring the
path. Which values are allowed is set by the deployment, and the exact answer
for one value is to perform the operation and catch `ERR_PERMISSION_DENIED` —
the runtime resolves a path before judging it, so any advance answer could be
stale by the time the call happens.

The value grammar, one rule set for every capability that takes a list:

- entries are comma-separated — `--allow-run=git,ls`;
- each entry is trimmed, so `--allow-env="A, B"` ≡ `--allow-env=A,B` and quoting
  is a shell convenience, not a syntax;
- an empty entry (`a,,b`, a trailing comma) is an **error** — a typo must not
  quietly change what the run may reach;
- repeating a flag unions its entries (`--allow-run=git --allow-run=ls`);
- granting a capability both whole and narrowed (`--allow-env --allow-env=HOME`)
  is an error, not a precedence rule.

**A path** is absolute or relative to the **working directory** — `./data` on a
command line means what it means in the shell you typed it in, not what it means
to the script. An entry covers its subtree, matched by path component, so
`--allow-read=./app` never admits `./app-secrets`. The check runs **after
canonicalization**: `./data/link-to-etc/passwd` is refused however the link is
arranged. Inside the [root jail](#the-root-jail) a path list **narrows**: only
the named subtrees are reachable. An entry *outside* the jail **adds** that
subtree, which is how a run reaches a TLS certificate under `/etc/letsencrypt` or
a CA bundle in `/etc/ssl` — locations no project root contains. Only a path typed
on the command line can do that; guest code can never move the boundary, and a
path neither inside the jail nor named on the command line is still
`ERR_JAIL_ESCAPE`. `read` and `write` are two grants: a read entry does not make
its subtree writable. `read` and `write` are
separate lists; the same lists govern `runtime:fs` and `runtime:wasi`.

**A signal entry** is a signal name. Unlisted signals are also absent from
`signals()`: a program should enumerate what it may use, not what the platform
happens to deliver.

A value on a flag that could not enforce it would still be **rejected rather
than ignored** — that rule outlives the capabilities it was written for, and
applies to any capability added later. Denials never take a value: a scope narrows a
grant, so it is written `--allow-<name>=<list>`.

The default — nothing granted — still runs the entry file, which is read before
the runtime exists. But since it denies `imports` too, a run with no flags is a
**single-file** run; add `--allow-imports` for an app with dependencies.
`Clock`/`Entropy`/`Timers`/`TaskSpawn` have no flag and are never denied: no op
gates them, so a denied script still computes. Ask from JS with
[`permissions`](#runtimeprocess), or find the flag set a program needs with
`esdev --trace-permissions app.js`.

### Import policy — what may be *loaded*

Capabilities answer *what may executing code reach*. Which modules may **become**
executing code is a different question, and it has its own mechanism (DECISIONS
D39): a JSON file named by `--import-policy=<file>`.

```sh
esrun --allow-imports --allow-net=db.internal:5432 \
      --import-policy=./import-policy.json server.js
```

```json
{
  "allow": ["./src", "express", "@acme/ui"],
  "deny": ["aws-sdk"]
}
```

- **Entries read the way specifiers do.** An entry beginning with `.` or `/` is
  a path covering its subtree; anything else is a package name (`lodash`,
  `@scope/pkg`). No second grammar — it is the split the loader already makes
  between a bare and a relative specifier. The two are alternatives, not
  territories: a module matches if *either* kind names it, so a path entry
  pointing inside `node_modules` governs what is there like any other.
- **Deny wins over allow.** A module named by both is refused.
- **Omitting `"allow"`** permits everything not denied — the shape for a policy
  that only wants to exclude a few packages. An empty `"allow": []` is an error
  rather than a run that can load nothing; `"deny": []` is fine.
- **Unknown keys are an error.** A misspelled `"allowed"` would otherwise parse
  as protection that is not there.
- **Paths resolve relative to the policy file**, not the working directory: a
  policy is committed next to the project it governs and means the same thing
  wherever it is invoked from.
- **Matching runs on the resolved, canonicalized module**, after the root jail —
  so a symlink cannot name its way in, and a pnpm store path is still
  recognisably its package. A package entry covers that package's own files and
  says nothing about the packages *it* imports; each is named in its own right,
  including a nested `node_modules` copy.
- **The entry file is exempt** — it is read before a loader exists, and you
  named it.
- **Never auto-discovered.** The file is read only when `--import-policy` names
  it, exactly as `--env-file` works (D30).

The two layers do not substitute for each other: the `imports` capability
decides whether the loader runs at all, the policy decides what it may resolve.
A policy is therefore **not a way around a missing `imports` grant** — without
`--allow-imports`, an allow entry still loads nothing.

> **Known gap: no integrity.** A policy names packages and paths, not content.
> `"express"` says the loader may resolve that package; it says nothing about
> *which* version, or whether the bytes are the ones you audited. Lockfiles
> remain the install-time counterpart, and content pinning is future work.

**The parser is strict, and its grammar is two rules for every flag** — not just
the permission ones:

1. A flag is `--flag` or `--flag=value`. A value is never a separate argument.
2. esrun's flags come before the script; everything after it is the script's.

```sh
esrun --timeout=500 app.js build --watch
#     └─ esrun's ──┘ └file┘ └─ the script's ─┘
```

Both are enforced rather than conventional: a value arriving as a separate word
is indistinguishable from the script path, and a flag written after the script
silently does nothing — which for `--deny-net` is a security failure. `--` after
the script opts a script's own argument out of rule 2.

| Written | Why it fails |
| ------- | ------------ |
| `--timeout 500` | Rule 1 — `500` would be mistaken for the script |
| `--allow-net example.com` | Rule 1 |
| `esrun app.js --allow-net` | Rule 2 — grants nothing where it stands |
| `--deny-run=git` | A denial is all-or-nothing — a scope narrows a *grant* |
| `--allow-env=A,,B` | An empty entry in a scope list |
| `--deny-net` without `--allow-all` | Nothing to take from an empty baseline |
| `--allow-ffi` | Not one of the nine |

---

## `runtime:process`

Host process information: environment, arguments, working directory, platform,
and exit. Aligned *in spirit* with the WinterTC CLI-API proposal (DECISIONS
D26).

- **Capability:** `Env` — except the [signal](#signals) exports, which need `Signals`, and `args` / `platform` / `arch` / `exit` / `permissions`, which need none. `args` is the command line that started this program, so it is not host state a grant withholds (D65).
- **Status:** Available
- **Loading:** on demand — importing it adds nothing to startup if unused.
- **Snapshotting:** `env` and `args` are captured on **first access**, not at
  module evaluation, so importing this module needs no capability even under
  `--deny-env` (DECISIONS D26/D38).

```js
import { env, args, platform, arch, cwd, exit, unmask } from "runtime:process";
// Or the default aggregate:
import process from "runtime:process";
```

The `env` snapshot includes any values loaded from `esrun --env-file` (DECISIONS
D30). Files load **only** via that explicit flag (no auto-discovery); the OS
environment wins on a conflict unless `--env-override` is passed, and later
`--env-file`s win over earlier ones.

### Exports

| Export            | Type                                | Description                                                                                                                                                                              |
| ----------------- | ----------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `env`             | `Record<string, string \| Secret>`  | Environment variables as a **mutable in-process object**, seeded from a host snapshot taken at module evaluation (plus any `--env-file` values). Reads, writes, and deletes work in-process; they do **not** propagate to the host process or to child processes. Assigned values are **coerced to strings** — an environment holds nothing else, so `env.PORT = 8080` stores `"8080"` and a symbol throws, matching Node and Deno. Secret-keyed values are `Secret` wrappers (see below) — including ones the program **assigns at runtime**, so `env.MY_API_KEY = "…"` masks on the same convention the snapshot does. |
| `args`            | `readonly string[]`                 | Program arguments after the runtime binary and the script (or `-e` snippet). **Frozen.** Excludes the executable and script path.                                                          |
| `platform`        | `string`                            | Host OS — the OS-native value (`std::env::consts::OS`): `"linux"`, `"macos"`, `"windows"`, …                                                                                              |
| `arch`            | `string`                            | Host CPU architecture — the OS-native value (`std::env::consts::ARCH`): `"x86_64"`, `"aarch64"`, `"arm"`, …                                                                               |
| `cwd()`           | `() => string`                      | Current working directory. A **function** (not a value) because the directory can change during a run.                                                                                    |
| `exit(code = 0)`  | `(code?: number) => never`          | Records the exit code and **halts execution immediately** — code after the call does not run. The embedder reads the recorded code and treats it as a clean exit, not an error.            |
| `unmask(value)`   | `(value: string \| Secret) => string` | Reveal a masked `Secret`'s real value. A plain `string` passes through unchanged, so `unmask(env.ANY)` is always safe.                                                                  |
| `Secret`          | `class`                             | Opaque holder for a masked env value (see **Secret masking**).                                                                                                                            |
| `signals()`       | `() => SignalName[]`                | Signal names this platform can deliver. **Capability: `Signals`.**                                                                                                                        |
| `onSignal(sig, fn)` | `(SignalName, (SignalName) => void) => void` | Run `fn` when `sig` arrives, suppressing its default action. **Capability: `Signals`.**                                                                          |
| `offSignal(sig, fn)` | `(SignalName, (SignalName) => void) => void` | Remove a handler; removing the last one for a signal restores the default action. **Capability: `Signals`.**                                                     |
| `permissions`     | `object`                            | What this process is allowed to reach — see **Permissions** below. **Needs no capability.**                                                                                               |
| `default`         | `object`                            | An aggregate bundling all named exports. Named imports are preferred for clarity and tree-shaking.                                                                                        |

### Permissions

The policy is fixed at launch — by `esrun`'s [permission flags](#granting-capabilities-in-esrun)
or by the embedder's capability set — so this is introspection only. There is
nothing to request and no prompt to await, which is why `has()` is a synchronous
boolean rather than a promise.

```js
import { permissions } from "runtime:process";

permissions.denied;        // ["read", "write"] — [] when nothing is denied
permissions.has("net");    // true
if (permissions.has("write")) await fs.write("cache.json", data);
```

| Export | Type | Description |
| ------ | ---- | ----------- |
| `permissions.denied` | `readonly PermissionName[]` | The names this process may not use, in capability order. |
| `permissions.has(name)` | `(PermissionName) => boolean` | Whether `name` is available. Throws `TypeError` for a name outside the nine — a typo'd check would otherwise read as a denial and take the degraded path forever. |

`PermissionName` is `"read" | "write" | "imports" | "net" | "listen" | "env" |
"run" | "signals" | "workers"` — the same words the `--deny-<name>` flags use,
and the same words `new Worker(url, { permissions })` takes.

Needs no capability, deliberately: it reveals only what a program could learn by
calling each op and catching the denial, and code that must ask "may I?" is
exactly the code running under the tightest policy.

### Signals

Watching a signal **suppresses its default action** — which is the point, and
why it needs the `Signals` capability rather than riding on `Env`. A `SIGTERM`
handler is what stops an orchestrator's shutdown from killing the process
outright, and is how graceful shutdown is written:

```js
import { onSignal, offSignal } from "runtime:process";
import { serve } from "runtime:http";

const server = serve(handler);

const shutdown = async (signal) => {
  offSignal(signal, shutdown);   // second ^C should kill, not queue
  await server.stop();           // stop accepting, drain in-flight
  await pool.close();
};
onSignal("SIGINT", shutdown);
onSignal("SIGTERM", shutdown);
```

| Platform | Deliverable |
| --- | --- |
| Unix | `SIGINT`, `SIGTERM`, `SIGHUP`, `SIGUSR1`, `SIGUSR2` |
| Windows | `SIGINT`, `SIGBREAK` |

Asking for a signal the platform cannot deliver **throws**, rather than
registering a handler that would never fire; `signals()` reports the set. An
unknown name is a `TypeError`.

While anything is watched, the program stays alive to receive it — the same
behaviour as Node and Deno, and the reason to install a handler at all. Removing
the last handler releases it, so a program that stops listening can still exit.

Repeated deliveries **coalesce**: a burst of `SIGHUP`s while the first is still
being handled arrives once. Signals are edge notifications ("a reload was asked
for"), and replaying a backlog helps nobody. A handler that throws is reported
like any other unhandled failure and does not stop the others.

### Secret masking

Env entries with a secret-bearing key (case-insensitive) are exposed as a
`Secret` rather than a raw string. A key qualifies when it **ends with**
`_KEY(S)`, `_TOKEN(S)`, `_SECRET(S)`, `_PASS`, or `_PASSWORD(S)`, or **contains**
`CREDENTIAL(S)` or `AUTH` as an underscore-delimited word (so `AUTH_TOKEN`
matches, `AUTHOR` does not). A `Secret`
renders as `"[redacted]"` everywhere a value would otherwise leak — `console`
output, string coercion / template literals, and `JSON.stringify`. The real
value is held in a module-private `WeakMap` and is obtainable only via
`unmask(...)`. This guards against **accidental** disclosure to logs; it is not
a barrier against hostile guest code (which can call `unmask` itself). DECISIONS
D30.

```js
import { env, unmask } from "runtime:process";
console.log(env.DB_PASSWORD);        // [redacted]
console.log(`${env.DB_PASSWORD}`);   // [redacted]
JSON.stringify(env);                 // ..."DB_PASSWORD":"[redacted]"...
const pw = unmask(env.DB_PASSWORD);  // real value, explicit
```

### Examples

```js
// env — read / write / delete (in-process only)
import { env } from "runtime:process";
console.log(env.HOME);
env.FEATURE_FLAG = "on";
delete env.CACHE_DIR;
```

```js
// args — program arguments
// $ esrun app.mjs build --watch
import { args } from "runtime:process";
console.log(args); // ["build", "--watch"]
```

```js
// exit — stop the run with a status code
import { exit } from "runtime:process";
if (failed) exit(1);
exit(); // defaults to 0
```

---

## `runtime:path`

Modern, platform-aware path utilities. Pure computation — it performs no I/O.
The host platform and working directory come from
[`runtime:process`](#runtimeprocess), so separators and `resolve()` follow the
real OS.

**Capability:** none to import, and none for the pure string functions. `Env` is
needed only to read the working directory — so `resolve()` requires it when no
segment is absolute (and `relative()`/`toFileURL()` inherit that, since they
resolve first). `resolve("/abs", "b")` needs nothing.

This is intentionally free of legacy baggage: one platform-correct surface (no
`posix`/`win32` dual namespaces, no overloaded signatures), plus first-class
`file:` URL interop — `dirname(fromFileURL(import.meta.url))` is the modern
`__dirname`.

```js
import { join, resolve, dirname, fromFileURL } from "runtime:path";

const here = dirname(fromFileURL(import.meta.url));
const cfg = resolve(here, "config", "app.json");
```

### Exports

| Export                  | Type                          | Description                                                                 |
| ----------------------- | ----------------------------- | --------------------------------------------------------------------------- |
| `sep`                   | `string`                      | Path segment separator for the host OS (`"/"` or `"\\"`).                    |
| `delimiter`             | `string`                      | Path list delimiter for the host OS (`":"` or `";"`).                       |
| `isAbsolute(p)`         | `(string) => boolean`         | Whether `p` is an absolute path.                                            |
| `normalize(p)`          | `(string) => string`          | Collapses `.`/`..` and redundant separators. A trailing separator is **kept** — it says the path names a directory. |
| `join(...segments)`     | `(...string) => string`       | Joins segments with the separator, then normalizes.                         |
| `resolve(...segments)`  | `(...string) => string`       | Resolves to an absolute path, anchoring at `cwd()` if no segment is absolute. Drops a trailing separator (unless the result is the root): it answers *which location*, and a location is the same one however it is spelled. |
| `dirname(p)`            | `(string) => string`          | The directory portion of `p`.                                               |
| `basename(p)`           | `(string) => string`          | The final segment of `p`. No suffix-stripping overload — `parse(p).name` is the exact equivalent of Node's `basename(p, ext)`. |
| `extname(p)`            | `(string) => string`          | The extension of the final segment, including the dot (or `""`).            |
| `parse(p)`              | `(string) => object`          | `{ root, dir, base, name, ext }`. `name` is the final segment with its extension removed. |
| `relative(from, to)`    | `(string, string) => string`  | Relative path from `from` to `to` (both resolved first).                    |
| `fromFileURL(url)`      | `(string \| URL) => string`   | Converts a `file:` URL to a path.                                           |
| `toFileURL(p)`          | `(string) => URL`             | Converts a path (resolved to absolute) to a `file:` URL.                    |
| `default`               | `object`                      | An aggregate of all named exports.                                          |

---

## `runtime:fs`

**Blob-based** file I/O, modeled on the web `Blob` surface — lazy file handles
and writes that accept any web body. Reads require `FileRead`, mutations require
`FileWrite`, and every path is confined to the project **root jail** (D25) — a
path that escapes (via `..` or a symlink) is rejected. All operations are async
(no sync variants); there are no callbacks.

```js
import { file, write, readDir, stat, mkdir, remove } from "runtime:fs";

await mkdir("data", { recursive: true });
await write("data/app.json", JSON.stringify({ ok: true }));

const f = file("data/app.json");          // lazy, Blob-like handle
const cfg = await f.json();                // .text() / .bytes() / .arrayBuffer() / .stream()
await write("data/copy.json", f);          // any web body: string|Blob|ArrayBuffer|TypedArray|Response|ReadableStream|file()
```

Paths may be a string, a `file:` URL (string or `URL`), or a `file()` handle.

### Module functions

| Export                | Type                                            | Description                                                                 |
| --------------------- | ----------------------------------------------- | --------------------------------------------------------------------------- |
| `file(path)`          | `(path) => FsFile`                              | A lazy, `Blob`-like handle — nothing is read until a read method is called. |
| `write(dest, input)`  | `(path, body) => Promise<number>`               | Writes any web body to `dest`; resolves to bytes written. Streams to disk if given a `ReadableStream`/`Response`. |
| `readDir(path)`       | `(path) => Promise<DirEntry[]>`                 | Directory entries: `{ name, isFile, isDir, isSymlink }`.                     |
| `stat(path)`          | `(path) => Promise<Stat>`                       | `{ size, isFile, isDir, isSymlink, mtimeMs }` (follows symlinks).           |
| `exists(path)`        | `(path) => Promise<boolean>`                    | Whether the path exists (missing → `false`, not an error).                  |
| `mkdir(path, opts?)`  | `(path, { recursive? }) => Promise<void>`       | Creates a directory; `recursive` creates parents.                           |
| `remove(path, opts?)` | `(path, { recursive? }) => Promise<void>`       | Removes a file or (with `recursive`) a directory tree.                      |
| `rename(from, to)`    | `(path, path) => Promise<void>`                 | Renames/moves an entry (both jailed).                                       |
| `copy(from, to)`      | `(path, path) => Promise<number>`               | Copies a file, overwriting `to`; resolves to bytes copied. Needs **both** `FileRead` and `FileWrite`. `ERR_SAME_FILE` if `from` and `to` are the same file. |
| `realPath(path)`      | `(path) => Promise<string>`                     | The canonical location — symlinks followed, `.`/`..` removed. `ERR_NOT_FOUND` if missing, `ERR_JAIL_ESCAPE` if it resolves outside the jail. `FileRead`. |
| `readLink(path)`      | `(path) => Promise<string>`                     | The stored target of a symlink, verbatim (may be relative, may dangle). `FileRead`. |
| `truncate(path, len?)`| `(path, number) => Promise<void>`               | Sets the file's length exactly, zero-filling if it grows.                   |
| `chmod(path, mode)`   | `(path, number) => Promise<void>`               | Sets permission bits (`0o600`). Windows honours only the owner-write bit, as the read-only flag. |
| `makeTempDir(opts?)`  | `({ dir?, prefix? }) => Promise<string>`        | Creates a directory with an unpredictable name; resolves to its path.       |
| `makeTempFile(opts?)` | `({ dir?, prefix? }) => Promise<string>`        | Creates an empty file with an unpredictable name; resolves to its path.     |

**Temporary entries** default to the base directory, **not** the OS temp
directory — that lives outside the root jail, so writing there would be the one
filesystem call that escapes it. Pass `dir` to place them elsewhere inside the
jail. The name comes from the host's temp-file machinery, so it is
unpredictable: a guessable name in a shared directory is a symlink-attack
invitation. Nothing is cleaned up automatically — what you create, you remove.

**`copy` needs both capabilities.** It reads one path and writes another;
gating it on the write alone would let a guest with no read access duplicate a
file it cannot see into somewhere it can reach by another route.

**The root is not a target.** An **empty path** is `ERR_INVALID_PATH` on every
operation — it names no file, and joining it onto the base directory would
otherwise resolve to the jail root. A **mutation** whose resolved target *is*
the root is refused the same way, however it is spelled (`.`, `./`, `data/..`,
the root's own absolute path): removing, renaming, truncating or `chmod`ing the
root destroys the sandbox the program is running in. Reading the root is
ordinary and unaffected — `stat(".")`, `readDir(".")` and `realPath(".")` work —
as is writing entries *inside* it.

### `FsFile` (from `file(path)`)

`text()`, `json()`, `bytes()` (`Uint8Array`), `arrayBuffer()`, `stream()`
(`ReadableStream`), `exists()`, `stat()`, `write(data)`, `delete()`, and the
`path` it points at — the Blob read surface plus convenience writes/deletes.

---

## `runtime:db`

Databases, in two tiers (DECISIONS D56). The **application tier** is `connect()`
and what it returns. The **driver tier** is what a third party needs to add a
backend of their own; both are exported from `runtime:db`, and the split is in
this documentation rather than in the specifier.

```js
import { connect, sqlite, sql } from "runtime:db";

const db = await connect("sqlite:./app.db", { driver: sqlite });
await db.execute(sql`INSERT INTO users (name) VALUES (${name})`);
for await (const row of await db.query("SELECT id, name FROM users")) {
  console.log(row.id, row.name);
}
await db.close();
```

**Capabilities.** Opening a database is a filesystem access and is scoped as
one: `FileRead` to read, `FileWrite` as well to write, confined by the same root
jail and `--allow-read` / `--allow-write` lists that back `runtime:fs`. The one
exception is `sqlite::memory:`, which needs **no capability at all** — it names
no file and touches no filesystem, so a filesystem grant would guard nothing
that happens, and what it costs is memory, which guest JS can already spend.
`runtime:db` adds no capability of its own.

### `connect(url, options)`

Opens a connection with the **driver** passed in `options.driver` — a value you
import, not a global installed by importing a package for its side effects.
`sqlite` is exported from `runtime:db`; every other driver is a package export
(`import { driver } from "@opentf/esrun-postgres"` — every driver package
exports one under that name, and nothing as a default). What comes back is that
driver's connection, so its own surface needs no second entry point. The URL's
scheme is checked against the driver's schemes.

`sqlite:` names a file format and a SQL dialect the way `postgres://` names a
wire protocol — not a particular implementation, which may be replaced without
the URL changing.

| Option | Meaning |
| --- | --- |
| `driver` | **Required.** The driver to open with. |
| `pool` | `true`, or `{ max, idleTimeout, acquireTimeout }` — a pool presenting the same surface one connection does, plus `size`, `idle`, `pending` and `withConnection(fn)`. |
| `key` | `sqlite`: encryption key, hex string or bytes. |
| `cipher` | `sqlite`: cipher name; defaults to the backend's. |
| `readOnly` | `sqlite`: open without the ability to write. |

A key belongs in the **options object, never the connection string** — a key in
a URL ends up in logs, error messages and stack traces, so one passed as a URL
parameter is refused rather than quietly honoured.

`sqlite::memory:` opens a database that exists only in memory; each connection
gets its own. The named form (`:memory:name`), which in SQLite means *sharing*
one, is refused rather than quietly not sharing.

### `Connection`

- `query(q, params?)` → `Rows`
- `execute(q, params?)` → `{ changes, lastInsertRowid }`
- `executeMany(sql, rows)` — one statement, many parameter sets, **one
  crossing**. A crossing costs about the same whatever it carries, so a loop
  that crosses per row spends its time on the boundary rather than in the
  database. Runs as a single transaction unless one is already open, in which
  case it joins it.
- `transaction(fn)` — commits when `fn` returns, rolls back when it throws.
  Nested calls become savepoints where the backend has them, so a helper that
  opens a transaction composes with a caller that already did. A rollback that
  itself fails never replaces the error that caused it.
- `close()`, and `Symbol.asyncDispose` for `await using`.

`q` is SQL text, a `` sql`` `` template, or a `QueryAst`. `params` is an array
(bound by position) or an object (bound by name). A third argument takes
`{ signal }`:

```js
await db.query(sql, params, { signal: AbortSignal.timeout(5_000) });
```

Aborting asks the backend to cancel and **waits** for it to answer, so the
connection is left in a known state and stays usable — the difference between
cancelling and hanging up. The rejection carries the signal's own `reason`, not
the backend's word for a cancelled statement, including when a streaming result
is abandoned halfway. `sqlite:` interrupts a running statement; a networked
backend cancels over the protocol.

**Which forms a backend takes is its own declaration.** `dialect.supports.queryText`
(default `true`) and `supports.queryAst` (default `false`) say so, and the form a
backend does not take is refused with `ERR_DB_QUERY_FORM` — in either direction.
`sqlite:` and `postgres:` take SQL; `redis:` takes command arrays and refuses
SQL, which is what the AST form was carried in the contract from the first
release for.

`supports.transactions` (default `true`) is the matching declaration for
`transaction(fn)`. A backend that says `false` refuses with
`ERR_DB_UNSUPPORTED` rather than emitting a `BEGIN` it has never heard of, and
its `executeMany` runs **without** a transaction — so a batch is not atomic
there, which is why it is declared rather than assumed.

### `` sql`` ``

Every interpolation becomes a parameter, never text, and a nested `` sql`` ``
fragment splices with its own values. The fragments and values are kept apart
until the backend renders them, so one template targets `$1`, `?` and `:name`
backends unchanged — and there is no arrangement of it that puts a value into
the text.

### `Rows`

Async-iterable, pulled one batch at a time — never the whole result, so a table
larger than memory streams through at the cost of a batch. Stopping early
(`break`, `return`, `throw`) closes the cursor and leaves the connection usable.
Also `toArray()`, `first()` (`null` when empty), `close()`, and `columns`.

A result small enough to fit one batch comes back **with the query itself** —
no cursor is opened, so there is nothing to fetch and nothing to close, and the
whole query costs one crossing rather than three. `rows.exhausted` reports which
happened.

### `Row`

A **lazy view** over its batch, with one getter per column, so a query that
selects more columns than it reads pays only for the ones it touches. A 64-bit
integer arrives as a `bigint` only where a `number` would have lost it; a blob
arrives as a `Uint8Array`.

Because the getters live on the prototype, **`{ ...row }` does not copy the
columns** — it yields an empty object, and leaks nothing internal either. Use
`row.toObject()`, which is how a row is materialized; `JSON.stringify(row)`
works, and `row.values()` gives the columns in query order.

### Errors

Failures are a `DbError` whose `code` is layered: the driver's own
classification first (`ERR_DB_UNIQUE_VIOLATION`, `ERR_DB_DEADLOCK`,
`ERR_DB_BUSY`, …), then a stable host code, then `ERR_DB_BACKEND`. A denied
capability stays `ERR_CAPABILITY_DENIED` and a jail escape stays
`ERR_JAIL_ESCAPE` — an application testing for those should not have to know
that the call went through a database. The backend's own code, where it had one,
stays on `e.backendCode`.

### The driver tier

For building a backend or an ORM: `defineDriver(spec)`, which turns
`{ name, schemes, dialect, open, pooled? }` into the value a caller passes to
`connect`; `PooledConnection`, the pooled form `pool: true` builds and a driver
subclasses to add its own surface; `BaseConnection` (transactions, savepoints, the
closed-connection check, and a correct-but-slow default batch a driver overrides
to make fast); `Dialect` (`placeholder`, `quoteIdent`, `supports`);
`defineRowShape` and `decodeBatch` (the row decoder for a backend handed bytes)
and `defineRecordShape` / `Rows.fromObjects` (the same `Row` contract for one
whose values are already JavaScript);
`encodeParams` / `splitParams`; `ByteWriter`; `mapError` / `asDbError`; and
`runBackendConformance(open)`, the suite a driver runs to demonstrate it behaves
like the built-ins. There is no registry to claim a scheme in: a driver is a
value, so two drivers for the same scheme coexist and the caller says which one
it meant.

A connection also answers `usable` (still worth using), `reusable` (fit for the
next caller — the one question a protocol-blind pool cannot decide) and
`withConnection(fn)` (one connection held for the whole of `fn`). All three are
on a single connection and on a pool alike, so code that holds "a connection"
never has to ask which kind it has.

`dialect.supports` carries a driver's **own** capability flags beside the ones
the kit acts on, and `driver.dialect` answers before a connection is opened —
which is how an ORM branches on a backend that did not exist when it was
written.

A driver supplies `_query`, `_execute` and `_close`, and may override
`_executeMany(query, sets)` for a real batch path, `_cancel()` for whatever its
backend offers, and `_beginTransaction` / `_commitTransaction` /
`_rollbackTransaction` — which default to the SQL every SQL backend spells the
same way, and exist as methods so that a backend without SQL can still have
transactions.

Most conformance checks are written in SQL. Against a backend declaring
`supports.queryText: false` those are **skipped with a reason** rather than
failed — a check you cannot express is not a finding — and what runs is the part
that holds for every backend whatever form it takes.

Adding a networked backend needs **no new runtime code**: the Postgres, Redis
and MySQL drivers are JS over [`runtime:net`](#runtimenet).

---

## `runtime:net`

TCP sockets and UDP datagrams (SPEC §12). `connect()` follows the **WinterTC Sockets API**:
outbound TCP with web-stream `readable`/`writable`. `listen()` returns an
async-iterable of inbound sockets. `connect` requires `Net`; `listen` requires
`NetListen`. All I/O is async — nothing blocks. **TLS** client connections are
supported via `secureTransport: "on"` (certificate verification on, with `sni`
and `alpn`). `sni` overrides the server name used for **both** the SNI extension
and certificate hostname verification (they share one name in rustls), so set it
only to a name the presented certificate is valid for. `secureTransport:
"starttls"` opens plaintext and upgrades in place via `Socket.startTls()` (SMTP/
IMAP-style). `listen({ secureTransport: "on", cert, key })` **terminates TLS
server-side**: pass a PEM `cert` chain + `key` (and optional `alpn`) and every
accepted socket is encrypted (its `opened.alpn` reports the negotiated protocol).
The cert/key are supplied inline, so server TLS needs no capability beyond
`NetListen`.

```js
import { connect, listen } from "runtime:net";

// Client (WinterTC connect()):
const sock = connect({ hostname: "example.com", port: 80 });
await sock.opened;
const w = sock.writable.getWriter();
await w.write(new TextEncoder().encode("GET / HTTP/1.0\r\n\r\n"));
for await (const chunk of sock.readable) { /* … */ }

// TLS client (secureTransport: "on") with ALPN:
const tls = connect({ hostname: "example.com", port: 443 }, {
  secureTransport: "on",
  alpn: ["h2", "http/1.1"],
});
const { alpn } = await tls.opened; // negotiated protocol, e.g. "h2" (or null)

// Server:
const server = listen({ hostname: "127.0.0.1", port: 8080 });
for await (const conn of server) {
  conn.readable.pipeTo(conn.writable); // echo
}

// TLS server (terminates TLS on accept):
const tlsServer = listen({
  hostname: "127.0.0.1", port: 8443,
  secureTransport: "on", cert: certPem, key: keyPem, alpn: ["h2", "http/1.1"],
});
```

### Exports

| Export                       | Type                                  | Description                                                        |
| ---------------------------- | ------------------------------------- | ------------------------------------------------------------------ |
| `connect(address, options?)` | `(addr, { secureTransport?, sni?, alpn?, allowHalfOpen? }) => Socket` | Open an outbound TCP (or TLS) connection; returns a `Socket` immediately (`opened` settles on connect). `secureTransport: "on"` negotiates TLS, `"starttls"` opens plaintext for a later `startTls()`; `sni` overrides the server name (default: the host); `alpn` is the offered protocol list; `allowHalfOpen` keeps writing after the peer's FIN. `Net`. |
| `listen(options)`            | `({ hostname?, port, secureTransport?, cert?, key?, alpn?, reusePort? }) => Listener` | Bind a listening socket. `secureTransport: "on"` terminates TLS on each accept — requires a PEM `cert` + `key`; `alpn` advertises protocols; `reusePort` shares the port with other processes (see below). `NetListen`. |
| `bind(options)`              | `({ hostname?, port, reusePort?, reuseAddress?, broadcast?, ttl?, multicastTtl?, multicastLoopback? }) => DatagramSocket` | Bind a UDP socket (see [UDP](#udp)). `NetListen` to bind, `Net` to send. |

**`Socket`** — `readable`/`writable` (web streams), `opened: Promise<SocketInfo>`,
`closed: Promise<void>`, `close(reason?)`, `upgraded`, and `startTls(): Socket`
(valid only on a `"starttls"` socket; returns a new TLS `Socket` with `upgraded
=== true`). `close`'s `reason` is advisory (WinterTC) and ignored. Closing the
writable half-closes (FIN); `allowHalfOpen` (a `connect` option, default
`false`) keeps the writable usable after the peer's FIN.
**`SocketInfo`** (from `opened`): `{ remoteAddress, remotePort, localAddress,
localPort, alpn }` — `remoteAddress`/`localAddress` are WinterTC `"host:port"`
strings (IPv6 host bracketed); `alpn` is the negotiated protocol for a TLS
socket, else `null`.

**`Listener`** — async-iterable of `Socket`; `addr: Promise<{ hostname, port }>`,
`accept()`, `close()`.

**Errors** — socket failures (bad options, connect/TLS/I/O errors) surface as a
`TypeError` whose message is prefixed `"SocketError: "` (WinterTC `SocketError`),
including a bind failure reaching `Listener.addr`. The **port** is validated at
the call: it must be an integer in `0`–`65535`, and `connect` additionally
rejects `0` — a port that is not a port used to be coerced to `0`, so a typo
connected somewhere else rather than failing. `listen({ port: 0 })` remains the
way to ask for an ephemeral port.

### UDP

`bind()` returns a `DatagramSocket` — messages, not a byte stream, so it has
`send`/`receive` rather than `readable`/`writable`. A datagram arrives whole or
not at all and carries its own sender, which is what a stream would erase.

```js
import { bind } from "runtime:net";

const sock = bind({ hostname: "0.0.0.0", port: 5353 });
const { port } = await sock.addr;          // port 0 ⇒ ephemeral

for await (const { data, address, port } of sock) {
  await sock.send(data, { hostname: address, port }); // echo to the sender
}
```

| Member | Type | Description |
| --- | --- | --- |
| `send(data, address?)` | `Promise<number>` | Send one datagram; resolves with the bytes sent. `address` is `"host:port"` or `{ hostname, port }`, and is required unless the socket is connected. Requires `Net`, checked **per destination**. |
| `sendMany(messages, address?)` | `Promise<number>` | Send a batch in one crossing; resolves with how many left. Each entry is a payload or `{ data, address }`. Saves the crossing, not the syscalls; a failure part-way reports how many had gone. Requires `Net`, still checked per destination. |
| `receive()` | `Promise<Datagram \| null>` | The next `{ data, address, port, truncated }`, or `null` once closed. One call is one message — a zero-length datagram is a message, not an EOF. |
| `receiveMany(max?)` | `Promise<Datagram[] \| null>` | A datagram plus up to `max - 1` more that had **already** arrived (default 32). Never waits for a full batch, so latency is unchanged and a busy socket costs one crossing per batch. |
| `connect(address)` | `Promise<SocketInfo>` | Fix the peer: sends need no address, and datagrams from anyone else are discarded. No packet is sent, so this succeeds against a host that is not listening. Requires `Net`. |
| `joinMulticast(group, { interface?, source? })` | `Promise<void>` | Join a group. `interface` is an IPv4 address for a v4 group, an interface **index** for a v6 one. `source` makes it source-specific (RFC 4607, IPv4 only) — the network filters, so nobody else's traffic arrives. |
| `leaveMulticast(group, { interface?, source? })` | `Promise<void>` | Leave a group. A membership taken with a `source` must be left with the same one. |
| `setTtl(n)` / `setMulticastTtl(n)` | `Promise<void>` | Hop limits, after the bind. |
| `setBroadcast(on)` / `setMulticastLoopback(on)` | `Promise<void>` | The two toggles, after the bind. `setBroadcast` is IPv4 only. |
| `setMulticastInterface(iface)` | `Promise<void>` | Which local interface carries **outgoing** multicast — an IPv4 address on a v4 socket, an interface index on a v6 one. The one option with no bind-time twin. |
| `ref()` / `unref()` | `this` | Node's handle ref-counting: `unref()` stops a pending `receive()` from keeping the process alive. A parked receive keeps working either way. |
| `addr` | `Promise<{ hostname, port }>` | The bound address. |
| `close()` | `Promise<void>` | Close the socket; a parked `receive()` resolves to `null`. |
| `closed` | `Promise<void>` | Resolves once closed. |
| `[Symbol.asyncIterator]` | `AsyncIterable<Datagram>` | `for await (const d of sock) { … }`, ending at `close()`. |

**Bind options** — `reusePort` (share the address across processes; Unix only),
`reuseAddress` (share it with another socket — what two processes receiving one
multicast group need), `broadcast` (permit sending to the broadcast address;
IPv4 only), `ttl`, `multicastTtl`, `multicastLoopback`, and `ipv6Only` (whether a
v6 bind also accepts v4-mapped traffic; omitted leaves the platform default,
which differs between platforms). An omitted option leaves the OS default rather
than a value chosen here. The address family decides which spelling of each is
used, so an IPv6 socket asking for `broadcast` is an error rather than a flag
that sets nothing.

`reusePort`, `reuseAddress` and `ipv6Only` are **bind-time only** — they have to
be set between `socket()` and `bind()`, so a setter for them would be one that
quietly did nothing. The rest have setters (`setTtl`, `setBroadcast`, …) for the
cases where they change while a program runs.

**Truncation** — `datagram.truncated` is `true` when the message did not fit the
receive buffer, so `data` is a prefix and the rest is gone. It cannot happen over
IPv4, whose largest datagram fits; an IPv6 jumbogram is what reaches it.

**Two capabilities, not one.** `bind` requires `NetListen` — it takes a port,
and a port is how a process is reached, ephemeral or not — while `send` and
`connect` require `Net`. A UDP socket is a server and a client at once, so a
program that only receives needs `listen` alone, and one that sends needs both.
`--allow-listen` scopes the bind; `--allow-net` scopes every destination.

One failure reaches **every** surface of the socket it belongs to: a refused
connect rejects `opened`, the streams, `close()` and a later `startTls()` alike,
because all of them derive from the same pending connection. Handling it on any
one of them is enough — the others do **not** additionally report as unhandled
rejections, so a single unreachable host cannot end a process that already dealt
with it. The corollary: a socket whose failure is *never* observed — nothing
awaits `opened`, nothing touches the streams — fails silently rather than
failing the run. `Listener.addr` behaves the same way with respect to a bind.

## `runtime:http`

An HTTP/1.1 **and HTTP/2** server: `serve((request) => response)`. The handler receives a web
`Request` and returns (or resolves to) a web `Response` — the same Fetch API
objects `fetch` uses. A thrown error or a non-`Response` return becomes a `500`.
`serve` requires `NetListen` (it binds a listening socket). All I/O is async.
Bodies **stream both directions**: the request body is a `ReadableStream`
pulling chunks as they arrive (nothing is buffered unless the handler asks, e.g.
`await request.text()`), and a `ReadableStream` response body is sent with
chunked transfer-encoding as it is produced (bounded-channel backpressure) — so
SSE-style responses and `new Response(request.body)` proxying work unbuffered.
`secureTransport: "on"` terminates **TLS** on accept (see below). The protocol
version is the client's choice and never reaches the handler (see
[HTTP/2](#http2)).

```js
import { serve } from "runtime:http";

const server = serve({ hostname: "127.0.0.1", port: 8080 }, async (request) => {
  const url = new URL(request.url);
  if (url.pathname === "/echo") {
    return new Response(await request.text(), { status: 200 });
  }
  return Response.json({ method: request.method, path: url.pathname });
});

const { port } = await server.addr; // ephemeral port resolved here
// … later:
await server.stop();
```

### Exports

| Export                            | Type                                          | Description                                                        |
| --------------------------------- | --------------------------------------------- | ------------------------------------------------------------------ |
| `serve(handler)`                  | `(Handler) => Server`                         | Start a server on an ephemeral port. `NetListen`.                  |
| `serve(options, handler)`         | `({ hostname?, port?, secureTransport?, cert?, key?, alpn?, timeouts?, maxConnections?, maxConnectionsPerIp?, reusePort? }, Handler) => Server` | Start a server bound to `options`. `NetListen`. |

`Handler` is `(request: Request, info: ConnectionInfo) => Response | Promise<Response>`.
The second argument is optional to take — a one-parameter handler is unaffected.

#### The connection a request came from

`info.remoteAddr` is the other end of the socket, in the shape `Deno.serve`
passes, so a handler ports either way:

```js
serve({ port: 8080 }, (request, info) => {
  // { transport: "tcp", hostname: "203.0.113.7", port: 54321 }
  return new Response(`hello ${info.remoteAddr.hostname}`);
});
```

It is `null` when the host has no peer to report — a mock provider, a transport
with no address — rather than an address-shaped object full of blanks, which a
handler would happily key a rate limit on.

**Behind a reverse proxy this is the proxy.** `X-Forwarded-For` is deliberately
never consulted: resolving it takes knowing which hop to trust, and a header
anyone can send is not an identity until something says whose to believe. The
header is delivered untouched, so a deployment that *does* know can resolve it
itself:

```js
const client = request.headers.get("x-forwarded-for")?.split(",")[0].trim()
  ?? info.remoteAddr.hostname;
```

On HTTP/2 every request multiplexed onto one connection reports the same peer,
because they are one connection.

**`Server`** — `addr: Promise<{ hostname, port }>` (resolves once listening),
`finished: Promise<void>` (resolves after `stop()`), `stop(): Promise<void>`.

#### HTTPS

`secureTransport: "on"` terminates TLS on accept. The certificate and key travel
**inline**, exactly as `runtime:net` `listen` takes them — reading a file is the
filesystem's privilege, so a guest serving HTTPS from a cert on disk reads it
with `runtime:fs` under its own gate, and serving needs no grant beyond
`NetListen`:

```js
import { serve } from "runtime:http";
import { file } from "runtime:fs";

serve(
  {
    port: 443,
    secureTransport: "on",
    cert: await file("/etc/certs/fullchain.pem").text(), // PEM chain, leaf first
    key: await file("/etc/certs/privkey.pem").text(),
  },
  (request) => new Response(request.url), // https://…
);
```

`request.url` reports the `https:` scheme — that comes from the listener, so a
client cannot talk a plain server into claiming it via a `Host` header. The host
part *is* the client's `Host` (or `:authority` on HTTP/2), but only if it is one:
a value that is not a bare host and optional port — one carrying a path, a query,
or userinfo — is answered `400` and never reaches the handler, because such a
value spliced into a URL changes the path the handler would route on. `alpn`
defaults to `["h2", "http/1.1"]` — both versions this server speaks, h2 first
because ALPN order is the server's preference.

An unparseable certificate or key fails the `serve` call itself rather than each
later handshake, and `secureTransport: "on"` without both a `cert` and a `key`
is a `TypeError` — binding a port that rejects every connection would look like
a working server nothing can reach. A failed handshake ends that connection
only; on a public port those are routine and must not stop the server.

#### HTTP/2

The version is negotiated per connection and the handler never sees it: the same
`Request` in, the same `Response` out, whichever version carried it.

- **Over TLS** it is ALPN. `serve` offers `["h2", "http/1.1"]` by default, so an
  h2 client gets HTTP/2 and everything else gets HTTP/1.1. Naming `alpn`
  explicitly narrows that — `alpn: ["http/1.1"]` pins a listener to HTTP/1.1 for
  a client that mishandles h2.
- **On a cleartext port** an HTTP/2 client is served **h2c by prior knowledge**:
  a connection opening with the HTTP/2 preface is read as HTTP/2, anything else
  as HTTP/1.1. There is no `Upgrade:`-header dance — that mechanism is
  deprecated, and no client relies on it. This is what a reverse proxy or a gRPC
  client terminating TLS in front of the runtime speaks.

What HTTP/2 buys, given the handler is unchanged: requests **multiplex** over one
connection (many in flight at once, answered in any order — the runtime already
hands responses back per request, not per connection), one TLS handshake serves a
whole session, and headers are HPACK-compressed. Concurrency per connection is
capped at **256 streams**, advertised in the server's `SETTINGS`, so one peer
cannot open unbounded streams against a single-threaded isolate.

Whether that is a win depends entirely on how the client connects, and it is
worth being concrete rather than assuming h2 is faster (`bench/http2.sh`, same
server, best of 3 interleaved repetitions, only the version changed):

| client shape | HTTP/1.1 | HTTP/2 |
| --- | --- | --- |
| 50 connections × 1 stream | 66,939 req/s | 53,080 req/s (**0.79×**) |
| 1 connection × 50 streams | 20,157 req/s | 73,541 req/s (**3.65×**) |

With 50 sockets already open there is nothing to multiplex and HTTP/2 is pure
framing overhead — it *loses*. On one connection, where HTTP/1.1 is strictly
serial, 73,541 req/s is both a 3.65× gain and the fastest of the four runtimes
measured on that shape (Bun 49,142, Node 39,700, Deno 39,209 — see
`bench/README.md`, which also explains which comparisons in that table are
apples-to-apples and which are not). That second shape is the one a reverse
proxy, an API gateway, or a gRPC client is in.

`request.url` is rebuilt from `:authority` on HTTP/2, which is the version's
replacement for the `Host` header — one URL shape either way. Framing stays the
server's job on both versions: a handler's own `Content-Length` /
`Transfer-Encoding` are dropped, and HTTP/2 — which frames bodies itself and
forbids `Transfer-Encoding` outright — never sees a chunked encoding.

#### Trailers

Header fields that follow the body — where gRPC carries the status of a call,
because it is not known until the body has been produced.

```js
import { serve, withTrailers, trailersOf } from "runtime:http";

// Sending: the value may be a promise, which is the point of trailers.
serve({ port: 8080 }, () =>
  withTrailers(new Response(body), { "grpc-status": "0" }));

// Reading, after the body — trailers are not on the wire before it ends.
const response = await fetch(url);
await response.text();
const status = (await trailersOf(response)).get("grpc-status");
```

| Export | Signature | Notes |
| --- | --- | --- |
| `withTrailers(response, trailers)` | `(Response, HeadersInit \| Promise<HeadersInit>) => Response` | Returns the same `Response`. |
| `trailersOf(response)` | `(Response) => Promise<Headers>` | Empty `Headers` when there are none; never hangs. |

These are `runtime:http` exports rather than members of `Response`, because
trailers are **not part of the Fetch API** and no runtime exposes them there —
`Deno.serve` and `Bun.serve` cannot send them, and no runtime's `fetch` can read
them. A non-standard property on a standard object would mean code written here
silently does nothing elsewhere; an import shows the dependency.

Both HTTP versions carry them, so a handler does not produce a different
response depending on what the client negotiated. Two wire rules are worth
knowing:

- **HTTP/1.1 sends only the fields named in the response's `Trailer` header.**
  That header is added for you whenever the names are known before the head goes
  out — everything except a promise attached to a *streaming* body, where the
  head has already been sent. Declare `Trailer` yourself there.
- **A conformant HTTP/1.1 server sends trailers only to a client that asked**
  (`TE: trailers`, RFC 9110 §10.1.4). `fetch` sends that header, so trailers
  arrive; a client that does not will not see them, which is correct rather than
  a fault.

A trailered response is always chunked — a `Content-Length` response has nowhere
to put a trailer section.

#### Timeouts

A connection that is not making progress is closed. Nothing about this is
visible to a handler, and there is nothing to switch on:

| Option | Default | What it bounds |
| --- | --- | --- |
| `timeouts.handshake` | `10000`ms | From accept until the connection can carry requests: the TLS handshake, and the wait for the first byte the HTTP version is read from. A TLS connection passes both stages, so it may take up to twice this before it counts as established. |
| `timeouts.headerRead` | `30000`ms | How long a request head may take to arrive in full — and on HTTP/1.1, the **idle keep-alive limit** too. |
| `timeouts.h2KeepAlive` | `20000`ms | How often an idle HTTP/2 connection is probed with a PING, and how long the ACK may take. A dead peer is reclaimed within twice this. |
| `timeouts.bodyRead` | `30000`ms | How long a request **body** may take, before the allowance `bodyMinRate` earns it. |
| `timeouts.bodyMinRate` | `1024` B/s | Bytes per second that extend `bodyRead`. A floor to beat, not a rate to sustain; `0` earns nothing, making `bodyRead` a flat cap. |

Each is a number of milliseconds (`bodyMinRate` a number of bytes per second);
`null` disables it; omitting it keeps the
default. They are on by default because a timeout nobody configures protects
nobody — without them, a peer that completes the TCP handshake and then says
nothing holds a task and a descriptor for as long as it likes, at the cost of
one syscall to it.

```js
serve({ port: 8080, timeouts: { headerRead: 5000, h2KeepAlive: null } }, handler);
```

They bound only connections that are **idle or stalled**. A request in flight
and a response still streaming are never interrupted, however long they take — a
live feed or a slow download is unaffected by `headerRead` no matter how far past
it runs.

A request **body** is the one case elapsed time cannot judge. `headerRead` stops
when the head is complete, so a peer that sends a well-formed head and then
dribbles its body a byte at a time is past every other timer here — slowloris,
one phase later. A flat cap cannot separate it from a 100 MiB upload on a slow
link, since the two take the same length of time; what separates them is how much
they send while taking it. So `bodyRead` is an allowance a body **earns**:

```text
deadline = bodyRead + received / bodyMinRate
```

At the defaults a 100 MiB upload has over a day to arrive, a 1 GiB one over a
week, and a peer sending one byte a minute is closed at ~30s having earned about
10 milliseconds. An upload extends its own deadline by uploading; a dribbler
cannot. The failure reaches the handler as the body stream erroring with
`ERR_TIMED_OUT`, naming what arrived and how slowly.

Two consequences worth knowing before you tune them:

- On HTTP/1.1 the idle keep-alive limit and the request-head limit are **one
  timer**, because waiting for the next request on a kept-alive connection *is*
  waiting for a request head. At the default an idle connection is closed after
  30s and a client that wants another request opens a new one — the same posture
  as nginx (75s) and Node (5s).
- HTTP/2 keeps its connections open by design and has no idle limit, so it uses
  PING probes instead. Without them a peer that vanishes without a FIN — a NAT
  that dropped the mapping, a killed VM — keeps its connection *and* its share of
  the 256-stream budget until the OS TCP keepalive notices, which is two hours by
  default on Linux.

#### How many connections at once

#### Sharing a port across processes

`reusePort: true` binds with `SO_REUSEPORT`, so several **processes** can listen
on the same address and the kernel balances new connections across them. It is
how a server is run across cores without a front proxy, and how one is replaced
without dropping connections — the replacement binds alongside the outgoing
process before it exits.

```js
// Each of N processes runs this; every one must set the flag.
serve({ port: 8080, reusePort: true }, handler);
```

Every sharer has to set it: a plain bind on a port already held is still
`ERR_ADDRESS_IN_USE`, which is what makes the flag meaningful rather than a
no-op. **Unix only** — Windows has no equivalent (`SO_REUSEADDR` there lets an
*unrelated* process take a bound port, a hijacking primitive rather than a
load-balancing one), so asking for it on Windows is an error rather than a
silent exclusive bind that fails the moment you scale.

The same option is on [`runtime:net`](#runtimenet) `listen()`, and the two bind
on identical terms.

`maxConnections` caps how many connections the server holds at once. Unlimited
by default:

```js
serve({ port: 8080, maxConnections: 10_000 }, handler);
```

A connection over the cap is **held, not refused**. The server enforces the
limit by not accepting, so the connection waits in the kernel's backlog — and is
served as soon as a slot frees, rather than being dropped. Nothing is spent on
it while it waits: no descriptor, no task, no read buffer. (Once the backlog
itself fills, the OS refuses further connections, which is the only refusal in
the design and it costs the server nothing.)

There is no default because the right number follows from your file-descriptor
budget and the memory a connection costs — neither of which the runtime can
read. It is worth setting on a public port: an HTTP/1.1 connection's read buffer
can reach ~408KB, so the connection count multiplies straight into memory. Node,
Deno and Go leave this unlimited too.

`maxConnectionsPerIp` caps how many of those connections **one peer address** may
hold. Also unlimited by default:

```js
serve({ port: 8080, maxConnections: 10_000, maxConnectionsPerIp: 64 }, handler);
```

Without it, `maxConnections` bounds what the deployment spends and nothing else:
one peer opening every slot fills the server exactly as a thousand peers opening
one each do, and it is then full for everybody. This is the half that says
*whose* connections they are.

A connection over this is **refused**, not held — the opposite of the
whole-server cap, and deliberately. There the excess is legitimate traffic
queueing for a slot, and waiting costs nothing; here the excess is by definition
one client past its share, and a held connection is already accepted, so it costs
a descriptor and the peer decides when it ends. Closing returns both at once.

> **Behind a proxy, leave this off.** The count is per address, and every
> connection through a load balancer or a NAT gateway carries the same source —
> so a cap here is a cap on the whole service. Use the proxy's own per-client
> limits instead. This is why it is off by default, and why the number belongs to
> whoever knows what sits in front of the server.

The per-connection limits it multiplies against are fixed:

| Limit | Value |
| --- | --- |
| HTTP/1.1 header fields | 100 |
| HTTP/1.1 read buffer | ~408KB |
| HTTP/2 header list | 16KB (advertised in `SETTINGS`) |
| HTTP/2 concurrent streams | 256 (advertised in `SETTINGS`) |

#### Shutdown

`server.stop()` stops accepting and resolves once the accept loop has ended;
in-flight requests still complete.

`esrun` wires this to `^C` and `SIGTERM` for you, so a server does not need to
handle signals to shut down cleanly:

| Situation | What happens |
| --- | --- |
| A server is running | Stop accepting, drain in-flight requests, exit `128 + signal` (`130`/`143`) |
| No server is running | Exit immediately — there is nothing in flight to protect, and a plain script should still die instantly on `^C` |
| The guest installed a signal handler | `esrun` stays out of the way entirely; the handler owns shutdown |
| A second interrupt during the drain | Exit immediately |
| The drain outlasts `--shutdown-grace` | Exit anyway (default `10000`ms) |

Draining waits for the *connections* to close, not just for the handler to
return: a response is handed to the transport before it reaches the socket, and
exiting between those two points is what turns an in-flight request into an
empty reply.

To run your own cleanup instead, install a handler with
[`onSignal`](#signals) — that alone tells `esrun` to leave shutdown to you.

#### `request.signal` — the client hung up

`request.signal` aborts (with an `AbortError` `DOMException`) when the client
disconnects before the handler has produced a response. Expensive work that
nobody will read can then be abandoned, and it composes with anything else that
takes a signal:

```js
serve(async (request) => {
  // The upstream call is dropped the moment the caller goes away.
  const upstream = await fetch(slowUrl, { signal: request.signal });
  return new Response(upstream.body);
});
```

Reading `request.signal` is what starts the watch on the connection, so a
handler that never asks costs nothing — the same deal as `request.headers`. The
signal covers the window *before* the response is handed over; a client that
vanishes partway through a streamed response body instead ends that stream.

An embedder's own `HttpServerProvider` opts in by implementing
`request_disconnected`; one that does not gets a signal that simply never fires.

---

## `runtime:websocket`

The WebSocket **server** side (DECISIONS D29). The *client* is the global
[`WebSocket`](#websocket); serving is capability-gated host I/O, so it lives in a
`runtime:` module like `runtime:net` `listen()`. `serve()` requires `NetListen`
and returns a `WebSocketServer` — an async-iterable of accepted, already-open
server-side connections. `ws:` only (a `wss:` server is a follow-up).

```js
import { serve } from "runtime:websocket";

const clients = new Set();
const server = serve({ hostname: "127.0.0.1", port: 4001 });
for await (const ws of server) {
  clients.add(ws);
  ws.addEventListener("message", (e) => {
    for (const c of clients) c.send(e.data); // broadcast (a chat room)
  });
  ws.addEventListener("close", () => clients.delete(ws));
}
```

### Exports

| Export            | Type                                   | Description                                                |
| ----------------- | -------------------------------------- | ---------------------------------------------------------- |
| `serve(options)`  | `({ hostname?, port, timeouts?, maxConnections?, maxConnectionsPerIp?, maxBufferedAmount? }) => WebSocketServer` | Bind a WebSocket server on its own port; `port` 0 picks an ephemeral one. `NetListen`. |
| `upgradeWebSocket(request, options?)` | `(Request, { protocol? }) => { response, socket }` | Turn a `runtime:http` request into a WebSocket, so one port serves `https:` and `wss:` together. No capability of its own — the port was already bound under `NetListen`. |
| `broadcast(connections, data)` | `(Iterable<conn>, string \| BufferSource \| Blob) => void` | Send one message to many connections in a single host crossing (the batched form of a `.send()` loop). A closed connection is skipped; an element that is not a connection is a `TypeError`, checked before anything is sent. |

| Option | Default | Description |
| ------ | ------- | ----------- |
| `timeouts.handshake` | `10000` | Milliseconds from accept until the opening handshake completes; `null` disables. RFC 6455's handshake is an HTTP request head, so this is the slowloris bound on the same bytes. It never touches an **established** connection — a socket that has said nothing for a week is idle, not stalled. |
| `maxConnections` | unlimited | The most connections to hold at once. A connection over the cap is **held, not refused**: it waits in the kernel backlog and is served once a slot frees. Worth setting on a public port — WebSocket connections are long-lived by design, so unlike an HTTP server's the count does not fall back down on its own. |
| `maxConnectionsPerIp` | unlimited | The most connections **one peer address** may hold. Without it, `maxConnections` says nothing about whose connections fill it — one peer can take every slot. A connection over this is **refused**, not held. Leave it off behind a proxy or a NAT, where every connection shares one source address. |
| `maxBufferedAmount` | `8388608` (8 MiB) | The most bytes that may sit queued for one connection before it is closed with `1013`. **On by default**, unlike the caps above: the number does not depend on what the deployment knows. `0` removes it. |

The connection spellings match `runtime:http`'s `serve()`, and the numbers live
only in the host — the prelude sends "unset", so there is one copy to keep true.

### Backpressure

`send()` is fire-and-forget: the WebSocket API has no way to report a full
buffer, so writing faster than a peer reads never stalls your code. The messages
queue on the host instead, one pending send each — and a peer that has stopped
reading a fan-out is then a memory leak with a network interface.

Two things answer that. **`connection.bufferedAmount`** is the bytes handed to
`send()` that the host has not taken yet, so a sender can feel the peer:

```js
for await (const chunk of source) {
  if (conn.bufferedAmount > 1 << 20) break; // this peer is a megabyte behind
  conn.send(chunk);
}
```

And **`maxBufferedAmount`** is what happens when nobody looks: past it the host
closes that connection with `1013` (Try Again Later) rather than hold more, so
one slow peer costs a connection instead of the process. It applies per
connection, including the ones `broadcast()` fans out to — a broadcast is a list
of sends, and the one peer that has stopped reading is the one that is closed for
it. Client connections opened with `new WebSocket(...)` carry the same default;
a slow server is the same problem from the other end.

**`WebSocketServer`** — async-iterable of server connections;
`addr: Promise<{ hostname, port }>`, `accept(): Promise<conn | null>`,
`close(): Promise<void>`.

**connection** (each accepted socket) — already open: `send(data)`
(`string`/`Blob`/`ArrayBuffer`/`ArrayBufferView`), `close(code?, reason?)`,
`binaryType`, `bufferedAmount`, `protocol`, and `message`/`close` events (`on*`
or `addEventListener`) — the same surface as the client `WebSocket`, minus the
connecting handshake.

For chat-style fan-out, prefer **`broadcast(connections, data)`** over a
`.send()` loop: it makes one host crossing and one payload copy for the whole
room, enqueues to every connection concurrently (a slow peer can't stall the
rest), and coalesces the writes — so delivery stays full. A `wss:` server and
pub/sub topics are follow-ups (D29).

## `runtime:serialization`

A high-performance parsing and serialization module for structured data formats: XML, YAML, TOML, JSONL, MessagePack, and Protobuf. The text/binary parsers are backed by optimized Rust implementations; Protobuf is a pure-JS reflective implementation. All are exposed via zero-cost host boundaries.

- **Capability:** None (pure computation)
- **Status:** Available

```js
import { XML, YAML, TOML, MessagePack, Protobuf } from "runtime:serialization";

const obj = XML.parse("<root><hello>world</hello></root>");
const yaml = YAML.parse("hello: world");
const msgpackBytes = new Uint8Array([0x81, 0xa5, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0xa5, 0x77, 0x6f, 0x72, 0x6c, 0x64]);
const obj2 = MessagePack.decode(msgpackBytes);

const schema = new Protobuf.Schema(`
  syntax = "proto3";
  message Hello { string name = 1; }
`);
const pbBytes = schema.encode("Hello", { name: "world" });
const pbObj = schema.decode("Hello", pbBytes); // { name: "world" }
```

### Exports

For each string format (XML, YAML, TOML), the module provides a namespace with three methods:

| Export | Description |
| --- | --- |
| `<Format>.parse(data)` | Parses the given format into a JavaScript object. |
| `<Format>.build(obj)` | Serializes a JavaScript object into the given format. |
| `<Format>.validate(data, opts?)` | Validates the given data without full allocation. `opts.detailed` provides `{ valid: boolean, error: string }`. |

`XML` requires a **well-formed** document: every element closed, and no input
left over inside an open one. A truncated document (`"<r>"`, `"<r><a>1"`) is a
`SyntaxError` from `parse` and `false` from `validate` — the two always agree.

For binary formats like MessagePack, the namespace is slightly different:

| Export | Description |
| --- | --- |
| `MessagePack.decode(bytes)` | Parses a MessagePack byte array into a JavaScript object. |
| `MessagePack.encode(obj)` | Serializes a JavaScript object into a MessagePack `Uint8Array`. |
| `MessagePack.validate(bytes, opts?)` | Validates the given byte array. |

**MessagePack type mapping.** A `Uint8Array` (and any other typed-array view,
and an `ArrayBuffer`) encodes as the **`bin`** family and decodes back to a
`Uint8Array` — binary is the reason to reach for the format, so it round-trips
rather than degrading to an array of numbers. An `ext` value decodes to its
payload bytes; there is no JS type that corresponds to one, and inventing a
wrapper would make it indistinguishable from real data.

Values with no own enumerable properties are converted rather than flattened to
`{}`: a `Map` encodes as a map (keys stringified), a `Set` as an array, and a
`Date` as its ISO-8601 string — so a `Date` **round-trips as a string**, not as
a `Date`. Anything with no MessagePack representation at all (a function, a
symbol, a `BigInt`) **throws** a `TypeError`; the encoder never substitutes
`nil` for a value it could not represent.

For JSONL, it provides transform streams under the `JSONL` namespace:

| Export | Description |
| --- | --- |
| `new JSONL.DecoderStream()` | A `TransformStream` that parses lines of JSON. |
| `new JSONL.EncoderStream()` | A `TransformStream` that stringifies objects to JSON lines. |

For XML, it also provides a `DecoderStream`:

| Export | Description |
| --- | --- |
| `new XML.DecoderStream()` | A `TransformStream` that parses XML chunks. |

For Protobuf, schemas are compiled from `.proto` source at runtime (pure JS, reflective — proto3 and editions 2023/2024; proto2-only constructs are rejected):

| Export | Description |
| --- | --- |
| `default` | An aggregate of all named exports (`XML`, `YAML`, `TOML`, `JSONL`, `MessagePack`, `Protobuf`), as every other `runtime:` module provides. |

For Protobuf, schemas are compiled from `.proto` source at runtime:

| Export | Description |
| --- | --- |
| `new Protobuf.Schema(proto, opts?)` | Compiles a `.proto` source string (or a `{ filename: source }` map for multi-file schemas with `import`s; the `google/protobuf/*` well-known types resolve automatically). |
| `Protobuf.Schema.fromDescriptorSet(bytes)` | Builds a `Schema` from a compiled `FileDescriptorSet` (`protoc --descriptor_set_out`, ideally with `--include_imports`) instead of `.proto` source. |
| `schema.decode(messageName, bytes)` | Decodes a `Uint8Array` for the fully-qualified `messageName`. |
| `schema.encode(messageName, value, options?)` | Encodes a JavaScript object into a `Uint8Array`. Field names may be `snake_case` (as in the `.proto`) or `camelCase`; a key matching no field throws unless `options.ignoreUnknownFields` is set. |
| `schema.encodeDelimited(messageName, value)` | Encodes one length-delimited message (varint length prefix + bytes — the `writeDelimitedTo` framing). |
| `schema.decodeDelimited(messageName, source)` | Async generator over a length-delimited stream of messages from a chunked byte `source` (`ReadableStream`, async/sync iterable, or `Uint8Array`). |
| `schema.toJson(messageName, value)` | Converts a decoded value to its canonical proto3-JSON representation. |
| `schema.fromJson(messageName, json)` | Parses canonical proto3-JSON into the decoded value shape (ready for `encode`). |
| `schema.decodeStream(messageName, fieldName, source)` | Async generator that streams the elements of a repeated message field from a chunked byte `source` (a `ReadableStream` or async/sync iterable of `Uint8Array`), yielding each element as it arrives and skipping the other fields. |

Decoded value shape: camelCase field names; 64-bit integer fields (`int64`/`uint64`/`sint64`/`fixed64`/`sfixed64`) as **BigInt**; enums as their value-name string (unknown numbers kept as numbers); `bytes` as `Uint8Array`; maps as plain objects; nested messages as plain objects. Fields absent on the wire are omitted.

In the proto3-JSON form, 64-bit integers and `bytes` become strings (base64 for `bytes`), enums their value-name string, and the well-known types take their special forms (Timestamp/Duration as strings, wrappers as bare values, Struct/Value/ListValue as native JSON, Any with an `@type` member, FieldMask as a comma path string, Empty as `{}`).

<!-- Reference links -->
[D27]: ./DECISIONS.md

## `runtime:hashing`

Digests, checksums, MACs and password hashing (DECISIONS D57). `crypto.subtle`
is the WebCrypto standard and stays exactly that; this is the rest of what a
server hashes for — the algorithms WebCrypto has no name for, hashing that runs
incrementally instead of all at once, encoded output, and passwords.

- **Capability:** None. Hashing reads nothing and reaches nothing, so every
  function works with nothing granted. The one exception is `password.hash()`,
  which draws a random salt from `crypto.getRandomValues` and therefore needs
  `Entropy`; `password.verify()` needs nothing, because the salt is inside the
  stored string.
- **Status:** Available

```js
import { hash, Hasher, hashStream, hmac, timingSafeEqual, password } from "runtime:hashing";

hash("sha256", "hello", "hex");            // "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
hash("xxhash3", buffer);                   // Uint8Array — a cache key, at a tenth of the cost

const h = new Hasher("blake3");            // hash what you cannot hold
for await (const chunk of file.stream()) h.update(chunk);
h.digest("hex");

const stored = await password.hash(input); // "$argon2id$v=19$m=19456,t=2,p=1$…"
await password.verify(input, stored);
```

### Algorithms

| Algorithm | Output | Notes |
| --- | --- | --- |
| `sha1`, `sha256`, `sha384`, `sha512` | 20/32/48/64 bytes | Also in `crypto.subtle`. WebCrypto's spellings (`SHA-256`) are accepted too. |
| `sha3-224`, `sha3-256`, `sha3-384`, `sha3-512` | 28/32/48/64 bytes | |
| `blake3` | 32 bytes | Fast, and the usual choice for hashing large content. |
| `md5` | 16 bytes | Broken for signatures. Present for interop — S3 ETags, CRAM-MD5. |
| `ripemd160` | 20 bytes | |
| `xxhash64`, `xxhash3` | 8 bytes | Not cryptographic. Cache keys, ETags, shard selection. |
| `crc32`, `crc32c` | 4 bytes | Not cryptographic. Checksums and framing. |

Names are case-insensitive. Checksums are refused by `hmac`, which needs a
cryptographic hash.

### Exports

| Export | Description |
| --- | --- |
| `hash(algorithm, data, encoding?)` | The digest of `data`, in one call. `data` is a string (hashed as UTF-8), an `ArrayBuffer`, or a view. |
| `new Hasher(algorithm)` | A hash computed across many chunks. `.update(data)` chains; `.digest(encoding?)` ends it. `.algorithm` reads it back. |
| `hashStream(algorithm, stream, encoding?)` | The digest of a `ReadableStream`, read to the end. Async. |
| `hmac(algorithm, key, data, encoding?)` | HMAC (RFC 2104), synchronous and in one call. For a `CryptoKey` or a JWK you already hold, use `crypto.subtle` — it is the same construction. |
| `timingSafeEqual(a, b)` | Constant-time comparison. Lengths are compared first, in ordinary time. |
| `password.hash(input, options?)` | Hashes a password, returning the string to store. Async. Needs `Entropy`. |
| `password.verify(input, stored)` | Whether `input` is the password `stored` was made from. Async. Needs nothing. |
| `password.needsRehash(stored, options?)` | Whether `stored` was written with weaker settings than `options` asks for. |

`encoding` is `"hex"`, `"base64"` or `"base64url"` for a string; omit it (or
pass `"bytes"`) for a `Uint8Array`. Encoding happens in the host, which is why
it is offered here rather than left to a loop at the call site.

A `Hasher` ends at its `digest()`: the host state is released, and calling
either method again throws rather than silently starting a second hash.

### Passwords

Argon2id by default; `bcrypt` and `scrypt` for hashes that already exist.

| Option | Applies to | Default |
| --- | --- | --- |
| `algorithm` | all | `"argon2id"` (also `"argon2i"`, `"argon2d"`, `"bcrypt"`, `"scrypt"`) |
| `memoryCost` | argon2 | `19456` (KiB) |
| `timeCost` | argon2 | `2` passes (`3` for argon2i) |
| `parallelism` | argon2, scrypt | `1` |
| `cost` | bcrypt, scrypt | `12` (bcrypt rounds, log₂) / `17` (scrypt N, log₂) |
| `blockSize` | scrypt | `8` |
| `salt` | all | 16 random bytes. Supply one only to reproduce a specific hash in a test. |

The defaults follow the OWASP Password Storage Cheat Sheet and live in the
module's source, in the open, because raising them is a decision to make
deliberately.

The stored string carries the algorithm, the parameters and the salt, so
nothing else has to be kept beside it — and verification reads them from the
string rather than from today's configuration. A hash written under weaker
settings therefore keeps verifying, which is what makes raising the settings
possible at all. `needsRehash()` is the companion: a correct login is the one
moment the plaintext is in hand, and so the only moment an old hash can be
replaced.

```js
if (await password.verify(input, user.hash)) {
  if (password.needsRehash(user.hash)) user.hash = await password.hash(input);
}
```

Two things to know. **These are slow on the thread that calls them** — that is
the entire mechanism — so a login endpoint under load wants a queue in front of
it, not a hundred concurrent calls. And **bcrypt refuses a password longer than
71 bytes** rather than truncating it, because truncating quietly makes two
different passwords the same password; verification still truncates, since a
stored hash may have been written by an implementation that did.

## `runtime:wasi`

WASI preview 1 (`wasi_snapshot_preview1`) — enough of the ABI to run what the
`wasm32-wasip1` toolchains emit: arguments, environment, clocks, randomness,
stdio, process exit, and the filesystem.

```js
import { WASI } from "runtime:wasi";
import { file } from "runtime:fs";

const wasi = new WASI({
  args: ["prog", "--flag"],
  env: { LOG: "debug" },
  preopens: { "/sandbox": "./data" }, // the only files the guest can reach
});
const bytes = await file("./prog.wasm").bytes();
const { instance } = await WebAssembly.instantiate(bytes, wasi.getImportObject());

const status = wasi.start(instance); // runs `_start`, returns the exit status
```

### Exports

| Export                    | Type                                | Description                                                                 |
| ------------------------- | ----------------------------------- | --------------------------------------------------------------------------- |
| `WASI`                    | `class`                             | `new WASI({ args?, env?, preopens?, version? })`; `version` must be `"preview1"`. |
| `wasi.getImportObject()`  | `() => object`                      | The `wasi_snapshot_preview1` import object to instantiate with.              |
| `wasi.start(instance)`    | `(Instance) => number`              | Runs a command module's `_start`; returns the exit status.                   |
| `wasi.initialize(instance)` | `(Instance) => void`              | Runs a reactor module's `_initialize`, leaving the instance live.            |

`start` returns `0` when `_start` returns normally, or the code passed to
`proc_exit`. A genuine fault still throws.

### No ambient authority

Unlike Node's `node:wasi`, **arguments and environment come only from the
constructor**. There is no path by which a wasm module reads the host's real
environment through this API, so constructing a `WASI` needs no capability and
inherits nothing. Forward the real environment explicitly if you want it — via
the `Env`-gated `runtime:process` — which makes that grant visible at the call
site.

This is the difference Node's own documentation is careful about: it states that
its threat model "does not provide secure sandboxing" and that WASI capabilities
there "do not form a security model". Here the sandbox is the runtime's, and a
wasm module reaches exactly as far as the imports you hand it.

### Filesystem

A guest sees only what `preopens` maps in — WASI's own model, and the reason its
file calls are all relative to a directory fd:

```js
const wasi = new WASI({
  preopens: { "/sandbox": "./data" }, // guest path → host path
});
```

Reaching a file passes **three** independent checks:

| Check | Enforced by | Failure |
| --- | --- | --- |
| The preopen maps the path, and it does not climb out of it | `runtime:wasi` | `ENOTCAPABLE` |
| `FileRead` / `FileWrite` is granted | the host op (D7) | `ENOTCAPABLE` |
| The resolved path is inside the root jail | the provider (D25) | `ENOTCAPABLE` |

Preopens are also isolated from each other: `../` out of `/a` cannot reach `/b`,
even though both are granted.

Implemented: `path_open`, `fd_read`, `fd_write`, `fd_seek`, `fd_tell`,
`fd_close`, `fd_fdstat_get`, `fd_filestat_get`, `path_filestat_get`,
`fd_readdir`, `fd_prestat_get`, `fd_prestat_dir_name`, `path_create_directory`,
`path_unlink_file`, `path_remove_directory`, `path_rename`.

Not implemented (report `ENOTCAPABLE`): `fd_pread`/`fd_pwrite`, `path_link`,
`path_symlink`, `path_readlink`, the `*_set_times`/`set_size` calls, and the
sockets. Every import is *present* regardless — a missing import is a
`LinkError` at instantiation, which would break a program that merely links a
symbol without calling it.

Because the syscalls are synchronous, these go through blocking host ops that
occupy the runtime's thread for the duration of the call. Embedders wire this up
with `HostProviders::with_sync_file_system`; without one, every file call
reports `ENOTCAPABLE`.

Stdout and stderr are line-buffered through the console sink, with any
unterminated trailing write flushed when the program finishes. Stdin reads as an
immediate end-of-file.

---

## `runtime:system`

Child processes (DECISIONS D37). A command is a **program plus an argv** — there
is no shell, so nothing is word-split, glob-expanded, or re-parsed, and a
guest-supplied argument can never become a second command. Output moves over web
streams, so a child's stdout can be handed straight to `new Response(...)` and a
request body can be piped straight into its stdin.

- **Capability:** `Run` (spawning). `inheritEnv: true` additionally needs `Env`.
- **Status:** Available
- **Loading:** on demand.

```js
import { Command } from "runtime:system";

const { success, code, stdout } = await new Command("git", {
  args: ["rev-parse", "HEAD"],
}).output();
```

### Exports

| Export | Type | Description |
| --- | --- | --- |
| `Command` | `class` | A command to run: `new Command(program, options?)`. |
| `ChildProcess` | `class` | A running child, from `spawn()`. Not constructed directly. |
| `default` | `object` | An aggregate bundling both. |

### `new Command(program, options?)`

`program` is a path (absolute, or relative to `cwd`) or a bare name looked up on
the **host** `PATH`. Program lookup is host authority: the `env` you pass
describes the child's environment, never where the runtime looks for
executables. On Windows the `PATHEXT` spellings are tried too; `.bat`/`.cmd`
files are refused, because running one requires handing the command interpreter
a line to re-parse (CVE-2024-27980) and this runtime does not spawn a shell.

| Option | Type | Default | Description |
| --- | --- | --- | --- |
| `args` | `(string \| number \| boolean)[]` | `[]` | Passed verbatim — no quoting or escaping needed. |
| `cwd` | `string \| URL` | the parent's | The child's working directory. |
| `env` | `Record<string, string \| Secret \| undefined>` | `{}` | The child's environment. A `Secret` from `runtime:process` is unwrapped for the child (it would otherwise arrive as the literal `"[redacted]"`); `undefined` removes an inherited key. |
| `inheritEnv` | `boolean` | `false` | Start from the host environment. **Needs `Env` as well as `Run`.** |
| `stdin` | `"null" \| "piped" \| "inherit"`, or a body | `"null"` | A body — string, bytes, `Blob`, `Response`, or `ReadableStream` — is written to the child and stdin then closed. |
| `stdout` / `stderr` | `"piped" \| "inherit" \| "null"` | `"piped"` | How the output is connected. |
| `signal` | `AbortSignal` | — | Aborting kills the child; `output()` rejects with the abort reason. |
| `timeout` | `number` (ms) | — | Kill the child after this long. `output()` rejects with a `TimeoutError`. |
| `killSignal` | `SignalName` | `"SIGTERM"` | The signal used by `kill()`, a timeout, or an abort. |
| `maxBuffer` | `number` (bytes) | `16777216` | `output()` only: past this, the child is killed and the call throws `ERR_MAX_BUFFER`. |

| Method | Type | Description |
| --- | --- | --- |
| `output()` | `Promise<CommandOutput>` | Run to completion, collecting output. Both streams are read **while** the child runs, so a child that fills its pipe cannot deadlock against the wait. |
| `spawn()` | `Promise<ChildProcess>` | Start the child. **Async** (unlike Deno's sync `spawn()`): a failure to *start* — no such program, permission denied — belongs to this call, not to a stream or a status settled later. |

`CommandOutput` is `{ success: boolean, code: number | null, signal: string |
null, stdout: Uint8Array, stderr: Uint8Array }`. `code` is `null` when a signal
ended the process; `signal` is its name.

### `ChildProcess`

| Member | Type | Description |
| --- | --- | --- |
| `pid` | `number` | The OS process id. |
| `stdin` | `WritableStream \| null` | `null` unless stdin was `"piped"`. Closing it is the child's EOF. |
| `stdout` / `stderr` | `ReadableStream<Uint8Array> \| null` | `null` unless that channel was `"piped"`. Pulled chunk by chunk, so a child that outruns its reader is stopped by a full pipe rather than buffered without limit. |
| `status` | `Promise<{ success, code, signal }>` | Resolves when the child exits. |
| `kill(signal?)` | `(SignalName?) => Promise<void>` | Defaults to `killSignal`. Signalling an already-exited child is a no-op, not an error. |
| `[Symbol.asyncDispose]()` | | `await using child = await cmd.spawn()` kills and reaps at the end of the scope. |

The streams are created on **first use**, and reading `status` is what keeps the
program alive until the child exits (a pending host op is pending work). So a
child nobody waits on holds nothing open: spawn it, ignore it, and the program
can still exit — the child is killed on the way out, never orphaned.

Only the direct child is signalled by `kill()`. A child that spawned its own
children does not pass it on, so grandchildren can outlive a kill.

### Not provided

| Absent | Why |
| --- | --- |
| A shell (`exec`, `shell: true`, `` $`…` ``) | The whole shell-injection class stays out. A `Command` takes an argv. |
| `fork()` / IPC | Worker processes with structured-clone messaging want their own design, not a flag on `Command`. |
| `stdio` arrays, raw fd numbers | An fd hands the guest authority over anything the host inherited — incompatible with the capability model. |
| `detached`, `uid`/`gid`, `argv0` | Privilege and identity manipulation belong to the embedder's provider, not to guest code. |
| Sync variants (`outputSync`) | Ops run inside the async runtime; a blocking spawn would stall the loop (the same wall DECISIONS D36 describes). |
| PTY | This runtime pipes; allocating a terminal is a different feature. |

### Embedding

`HostProviders::with_commands` installs a `CommandProvider`. Without one, the
`runtime:system` ops fail cleanly like a denied capability. The default
`SystemCommands` accepts a policy — `with_allowlist(["git", "ffmpeg"])` and
`with_max_children(n)` — for an embedder that must grant `Run` without granting
a shell.

---

## `runtime:build`

The bundler, callable from guest JavaScript.

- **Capability:** `FileRead`; `write()` also needs `FileWrite`.
- **Status:** Available under **`esdev` only**. `esrun` does not serve this
  module: importing it there fails at load with *unknown built-in module*.
- **Loading:** on demand. The bundler thread starts on the first `build()`.

rolldown is already inside `esdev` — it is what `esdev build` runs. What was
missing was a way for a *program* to reach it. Without that, a framework's dev
server has to import a bundler from npm, which is a napi addon this runtime does
not load, so the dev server has to be a Node program.

```js
import { build } from "runtime:build";

const bundle = await build({
  input: "app/main.jsx",
  external: (id) => id.startsWith("/__route/"),
  resolve: { alias: { "@": "./src" }, extensions: [".js", ".jsx"] },
  define: { "process.env.NODE_ENV": '"development"' },
  plugins: [mdx()],
});

const { output, watchFiles } = await bundle.generate({
  format: "esm",
  codeSplitting: false,
});
serve(output[0].code);          // never written to disk
```

### `build(options)`

| Option | Type | Default | Description |
| --- | --- | --- | --- |
| `input` | `string \| string[] \| Record<string, string>` | — | The entry, or entries. |
| `external` | `string[] \| (id, importer, resolved) => boolean` | — | What to leave unbundled. A predicate as well as a list: a dev server externalises a shape (`/__route/*`), not a set. |
| `platform` | `"neutral" \| "browser" \| "node"` | `neutral` | Which environment the output runs in; decides `exports` conditions — see below. |
| `resolve` | `{ alias, extensions, conditionNames, mainFields }` | — | Resolution. `conditionNames` is **appended** to the platform's; `mainFields` replaces. |
| `define` | `Record<string, string>` | — | Compile-time replacements. |
| `plugins` | `Plugin[]` | `[]` | See below. |
| `minify` / `treeshake` | `boolean` | `false` / `true` | |
| `cwd` | `string` | the entry's directory | Where the build runs. Resolved through the run's own filesystem view. |

Output options — `format`, `dir`, `file`, `codeSplitting`, `sourcemap`,
`entryFileNames`, `chunkFileNames`, `assetFileNames`, `banner`, `footer` — may
be given here or per call; the per-call ones win.

**Resolution defaults are the same ones `esdev build` asserts**, from the same
place, because a project that builds one way through the subcommand and another
way through this module is the worst kind of build bug: nothing fails, and the
bundle dies later on an import.

| `platform` | Conditions asserted | `mainFields` |
| --- | --- | --- |
| `neutral` (default) | `worker` | `["module", "main"]` |
| `browser` | `browser` | `["module", "main"]` |
| `node` | none of ours | the bundler's own |

`worker` is the key a Web-API-targeting package uses for the build that does not
reach for `node:` modules — React's `react-dom/server` resolves to its Web
Streams implementation under it, and to a `node:stream` one without. `browser`
is the other half of that: a client bundle built with `worker` asserted gets the
build that expects no `document`. The two are alternatives rather than
additions, because conditions match in the order the *package author* wrote them
(D40), so the wrong one being present at all is enough to win.

`mainFields` exists because a neutral platform leaves it empty, which fails to
resolve any package old enough to predate `exports` — and a good deal of the
registry is.

This is deliberately **not** the runtime's own condition set, which D40 keeps
standards-only. A condition changes which code runs, so the place to choose one
is a build somebody ran on purpose, not a server resolving imports under load.

### `Bundle`

| Member | Returns | Description |
| --- | --- | --- |
| `generate(output?)` | `Promise<BuildResult>` | Builds; chunks come back **in memory**. |
| `write(output?)` | `Promise<BuildResult>` | The same build, written under `dir`. |
| `close()` | `Promise<void>` | Releases the build. |
| `watchFiles` | `string[]` | What the last build read. |

A `BuildResult` is `{ output, watchFiles, warnings }`. An `output` entry is
either a chunk — `{ type: "chunk", fileName, name, code, isEntry,
isDynamicEntry, moduleIds, imports, dynamicImports, map }` — or an asset:
`{ type: "asset", fileName, source }`.

`watchFiles` is every file the build read **plus** every file a plugin declared
with `this.addWatchFile()`. Paired with [`runtime:watch`](#runtimewatch), it is
what lets a dev server drop the chunks a change invalidates and keep the rest.

### Plugins

The plugin system is **ours**, not the bundler's passed through: the `runtime:`
namespace is a versioned contract, and an API defined by a third party's trait
moves when that trait moves. rolldown is an implementation of what follows, not
the definition of it.

A plugin is an object with a `name` and hooks. **A hook is an object carrying a
`handler`** — one form, with rollup's bare-function shorthand refused, because
accepting it would make the filter, the order and the context argument optional
extras on somebody else's design.

```js
const mdx = {
  name: "mdx",
  transform: {
    filter: { id: /\.mdx$/ },
    handler(code, id, ctx) {
      const { js, meta } = compile(code, id);
      return { code: js, type: "jsx", dependsOn: [meta] };
    },
  },
};
```

| Hook | Handler | Returns |
| --- | --- | --- |
| `start` | `(ctx)` | `{ dependsOn }`, or nothing |
| `resolve` | `(source, importer, ctx)` | `{ id, external?, virtual? }`, or `null` |
| `load` | `(id, ctx)` | `{ code, type?, map?, dependsOn? }`, or `null` |
| `transform` | `(code, id, ctx)` | `{ code, type?, map?, dependsOn? }`, or `null` |
| `end` | `(error, ctx)` | nothing |

Five, against rollup's twenty-odd: each is a promise a future backend has to
keep, so the list is short deliberately. `null` means *not mine*; anything else
must be the object, and a bare string of code is refused with a message saying
so. `resolve` + `load` together are what makes a **virtual module** possible.

Each hook may declare, alongside its handler:

| Key | |
| --- | --- |
| `filter` | `{ id, code }` — a string (**exact**), a `RegExp`, or an array of either. `code` is `transform` only. Both given are anded. |
| `order` | `"pre"` or `"post"`, relative to the unordered plugins. |

**A filter is matched on the host's side, before anything crosses.** In rollup a
hook returning `null` costs a function call; here it costs a round trip into the
isolate, so an unfiltered `transform` is one crossing *per module in the graph*.
A pattern the host cannot evaluate — JavaScript's regular expressions are larger
than the matcher's — stops filtering rather than failing, because excluding
modules a plugin was meant to see is the expensive way to be wrong.

**`dependsOn` is returned, not declared.** Rollup's `this.addWatchFile()` is a
call you can forget, and forgetting it produces a build that serves stale
output. Relative paths resolve like every other path in a run and land in
`watchFiles` as the absolute path the graph reports.

**`virtual: true` replaces the NUL-byte prefix** rollup uses to mark an id with
no file behind it. The notation is the backend's business; a `load` filter still
matches the id the plugin named.

The context is the **last argument**, not `this` — an arrow-function handler
keeps it, where rollup's `this` is silently lost:

| Member | |
| --- | --- |
| `ctx.resolve(source, importer?)` | the bundler's resolver, mid-hook; `null` if nothing resolves |
| `ctx.emit({ type, … })` | adds a chunk or asset to a running build; returns a reference id |
| `ctx.warn` / `info` / `debug` | diagnostics; warnings come back in `warnings` |
| `ctx.error(msg)` | fails the build — throws |
| `ctx.isEntry` | on `resolve`: whether the specifier is an entry |

It is live only while its hook runs; stashing it and calling `resolve()` later
throws.

A plugin is **guest code**: it runs in the isolate under the same capability
model as the rest of the program, so a plugin that reads a file needs
`FileRead`. No other bundler's plugin API can state what a plugin is allowed to
do, because none of them has a capability model to state it in.

### What a backend must provide

Written down so that "the bundler could be replaced" is checkable rather than
hoped for. An implementation must be able to: resolve a specifier through an
outside party and accept an id with **no file behind it**; ask an outside party
for a module's contents and for a rewrite of them; accept, from either, a list
of files the module depends on that it could not have discovered; resolve a
specifier on demand mid-hook; accept an additional entry or asset while running;
and report, per chunk, the modules that went into it.

Seven. rolldown has all of them; esbuild has four, with no transform hook and no
chunk-level emit — so the cost of a swap is legible before it is paid.

The layering that makes this real: the contract is one module, the adapter is
another, and the adapter is the only place the bundler is named. Nothing in the
API above is expressed in a bundler's types.

### Where the work happens

The bundler runs on a **thread of its own**, with a multi-threaded runtime: its
graph walk is parallel, and putting it on the isolate's thread would serialize
it onto one core and stall the program whose dev server it is.

Plugin hooks cannot follow it there — a V8 isolate belongs to one thread — so
the direction is inverted. A hook posts a request and waits; the isolate's pump
(an ordinary async op) resolves with it, runs the JavaScript, and replies.
Several hooks are in flight at once, so the bundler's parallelism survives the
crossing. A hook that blocks the isolate *synchronously* blocks everything the
program is doing, its server included.

### What is not scoped

`--allow-read` bounds where a build may be **started**: `cwd` is resolved
through the run's own filesystem view. What the bundler reads from there — the
module graph, `node_modules` — it reads itself, with this process's authority
rather than through the jail. A module graph's extent is not knowable up front,
and a check that stopped at the first `node_modules` symlink would look like a
boundary without being one. Stated rather than implied.

---

## `runtime:test`

`test()` and the assertions `esdev test` runs.

- **Capability:** none. An assertion computes and reaches nothing; the tally is
  bookkeeping the process keeps about itself.
- **Status:** Available under **`esdev` only**. A test file is never a
  production artifact (D59), so `esrun` does not serve this module.
- **Loading:** on demand.

```js
import { test, assert, assertEquals, assertThrows, assertRejects } from "runtime:test";

test("adds", () => assertEquals(add(2, 3), 5));
test("fetches", async () => assertEquals((await get("/")).status, 200));
```

These were **globals** until now, prepended to each test file's own source.
Importing them fixes three things at once: the runtime hands out no ambient
names anywhere else; a helper module beside the test file can now use the
assertions (only the entry was ever wrapped); and they have types, so a `.ts`
test file no longer references five undeclared names.

Run a file with `esdev test`, or directly — `esdev app.test.ts` prints the same
report, because what makes a run a test run is the module it imported.

### `test(name, fn)`

Registers a test and **starts it immediately**; `fn` may be `async`. Tests are
not queued, so one awaiting a timer does not hold up the next. `esdev` prints
the tally once the program reaches quiescence and exits non-zero if anything
failed.

A test that never settles is reported as a failure — *"the test never finished"*
— rather than left out of a green run.

### Assertions

| Function | |
| --- | --- |
| `assert(condition, message?)` | Fails unless `condition` is truthy. |
| `assertEquals(actual, expected, message?)` | Fails unless the two are **structurally** equal. |
| `assertThrows(fn, want?, message?)` | Fails unless `fn` throws, and unless the error matches `want`. |
| `assertRejects(fn, want?, message?)` | The async form. Returns a promise — `await` it. |

`assertEquals` walks the values rather than stringifying them: `BigInt` and
`NaN` through `Object.is`, typed arrays and `ArrayBuffer` byte by byte, `Map`
and `Set` by contents, `Date`/`RegExp`/`Error` by what identifies them, objects
by their key *set* rather than key order, and cycles terminate. Stringifying was
the original implementation and could not express the assertion an int64 test
most needs — `JSON.stringify` throws on a `BigInt`.

`want` is **what the error must be**, not a label: an error `name` or a
substring of its message, a `RegExp` over the message, or a constructor for an
`instanceof` check. The failure label is the third argument.

```js
assertThrows(() => parse(bad), TypeError);
assertThrows(() => parse(bad), "field number 0");
await assertRejects(() => fetchIt(), /timed out/, "the client should give up");
```

---

## `runtime:watch`

File-change events, delivered to guest JS.

- **Capability:** `FileRead`, scoped by the same `--allow-read` list as reading.
- **Status:** Available under **`esdev` only**. `esrun` does not serve this
  module: importing it there fails at load with *unknown built-in module*.
- **Loading:** on demand.

A watcher is development machinery — what it watches is source — so the binary
that serves production does not carry one. `esdev --watch` is the other watcher
in this toolchain, and answers a change by `SIGTERM`ing the program and starting
it again; this module exists for the case where that is the wrong answer. A dev
server holding compiled chunks, an open websocket to a browser and a warm
compile server cannot discard all of it because one file of forty changed. It
needs to be told **which** file.

```js
import { watch } from "runtime:watch";

const changes = watch(["app", "lib"], { recursive: true });

for await (const { kind, path } of changes) {
  if (kind === "removed") cache.delete(path);
  else invalidate(path);
  for (const dep of rebuild()) changes.add(dep);
}
```

### `watch(paths, options?)`

Returns a `Watcher` synchronously; the watch is opened on the first `await`, so
a path outside the sandbox root, or one `--allow-read` does not cover, surfaces
as a rejection at the point of use.

| Parameter | Type | Default | Description |
| --- | --- | --- | --- |
| `paths` | `string \| string[]` | — | What to watch. Relative paths resolve against the entry module's directory, like every other path in a run. |
| `options.recursive` | `boolean` | `false` | Watch the directories below these too. |

Non-recursive by default, matching the OS watchers: on Linux a recursive watch
costs a descriptor per directory, and watching one file should not cost the tree
it sits in.

### `Watcher`

| Member | Returns | Description |
| --- | --- | --- |
| `[Symbol.asyncIterator]()` | `AsyncIterator<Change>` | Iterates changes; leaving the loop closes the watcher. |
| `next()` | `Promise<Change \| null>` | The next change, or `null` once closed. |
| `add(path)` | `Promise<boolean>` | Watch another path. `false` if already watched. |
| `remove(path)` | `Promise<boolean>` | Stop watching one path. `false` if it was not watched. |
| `close()` | `Promise<void>` | End the watch and release its descriptors. Idempotent. |

The watch set is mutable because it is not knowable up front: which files a
bundle depends on is known only after it is built, so a shared `lib/` outside
the app directory starts being watched once a chunk proves it depends on it.

### `Change`

| Field | Type | Description |
| --- | --- | --- |
| `kind` | `"created" \| "modified" \| "removed"` | What happened. |
| `path` | `string` | The resolved absolute path, in the form `runtime:fs` reports. |

Three names, where the backends have dozens: the consumer's question is whether
what it cached still stands, and a name meaning one thing on Linux and another
on macOS is worse than none.

**Events are debounced per path.** One editor save is several filesystem events,
and acting on each of them means rebuilding three times — twice against a file
that was already finished. Each path's events are held for a quiet period and
delivered as what they add up to: create-then-write is a **create**, and
remove-then-create (every editor's atomic save) is a **modification**, because
the path existed before and exists now.

### Embedding

This module is not part of the runtime crate's namespace. It is registered by
`esdev` through `Runtime::register_module` + `register_op`, the same seam any
embedder can use to add a `runtime:` module its binary — and only its binary —
can honour. Nothing about the module pipeline or the capability check differs
for it.

---

## Error codes

Host-side failures carry a **stable string `code`** on the thrown exception —
the contract guest code branches on. Messages are human prose and may be
reworded at any time; codes never change meaning. An error with no stable
classification simply has no `code`, so test `e.code === "ERR_X"`, never
exhaustively.

```js
try {
  await file("config.json").text();
} catch (e) {
  if (e.code === "ERR_NOT_FOUND") return defaults;
  throw e;
}
```

| Code | Meaning |
| --- | --- |
| `ERR_CAPABILITY_DENIED` | A required capability was not granted (deny-by-default). |
| `ERR_FOREIGN_HANDLE` | A socket, child process, server, file descriptor or request belonging to another agent. Handles are usable only by the agent that created them. |
| `ERR_PROVIDER_UNAVAILABLE` | The backing provider for this API is not installed. |
| `ERR_NOT_FOUND` | The path does not exist. |
| `ERR_ALREADY_EXISTS` | The target already exists. |
| `ERR_PERMISSION_DENIED` | The OS denied access (distinct from a capability denial). |
| `ERR_IS_DIRECTORY` / `ERR_NOT_DIRECTORY` | A file op hit a directory / a directory op hit a non-directory. |
| `ERR_DIRECTORY_NOT_EMPTY` | The directory is not empty. |
| `ERR_JAIL_ESCAPE` | The real (canonicalized) path escapes the filesystem root jail. |
| `ERR_INVALID_PATH` | The path names no valid target: it is empty, or it is the root jail itself and the operation would mutate it. |
| `ERR_SAME_FILE` | Source and destination name the same file, for an operation that would have to read one while truncating the other. |
| `ERR_CONNECTION_REFUSED` | The peer refused the connection. |
| `ERR_CONNECTION_RESET` | The connection was reset/aborted by the peer. |
| `ERR_TIMED_OUT` | The operation timed out. |
| `ERR_ADDRESS_IN_USE` | The local address is already in use. |
| `ERR_UNREACHABLE` | The host or network is unreachable. |
| `ERR_DNS` | Name resolution failed. |
| `ERR_TLS` | TLS handshake or certificate verification failed. |
| `ERR_TOO_MANY_REDIRECTS` | A redirect chain exceeded the Fetch specification's cap of 20. |
| `ERR_CANCELLED` | The operation was cancelled. |

`runtime:db` adds a portable classification on top, so an application can branch
on what a database did without knowing which one said so. These sit on the same
`code` property; the backend's own code, where it had one, stays on
`e.backendCode`. A host code above (a denied capability, a jail escape) is
**not** replaced by one of these — the driver's classification is tried first,
then the host's, then `ERR_DB_BACKEND`.

| Code | Meaning |
| --- | --- |
| `ERR_DB_UNIQUE_VIOLATION` | A unique constraint or primary key collided. |
| `ERR_DB_FOREIGN_KEY_VIOLATION` | A foreign key constraint failed. |
| `ERR_DB_NOT_NULL_VIOLATION` | A `NOT NULL` column was given null. |
| `ERR_DB_CHECK_VIOLATION` | A `CHECK` constraint failed. |
| `ERR_DB_DEADLOCK` | The transaction was chosen as a deadlock victim. |
| `ERR_DB_SERIALIZATION_FAILURE` | The transaction could not be serialized; retry it. |
| `ERR_DB_BUSY` | The database is locked by another writer. |
| `ERR_DB_CONNECTION_LOST` | The connection went away mid-operation. |
| `ERR_DB_AUTH_FAILED` | The server refused the credentials. |
| `ERR_DB_TIMEOUT` | The database gave up on the statement. |
| `ERR_DB_SYNTAX` | The backend could not parse the statement. |
| `ERR_DB_UNDEFINED_TABLE` / `ERR_DB_UNDEFINED_COLUMN` | No such table / column. |
| `ERR_DB_READ_ONLY` | A write against a read-only database. |
| `ERR_DB_QUERY_FORM` | The query was handed in a form this backend does not take — SQL text to an engine that wants an AST, or the reverse. |
| `ERR_DB_UNSUPPORTED` | The backend, scheme, option or parameter type is not supported. |
| `ERR_DB_CLOSED` | The connection is closed. |
| `ERR_DB_CONNECTION_BUSY` | The connection is already streaming a result set. Distinct from `ERR_DB_BUSY`, which is the database refusing — this is the client's own connection, and only the caller draining that result can free it. |
| `ERR_DB_BACKEND` | The backend failed in a way with no portable name. Check `e.backendCode` and `e.message`. |
| `ERR_ENTROPY` | The entropy source failed. |
| `ERR_MAX_BUFFER` | A child process wrote more than `runtime:system` `output()`'s `maxBuffer`. |
| `ERR_IO` | An I/O failure with no finer classification. |

The code rides on whatever exception class the failure surfaces as (`Error`,
`TypeError`, `DOMException`, the `SocketError:`-prefixed `TypeError` of
`runtime:net`, …) as an own `code` property. The set may grow in a minor
release; existing codes are stable.

## Error Diagnostics

When exceptions are thrown by ES-Runtime during module evaluation or unhandled promise rejections, the original `Error` subclasses and their stack traces are preserved. The CLI automatically extracts these diagnostics and prints them elegantly with ANSI colors. The stack trace will highlight exact lines and columns of errors: `TypeError: message \n    at fn (file:line:col)`.
