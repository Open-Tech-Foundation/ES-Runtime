declare module "runtime:build" {
  /** A hook's `this`: the bundler's own context, for as long as the hook runs. */
  export interface PluginContext {
    /**
     * Asks the **bundler's own resolver** where a specifier points, in the
     * middle of a hook. `null` if nothing resolves.
     */
    resolve(
      source: string,
      importer?: string,
      options?: { skipSelf?: boolean },
    ): Promise<{ id: string; external: boolean } | null>;
    /**
     * Declares a file the module being processed depends on but never imports
     * — frontmatter, a `_meta.js` a virtual module was generated from. This is
     * what puts it in `watchFiles`, and therefore what makes invalidation
     * correct for generated modules.
     */
    addWatchFile(file: string): void;
    /** Adds a chunk or an asset to a build that is already running. */
    emitFile(file: EmittedFile): string;
    /** A diagnostic, surfaced in the build's `warnings`. */
    warn(log: string | { message: string }): void;
    info(log: string | { message: string }): void;
    debug(log: string | { message: string }): void;
    /** Fails the build with this message. Throws — it does not return. */
    error(log: string | Error | { message: string }): never;
  }

  export type EmittedFile =
    | { type: "asset"; name?: string; fileName?: string; source: string | Uint8Array }
    | { type: "chunk"; id: string; name?: string; fileName?: string };

  /** What `load` and `transform` may return. `null` means "not mine". */
  export type ModuleResult =
    | null
    | undefined
    | {
        code: string;
        /** How the code should be treated. Omit to keep the backend's guess. */
        type?: "js" | "jsx" | "ts" | "tsx" | "json" | "css" | "text" | (string & {});
        /** A source map, as an object or as JSON. */
        map?: unknown;
        /**
         * Files this module depends on that the graph cannot discover — the
         * frontmatter a generated module was built from. **Returned**, not
         * declared by a call you can forget: forgetting it produces a build
         * that serves stale output. Relative paths resolve like any other path
         * in a run.
         */
        dependsOn?: string[];
      };

  /** What `resolve` may return. `null` means "not mine". */
  export type ResolveResult =
    | null
    | undefined
    | {
        id: string;
        external?: boolean | "absolute" | "relative";
        /**
         * There is no file behind this id — the plugin's `load` will provide
         * it. Replaces rollup's convention of prefixing the id with a NUL byte.
         */
        virtual?: boolean;
      };

  /**
   * A hook's context — the bundler's own, live only while that hook runs.
   *
   * The **last argument** of every handler, not `this`: an arrow-function
   * handler keeps it, where rollup's context-as-`this` is silently lost.
   */
  export interface PluginContext {
    /** Asks the bundler's resolver, mid-hook. `null` if nothing resolves. */
    resolve(
      source: string,
      importer?: string,
      options?: { skipSelf?: boolean },
    ): Promise<{ id: string; external: boolean } | null>;
    /** Adds a chunk or an asset to a build that is already running. */
    emit(file: EmittedFile): string;
    /** A diagnostic, surfaced in the build's `warnings`. */
    warn(log: string | { message: string }): void;
    info(log: string | { message: string }): void;
    debug(log: string | { message: string }): void;
    /** Fails the build with this message. Throws — it does not return. */
    error(log: string | Error | { message: string }): never;
    /** On `resolve`: whether the specifier being resolved is an entry. */
    readonly isEntry?: boolean;
  }

  export type EmittedFile =
    | { type: "asset"; name?: string; fileName?: string; source: string | Uint8Array }
    | { type: "chunk"; id: string; name?: string; fileName?: string };

  /** One pattern: a string is an **exact** match, a RegExp is tested. */
  export type FilterPattern = string | RegExp | (string | RegExp)[];

  /**
   * Which modules a hook wants. Matched **on the host's side, before the call
   * crosses into this isolate** — which is why it is declarative rather than a
   * predicate you write. An unfiltered `transform` is one crossing per module
   * in the graph.
   */
  export interface HookFilter {
    /** Matched against the module id — or, for `resolve`, the specifier. */
    id?: FilterPattern;
    /** Matched against the module's source. `transform` only. */
    code?: FilterPattern;
  }

  /** Where a hook runs relative to the plugins that did not say. */
  export type HookOrder = "pre" | "post";

  /**
   * One hook. **This is the only form** — a bare function is rollup's
   * shorthand and is refused, because accepting it would make the filter, the
   * order and the context argument optional extras on somebody else's design.
   */
  export interface Hook<H> {
    handler: H;
    filter?: HookFilter;
    order?: HookOrder;
  }

  /** A hook that runs once for the whole build, so it cannot be filtered. */
  export interface WholeBuildHook<H> {
    handler: H;
    order?: HookOrder;
  }

  /**
   * A plugin: a name, and hooks.
   *
   * The system is **ours**, not the bundler's passed through — a `runtime:`
   * module is a versioned contract, and an API defined by a third party's trait
   * moves when that trait moves.
   *
   * A plugin is guest code: it runs in this isolate under the same capability
   * model as the rest of the program, so a plugin that reads a file needs
   * `FileRead`.
   */
  export interface Plugin {
    name?: string;
    start?: WholeBuildHook<(ctx: PluginContext) => void | { dependsOn?: string[] } | Promise<void | { dependsOn?: string[] }>>;
    resolve?: Hook<
      (source: string, importer: string | null, ctx: PluginContext) =>
        | ResolveResult
        | Promise<ResolveResult>
    >;
    load?: Hook<(id: string, ctx: PluginContext) => ModuleResult | Promise<ModuleResult>>;
    transform?: Hook<
      (code: string, id: string, ctx: PluginContext) => ModuleResult | Promise<ModuleResult>
    >;
    end?: WholeBuildHook<(error: string | null, ctx: PluginContext) => void | Promise<void>>;
  }

  export interface ResolveOptions {
    /** `find` → what to try instead, in order. */
    alias?: Record<string, string | string[]>;
    /** Extensions tried for an extensionless specifier. */
    extensions?: string[];
    /**
     * Extra `exports` conditions, **appended** to the ones the platform
     * already asserts — `worker` for `neutral`, `browser` for `browser` — so
     * naming one of your own cannot silently drop the condition that decides
     * whether a package hands over its Web build or its `node:` one.
     */
    conditionNames?: string[];
    /**
     * `package.json` fields to fall back on when a package has no `exports`
     * map. Naming them **replaces** the default `["module", "main"]`: there is
     * one ordered list and a caller who writes one means it.
     */
    mainFields?: string[];
  }

  export interface OutputOptions {
    format?: "esm" | "cjs" | "iife" | "umd";
    /** Where `write()` puts things. Ignored by `generate()`. */
    dir?: string;
    file?: string;
    entryFileNames?: string;
    chunkFileNames?: string;
    assetFileNames?: string;
    /**
     * `false` puts everything reachable in one chunk, dynamic `import()`
     * included — what a server holding one route's bundle in memory wants.
     */
    codeSplitting?: boolean;
    sourcemap?: boolean | "inline" | "external" | "hidden";
    banner?: string;
    footer?: string;
  }

  export interface BuildOptions extends OutputOptions {
    /** The entry, or entries: one path, a list, or `{ name: path }`. */
    input: string | string[] | Record<string, string> | { name?: string; import: string }[];
    /**
     * What to leave unbundled. A **predicate** as well as a list, because a
     * dev server externalises a shape (`/__route/*`) rather than a set.
     */
    external?: string | string[] | ((id: string, importer: string | null, resolved: boolean) => boolean);
    /**
     * Which environment the output runs in. Decides `exports` conditions:
     * `neutral` (the default, and what this runtime is) asserts `worker`,
     * `browser` asserts `browser`, and `node` leaves resolution to the
     * bundler's own knowledge of Node. The same defaults `esdev build` uses.
     */
    platform?: "neutral" | "browser" | "node";
    resolve?: ResolveOptions;
    /** Compile-time replacements: `{ "process.env.NODE_ENV": '"development"' }`. */
    define?: Record<string, string>;
    plugins?: (Plugin | null | undefined)[];
    minify?: boolean;
    treeshake?: boolean;
    /** Where the build runs. Defaults to the entry module's directory. */
    cwd?: string;
  }

  export interface OutputChunk {
    type: "chunk";
    fileName: string;
    name: string;
    code: string;
    isEntry: boolean;
    isDynamicEntry: boolean;
    /** Every module that went into this chunk. */
    moduleIds: string[];
    imports: string[];
    dynamicImports: string[];
    /** The source map as JSON, when one was asked for. */
    map: string | null;
  }

  export interface OutputAsset {
    type: "asset";
    fileName: string;
    source: string | Uint8Array;
  }

  export interface BuildResult {
    output: (OutputChunk | OutputAsset)[];
    /**
     * Every file the build read, plus every file a plugin declared with
     * `this.addWatchFile()`. Pair it with `runtime:watch` to drop exactly the
     * cached chunks a change invalidates.
     */
    watchFiles: string[];
    warnings: string[];
  }

  export interface Bundle {
    /** Builds, and returns the chunks and assets **in memory**. */
    generate(output?: OutputOptions): Promise<BuildResult>;
    /** The same build, written under `dir`. Needs `FileWrite` as well. */
    write(output?: OutputOptions): Promise<BuildResult>;
    /** Releases the build. */
    close(): Promise<void>;
    /** What the last `generate()`/`write()` read. Empty before the first. */
    readonly watchFiles: string[];
    readonly warnings: string[];
  }

  /**
   * Starts a build.
   *
   * **`esdev` only.** A production binary that could bundle would have to carry
   * a bundler, and a deployment has nothing to bundle — so `esrun` does not
   * serve this module, and importing it there fails at load.
   *
   * Needs `FileRead`; `write()` needs `FileWrite` as well.
   *
   * ```ts
   * const bundle = await build({ input: "app/main.jsx", plugins: [mdx()] });
   * const { output, watchFiles } = await bundle.generate({ codeSplitting: false });
   * serve(output[0].code);
   * ```
   *
   * The bundler runs on a thread of its own; plugin hooks run here, in this
   * isolate, several at a time.
   */
  export function build(options: BuildOptions): Promise<Bundle>;

  const _default: { build: typeof build };
  export default _default;
}
