# @opentf/esrun-types

TypeScript type definitions for [ES Runtime](https://esrun.opentechf.org)
(`esrun`) — every `runtime:` standard module, so your editor gives completion
and type-checking for `import … from "runtime:fs"` and its siblings.

## Install

```sh
bun add -d @opentf/esrun-types   # or: npm i -D @opentf/esrun-types
```

## Use

Add the package to your `tsconfig.json`:

```json
{ "compilerOptions": { "types": ["@opentf/esrun-types"] } }
```

…or reference it from one file:

```ts
/// <reference types="@opentf/esrun-types" />
```

Then the `runtime:` imports are fully typed:

```ts
import { file, write } from "runtime:fs";

const cfg = await file("./config/app.json").json();
await write("./out/result.txt", "done", { append: true });
```

esrun targets the WinterTC web-platform surface, so web globals (`URL`, `Blob`,
`ReadableStream`, `Response`, …) come from your `lib` (`dom` or `webworker`).

## Covered

- `runtime:process` — `env`, `args`, `platform`, `arch`, `cwd()`, `exit()`
- `runtime:path` — `join`, `resolve`, `normalize`, `dirname`, `basename`, `extname`, `parse`, `relative`, `isAbsolute`, `sep`, `delimiter`, `fromFileURL`, `toFileURL`
- `runtime:fs` — `file()`, `write()`, `readDir`, `stat`, `exists`, `mkdir`, `remove`, `rename`, `Glob`
- `runtime:db` — `connect`, `sql`, `queryAst`, `sqlite`, `Connection`, `Rows`, `Pool`, `Driver`, `DbError`, and the driver-authoring surface (`defineDriver`, `runBackendConformance`)
- `runtime:net` — `connect`, `listen`, `bind`, `Socket`, `Listener`, `DatagramSocket`
- `runtime:http` — `serve`, `Handler`, `Server`, `withTrailers`
- `runtime:websocket` — `serve`, `upgradeWebSocket`, `WebSocketConnection`, `broadcast`
- `runtime:serialization` — `XML`, `YAML`, `TOML`, `MessagePack`, `JSONL`, `Protobuf`
- `runtime:hashing` — `hash`, `Hasher`, `hashStream`, `hmac`, `timingSafeEqual`, `password`
- `runtime:system` — `Command`, `ChildProcess`
- `runtime:wasi` — `WASI`

The last three are `esdev`'s, not `esrun`'s — a deployed binary has no bundler,
no watcher and no test runner:

- `runtime:build` — `build`, `Plugin`, `Hook`, `BuildOptions`, `BuildResult`
- `runtime:watch` — `watch`, `Watcher`, `Change`
- `runtime:test` — `test`, `beforeAll`, `afterAll`, `beforeEach`, `afterEach`, `assert`, `assertEquals`, `assertThrows`, `assertRejects`

…plus the few globals whose shape here differs from the standard libs.
