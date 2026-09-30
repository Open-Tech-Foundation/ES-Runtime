// A type test for `runtime:build`'s `resolve`. Compiled by `tsc -p .`, never
// run.

import { resolve } from "runtime:build";

const root = new URL("./", import.meta.url);
const fromUrl: string = resolve("@opentf/web", root);
const fromString: string = resolve("@opentf/web", root.href);

// @ts-expect-error — `from` is required: without it there is nothing to resolve from.
resolve("@opentf/web");

export { fromString, fromUrl };

// A hook's context is the contract esdev hands a plugin (D148): where it runs,
// as facts, and nothing a previous version had.
import type { Plugin, PluginContext } from "runtime:build";

export const framework: Plugin = {
  name: "framework",
  transform: {
    filter: { id: /\.jsx$/ },
    handler(code, id, ctx) {
      const command: "build" | "start" | "test" | "run" | undefined = ctx.command;
      const platform: "browser" | "server" | "node" | undefined = ctx.platform;
      const target: string | undefined = ctx.target;
      const hot: boolean = ctx.hot;
      const mode = platform === "server" ? "ssr" : hot ? "hot" : "csr";
      return { code: `${code}\n// ${id} ${command} ${target} ${mode}`, type: "js" };
    },
  },
};

declare const ctx: PluginContext;
// @ts-expect-error — `refresh` was replaced by `hot`.
ctx.refresh;
// @ts-expect-error — a dependency is returned in `dependsOn`, not declared.
ctx.addWatchFile("a");
// @ts-expect-error — the method is `emit`.
ctx.emitFile({ type: "asset", source: "" });
ctx.emit({ type: "asset", source: "" });
