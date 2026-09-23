/**
 * React Fast Refresh for `esdev start`. esdev provides hot reloading in
 * general; this plugin adds React's scheme on top of it.
 */

/** The scheme this plugin implements, as `esdev.json`'s `refresh` names it. */
const SCHEME = "react";

export default {
  name: "react-refresh",
  jsx: { refresh: true },

  transform: {
    filter: { id: /\.[jt]sx$/ },

    handler(code, id, ctx) {
      if (ctx.refresh !== SCHEME) return null;

      return {
        code:
          `import * as __refresh from "react-refresh/runtime";\n` +
          `globalThis.$RefreshReg$ = (type, name) => ` +
          `__refresh.register(type, ${JSON.stringify(id)} + " " + name);\n` +
          `globalThis.$RefreshSig$ = __refresh.createSignatureFunctionForTransform;\n` +
          `import.meta.hot.accept(() => __refresh.performReactRefresh());\n` +
          `${code}\n`,
      };
    },
  },
};
