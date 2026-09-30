/** React Fast Refresh, enabled only in a hot browser build. */
export default {
  name: "react-refresh",
  jsx: { refresh: true },
  transform: {
    filter: { id: /\.[jt]sx$/ },
    handler(code, id, ctx) {
      if (!ctx.hot || ctx.platform !== "browser") return null;

      const prefix =
        'import "@opentf/esdev-plugin-react/runtime";\n' +
        'import * as __refresh from "react-refresh/runtime";\n' +
        `globalThis.$RefreshReg$ = (type, name) => __refresh.register(type, ${JSON.stringify(id)} + " " + name);\n` +
        "globalThis.$RefreshSig$ = __refresh.createSignatureFunctionForTransform;\n" +
        "import.meta.hot.accept(() => __refresh.performReactRefresh());\n";
      const mappings =
        ";".repeat(prefix.split("\n").length - 1) +
        code
          .split("\n")
          .map((_, index) => (index === 0 ? "AAAA" : "AACA"))
          .join(";");
      return {
        code: `${prefix}${code}\n`,
        map: JSON.stringify({
          version: 3,
          sources: [id],
          sourcesContent: [code],
          names: [],
          mappings,
        }),
      };
    },
  },
};
