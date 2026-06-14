# API Reference

The canonical reference for ES-Runtime's public host APIs. This is the
source of truth (DECISIONS [D27]); the marketing site under `site/app/docs/**`
mirrors it for the web. **A change to any public API updates both.**

ES-Runtime is ESM-only and deny-by-default. Host functionality is exposed as
ES modules under the `runtime:` scheme — never as ambient globals — and each
module's operations are gated on an explicit [`Capability`](#capabilities).

## Contents

- [Scope & non-goals](#scope--non-goals)
- [Web-standard globals](#web-standard-globals)
- [The `runtime:` scheme](#the-runtime-scheme)
- [Capabilities](#capabilities)
- [`runtime:process`](#runtimeprocess)
- [`runtime:path`](#runtimepath)

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
| JSON module imports      | `import x from "./x.json"` unsupported (no import attributes).      |
| Package installer        | Resolves an existing `node_modules`; does not install.             |
| Bundler / linter / formatter / test runner | Left to dedicated tools.                         |
| Watch mode               | No built-in file watcher / auto-restart.                           |
| FFI / native addons      | Host extends via injected providers + ops (Rust), not FFI.         |
| Workers / multi-thread   | Multi-isolate is the embeddable VM layer (Layer B), not a global.  |

See `site/app/docs/scope` for the rendered version.

## Web-standard globals

The global scope tracks the WinterTC Minimum Common Web Platform API. Host
capabilities (filesystem, process, network) are **not** globals — they live in
[`runtime:` modules](#the-runtime-scheme).

- **Core:** `globalThis`, `self`, `console`, `queueMicrotask`, `structuredClone`, `reportError`
- **Timers:** `setTimeout`, `clearTimeout`, `setInterval`, `clearInterval`
- **URL:** `URL`, `URLSearchParams`
- **Fetch:** `fetch`, `Request`, `Response`, `Headers`
- **Encoding:** `TextEncoder`, `TextDecoder`, `TextEncoderStream`, `TextDecoderStream`, `atob`, `btoa`
- **Streams:** `ReadableStream`, `WritableStream`, `TransformStream`, `ByteLengthQueuingStrategy`, `CountQueuingStrategy` (+ controllers/readers)
- **Crypto:** `crypto` (`getRandomValues`, `randomUUID`), `crypto.subtle` (digest, HMAC, AES-GCM/CBC/CTR, HKDF, PBKDF2), `CryptoKey`
- **Events:** `Event`, `EventTarget`, `CustomEvent`, `AbortController`, `AbortSignal`
- **Data:** `Blob`, `File`, `FormData`, `DOMException`
- **Performance:** `performance` (`now()`, `timeOrigin`)

**Not available:** `process`/`Buffer`/`require` (Node), `Worker`/`MessageChannel`,
`WebSocket` (not yet), `navigator`/`localStorage`/`window` (browser).

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
| `runtime:process` | Available   | `Env`      | [↓](#runtimeprocess)          |
| `runtime:path`    | Available   | `Env`*     | [↓](#runtimepath)             |
| `runtime:fs`      | Planned     | `FileRead` / `FileWrite` | —               |
| `runtime:net`     | Planned     | `Net`      | —                             |
| `runtime:http`    | Planned     | `Net`      | —                             |

---

## Capabilities

ES-Runtime is deny-by-default: a fresh runtime can compute but cannot reach the
host environment, filesystem, or network until the embedder grants the relevant
capability. The standalone `esrun` CLI grants the capabilities its features
need. The check lives on the native op, so it cannot be bypassed by reaching a
different module path.

| Capability  | Grants                                                              |
| ----------- | ------------------------------------------------------------------- |
| `Env`       | Environment, arguments, cwd, platform — backs `runtime:process`.    |
| `FileRead`  | Read files within the configured root jail.                         |
| `FileWrite` | Write files within the configured root jail.                        |
| `Net`       | Open outbound network connections.                                  |
| `HrTime`    | Access high-resolution timing.                                      |

Filesystem access (including module resolution) is confined to a project **root
jail**, on by default and not currently optional (DECISIONS D25). Paths are
canonicalized to their real location before the check, so a symlink cannot
escape the jail.

---

## `runtime:process`

Host process information: environment, arguments, working directory, platform,
and exit. Aligned *in spirit* with the WinterTC CLI-API proposal (DECISIONS
D26).

- **Capability:** `Env`
- **Status:** Available
- **Loading:** on demand — importing it adds nothing to startup if unused.
- **Snapshotting:** values are captured when the module is evaluated.

```js
import { env, args, platform, arch, cwd, exit } from "runtime:process";
// Or the default aggregate:
import process from "runtime:process";
```

### Exports

| Export            | Type                       | Description                                                                                                                                                                              |
| ----------------- | -------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `env`             | `object`                   | Environment variables as a **mutable in-process object**, seeded from a host snapshot taken at module evaluation. Reads, writes, and deletes work in-process; they do **not** propagate to the host process or to child processes. |
| `args`            | `readonly string[]`        | Program arguments after the runtime binary and the script (or `-e` snippet). **Frozen.** Excludes the executable and script path.                                                          |
| `platform`        | `string`                   | Host OS — the OS-native value (`std::env::consts::OS`): `"linux"`, `"macos"`, `"windows"`, …                                                                                              |
| `arch`            | `string`                   | Host CPU architecture — the OS-native value (`std::env::consts::ARCH`): `"x86_64"`, `"aarch64"`, `"arm"`, …                                                                               |
| `cwd()`           | `() => string`             | Current working directory. A **function** (not a value) because the directory can change during a run.                                                                                    |
| `exit(code = 0)`  | `(code?: number) => never` | Records the exit code and **halts execution immediately** — code after the call does not run. The embedder reads the recorded code and treats it as a clean exit, not an error.            |
| `default`         | `object`                   | An aggregate bundling all named exports. Named imports are preferred for clarity and tree-shaking.                                                                                        |

### Examples

```js
// env — read / write / delete (in-process only)
import { env } from "runtime:process";
console.log(env.HOME);
env.FEATURE_FLAG = "on";
delete env.SECRET;
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
real OS; that is why it carries `Env` (\*importing it evaluates `runtime:process`).

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
| `normalize(p)`          | `(string) => string`          | Collapses `.`/`..` and redundant separators.                                |
| `join(...segments)`     | `(...string) => string`       | Joins segments with the separator, then normalizes.                         |
| `resolve(...segments)`  | `(...string) => string`       | Resolves to an absolute path, anchoring at `cwd()` if no segment is absolute.|
| `dirname(p)`            | `(string) => string`          | The directory portion of `p`.                                               |
| `basename(p)`           | `(string) => string`          | The final segment of `p` (no suffix-stripping overload).                    |
| `extname(p)`            | `(string) => string`          | The extension of the final segment, including the dot (or `""`).            |
| `parse(p)`              | `(string) => object`          | `{ root, dir, base, name, ext }`.                                           |
| `relative(from, to)`    | `(string, string) => string`  | Relative path from `from` to `to` (both resolved first).                    |
| `fromFileURL(url)`      | `(string \| URL) => string`   | Converts a `file:` URL to a path.                                           |
| `toFileURL(p)`          | `(string) => URL`             | Converts a path (resolved to absolute) to a `file:` URL.                    |
| `default`               | `object`                      | An aggregate of all named exports.                                          |

<!-- Reference links -->
[D27]: ./DECISIONS.md
