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
- [`runtime:fs`](#runtimefs)
- [`runtime:net`](#runtimenet)
- [`runtime:http`](#runtimehttp)
- [`runtime:websocket`](#runtimewebsocket)
- [`runtime:serialization`](#runtimeserialization)

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
- **Events:** `Event`, `EventTarget`, `CustomEvent`, `MessageEvent`, `CloseEvent`, `AbortController`, `AbortSignal`
- **Network:** `WebSocket` (capability-gated — see below)
- **Data:** `Blob`, `File`, `FormData`, `DOMException`
- **Performance:** `performance` (`now()`, `timeOrigin`)

**Not available:** `process`/`Buffer`/`require` (Node), `Worker`/`MessageChannel`,
`navigator`/`localStorage`/`window` (browser).

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

**Not yet:** the promise/stream-based `WebSocketStream`, and permessage-deflate
(`extensions` is always `""`). See DECISIONS D29.

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
| `runtime:fs`      | Available   | `FileRead` / `FileWrite` | [↓](#runtimefs) |
| `runtime:net`     | Available   | `Net` / `NetListen` | [↓](#runtimenet)     |
| `runtime:http`    | Available   | `NetListen` | [↓](#runtimehttp)               |
| `runtime:websocket` | Available | `NetListen` | [↓](#runtimewebsocket)         |
| `runtime:serialization` | Available   | None       | [↓](#runtimeserialization)           |

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
| `Net`       | Open outbound network connections (`fetch`, `runtime:net` `connect`). |
| `NetListen` | Bind a listening socket and accept inbound connections (`runtime:net` `listen`, `runtime:http` `serve`). |
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
| `env`             | `Record<string, string \| Secret>`  | Environment variables as a **mutable in-process object**, seeded from a host snapshot taken at module evaluation (plus any `--env-file` values). Reads, writes, and deletes work in-process; they do **not** propagate to the host process or to child processes. Secret-keyed values are `Secret` wrappers (see below). |
| `args`            | `readonly string[]`                 | Program arguments after the runtime binary and the script (or `-e` snippet). **Frozen.** Excludes the executable and script path.                                                          |
| `platform`        | `string`                            | Host OS — the OS-native value (`std::env::consts::OS`): `"linux"`, `"macos"`, `"windows"`, …                                                                                              |
| `arch`            | `string`                            | Host CPU architecture — the OS-native value (`std::env::consts::ARCH`): `"x86_64"`, `"aarch64"`, `"arm"`, …                                                                               |
| `cwd()`           | `() => string`                      | Current working directory. A **function** (not a value) because the directory can change during a run.                                                                                    |
| `exit(code = 0)`  | `(code?: number) => never`          | Records the exit code and **halts execution immediately** — code after the call does not run. The embedder reads the recorded code and treats it as a clean exit, not an error.            |
| `unmask(value)`   | `(value: string \| Secret) => string` | Reveal a masked `Secret`'s real value. A plain `string` passes through unchanged, so `unmask(env.ANY)` is always safe.                                                                  |
| `Secret`          | `class`                             | Opaque holder for a masked env value (see **Secret masking**).                                                                                                                            |
| `default`         | `object`                            | An aggregate bundling all named exports. Named imports are preferred for clarity and tree-shaking.                                                                                        |

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

### `FsFile` (from `file(path)`)

`text()`, `json()`, `bytes()` (`Uint8Array`), `arrayBuffer()`, `stream()`
(`ReadableStream`), `exists()`, `stat()`, `write(data)`, `delete()`, and the
`path` it points at — the Blob read surface plus convenience writes/deletes.

---

## `runtime:net`

TCP sockets (SPEC §12). `connect()` follows the **WinterTC Sockets API**:
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
| `listen(options)`            | `({ hostname?, port, secureTransport?, cert?, key?, alpn? }) => Listener` | Bind a listening socket. `secureTransport: "on"` terminates TLS on each accept — requires a PEM `cert` + `key`; `alpn` advertises protocols. `NetListen`. |

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
`TypeError` whose message is prefixed `"SocketError: "` (WinterTC `SocketError`).

## `runtime:http`

An HTTP/1.1 server: `serve((request) => response)`. The handler receives a web
`Request` and returns (or resolves to) a web `Response` — the same Fetch API
objects `fetch` uses. A thrown error or a non-`Response` return becomes a `500`.
`serve` requires `NetListen` (it binds a listening socket). All I/O is async.
Request and response bodies are buffered (streaming bodies are a follow-up); TLS
is not supported yet (terminate it at a proxy).

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
| `serve(options, handler)`         | `({ hostname?, port? }, Handler) => Server`   | Start a server bound to `options`. `NetListen`.                    |

`Handler` is `(request: Request) => Response | Promise<Response>`.

**`Server`** — `addr: Promise<{ hostname, port }>` (resolves once listening),
`finished: Promise<void>` (resolves after `stop()`), `stop(): Promise<void>`.

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
| `serve(options)`  | `({ hostname?, port }) => WebSocketServer` | Bind a WebSocket server; `port` 0 picks an ephemeral port. `NetListen`. |
| `broadcast(connections, data)` | `(Iterable<conn>, string \| BufferSource \| Blob) => void` | Send one message to many connections in a single host crossing (the batched form of a `.send()` loop). |

**`WebSocketServer`** — async-iterable of server connections;
`addr: Promise<{ hostname, port }>`, `accept(): Promise<conn | null>`,
`close(): Promise<void>`.

**connection** (each accepted socket) — already open: `send(data)`
(`string`/`Blob`/`ArrayBuffer`/`ArrayBufferView`), `close(code?, reason?)`,
`binaryType`, and `message`/`close` events (`on*` or `addEventListener`) — the
same surface as the client `WebSocket`, minus the connecting handshake.

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
const pbBytes = schema.build("Hello", { name: "world" });
const pbObj = schema.parse("Hello", pbBytes); // { name: "world" }
```

### Exports

For each string format (XML, YAML, TOML), the module provides a namespace with three methods:

| Export | Description |
| --- | --- |
| `<Format>.parse(data)` | Parses the given format into a JavaScript object. |
| `<Format>.build(obj)` | Serializes a JavaScript object into the given format. |
| `<Format>.validate(data, opts?)` | Validates the given data without full allocation. `opts.detailed` provides `{ valid: boolean, error: string }`. |

For binary formats like MessagePack, the namespace is slightly different:

| Export | Description |
| --- | --- |
| `MessagePack.decode(bytes)` | Parses a MessagePack byte array into a JavaScript object. |
| `MessagePack.encode(obj)` | Serializes a JavaScript object into a MessagePack `Uint8Array`. |
| `MessagePack.validate(bytes, opts?)` | Validates the given byte array. |

For JSONL, it provides transform streams under the `JSONL` namespace:

| Export | Description |
| --- | --- |
| `new JSONL.DecoderStream()` | A `TransformStream` that parses lines of JSON. |
| `new JSONL.EncoderStream()` | A `TransformStream` that stringifies objects to JSON lines. |

For XML, it also provides a `DecoderStream`:

| Export | Description |
| --- | --- |
| `new XML.DecoderStream()` | A `TransformStream` that parses XML chunks. |

For Protobuf, schemas are compiled from `.proto` source at runtime (pure JS, reflective — proto3 and edition 2023; proto2-only constructs are rejected):

| Export | Description |
| --- | --- |
| `new Protobuf.Schema(proto, opts?)` | Compiles a `.proto` source string (or a `{ filename: source }` map for multi-file schemas with `import`s; the `google/protobuf/*` well-known types resolve automatically). |
| `schema.parse(messageName, bytes)` | Decodes a `Uint8Array` for the fully-qualified `messageName`. |
| `schema.build(messageName, value)` | Encodes a JavaScript object into a `Uint8Array`. |

Decoded value shape: camelCase field names; 64-bit integer fields (`int64`/`uint64`/`sint64`/`fixed64`/`sfixed64`) as **BigInt**; enums as their value-name string (unknown numbers kept as numbers); `bytes` as `Uint8Array`; maps as plain objects; nested messages as plain objects. Fields absent on the wire are omitted.

<!-- Reference links -->
[D27]: ./DECISIONS.md

## Error Diagnostics

When exceptions are thrown by ES-Runtime during module evaluation or unhandled promise rejections, the original `Error` subclasses and their stack traces are preserved. The CLI automatically extracts these diagnostics and prints them elegantly with ANSI colors. The stack trace will highlight exact lines and columns of errors: `TypeError: message \n    at fn (file:line:col)`.
