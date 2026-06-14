# @opentf/esrun-types

TypeScript type definitions for [ES Runtime](https://es-runtime.opentechf.org)
(`esrun`) — the `runtime:` standard modules, so your editor gives completion and
type-checking for `import … from "runtime:process" | "runtime:path" | "runtime:fs"`.

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
