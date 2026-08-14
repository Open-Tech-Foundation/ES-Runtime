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

  /** What `load` and `transform` may return. */
  export type ModuleOutput =
    | string
    | null
    | undefined
    | {
        code: string;
        /** A source map, as an object or as JSON. */
        map?: unknown;
        moduleType?: "js" | "jsx" | "ts" | "tsx" | "json" | "css" | "text" | (string & {});
      };

  /**
   * A plugin. The hooks are rollup's, and take rollup's arguments; a plugin
   * written for rollup or rolldown works here unchanged, minus any hook this
   * list does not carry.
   */
  export interface Plugin {
    name?: string;
    buildStart?(this: PluginContext): void | Promise<void>;
    resolveId?(
      this: PluginContext,
      source: string,
      importer: string | null,
      options: { isEntry: boolean },
    ):
      | string
      | null
      | undefined
      | { id: string; external?: boolean | "absolute" | "relative" }
      | Promise<string | null | undefined | { id: string; external?: boolean | "absolute" | "relative" }>;
    load?(this: PluginContext, id: string): ModuleOutput | Promise<ModuleOutput>;
    transform?(
      this: PluginContext,
      code: string,
      id: string,
    ): ModuleOutput | Promise<ModuleOutput>;
    buildEnd?(this: PluginContext, error: string | null): void | Promise<void>;
  }

  export interface ResolveOptions {
    /** `find` → what to try instead, in order. */
    alias?: Record<string, string | string[]>;
    /** Extensions tried for an extensionless specifier. */
    extensions?: string[];
    /** `exports` conditions, in the order they are matched. */
    conditionNames?: string[];
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
    /** Which environment the output runs in. Decides `exports` conditions. */
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
