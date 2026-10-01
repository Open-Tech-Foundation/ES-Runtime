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

// An `html` hook adds tags, or replaces the document, or both (D151).
export const preload: Plugin = {
  name: "preload",
  html: {
    filter: { id: /index\.html$/ },
    handler(html, id, ctx) {
      const imports = (ctx.bundle ?? []).flatMap((file) =>
        file.type === "chunk" && file.isEntry ? file.imports : [],
      );
      return {
        html: html.replace("<title>", `<title data-id="${id}">`),
        tags: imports.map((file) => ({
          tag: "link",
          attrs: { rel: "modulepreload", href: `/${file}`, crossorigin: true },
          injectTo: "head-prepend" as const,
        })),
      };
    },
  },
};

export const refused: Plugin = {
  html: {
    // @ts-expect-error — one form of answer: `{ html }`, not a bare string.
    handler: (html) => html,
  },
};

// `finish` sees every target of a release build, keyed by name (D153).
export const sitemap: Plugin = {
  name: "sitemap",
  finish: {
    order: "post",
    async handler(targets, ctx) {
      const web = targets.web;
      const entries = web.files.filter((file) => file.type === "chunk" && file.isEntry);
      void ctx.command;
      void `${web.outDir}/${entries.length}`;
    },
  },
};

export const filteredFinish: Plugin = {
  finish: {
    // @ts-expect-error — `finish` runs once for the whole build: no filter.
    filter: { id: /x/ },
    handler: () => {},
  },
};
