/**
 * React Fast Refresh, as an esdev plugin.
 *
 * # Why this is here and not in esdev
 *
 * Because esdev has no idea what React is, and should not. What it provides is
 * the **generic** half of hot reloading — `import.meta.hot`, the update channel,
 * and the compiler's component registrations on request — and a framework's
 * scheme is a plugin on top of that. This is that plugin, and it lives in the
 * template because this is where React was chosen.
 *
 * # The two halves
 *
 * 1. **The compiler's.** `jsx: { refresh: true }` below asks the build for a
 *    `$RefreshReg$` call per component and a `$RefreshSig$` signature per
 *    hook-using function. That one cannot be done here: finding the components
 *    needs the syntax tree the compiler already has. esdev honours it only in a
 *    hot dev build of a target that named a `refresh` scheme, because the calls
 *    it inserts reach globals that only a hot loop installs.
 *
 * 2. **The per-module wrapper**, which is this file's `transform`. The
 *    registrations are a *global* call, and it has to mean "register under this
 *    module's id" while this module is evaluating.
 *
 * The runtime bootstrap that has to run before React itself loads is
 * `src/refresh.ts`, imported first by `src/entry.client.tsx`.
 *
 * # There is no epilogue, and that took some finding
 *
 * The obvious shape is a prologue that points `$RefreshReg$` at this module and
 * an epilogue that puts back what was there. It does not work, because the
 * compiler appends its registrations *after* everything this adds:
 *
 *     globalThis.$RefreshReg$ = …          ← the prologue
 *     function Home() { … }                ← the body
 *     globalThis.$RefreshReg$ = __prev     ← the epilogue, restoring
 *     $RefreshReg$(_c, "Home");            ← the compiler's registration
 *
 * The registration lands after the restore, so every component registers into
 * whatever the global was before — nothing, in practice. React then has no
 * component families to match, and an edit re-runs the module, keeps the state
 * and renders the old component: Fast Refresh appears to do nothing at all,
 * with no error anywhere.
 *
 * So the assignment is left standing. Modules in a bundle evaluate in sequence
 * and each one's prologue sets the globals again before its own body, so a
 * module's registrations always run while the globals still point at it.
 *
 * # The refresh is an accept callback
 *
 * For the same ordering reason `performReactRefresh()` cannot be a statement at
 * the end of the module — it would run before the registrations it depends on.
 * It is the `accept` callback instead, which the hot runtime calls *after* the
 * module has been re-run in full, which is the moment it is actually correct.
 * That makes Fast Refresh an ordinary consumer of the generic hot API.
 */

/** The scheme this plugin implements, as `esdev.json`'s `refresh` names it. */
const SCHEME = "react";

export default {
  name: "react-refresh",

  // The half a plugin cannot do for itself. Honoured only in a hot dev build of
  // a target that named a refresh scheme.
  jsx: { refresh: true },

  transform: {
    // JSX only. A `.ts` with no component in it would gain a prologue, an
    // import of the refresh runtime and an `accept()` it never needed — and
    // `accept()` is not harmless: it makes the module a hot boundary that
    // silently swallows changes it cannot actually apply.
    filter: { id: /\.[jt]sx$/ },

    handler(code, id, ctx) {
      // `ctx.refresh` is the scheme the target named, and it is there only
      // while the dev loop is running that target hot. A release build gets
      // nothing from this plugin — the wrapper is exactly wrong in what you
      // ship, since it makes every component module a hot boundary.
      if (ctx.refresh !== SCHEME) return null;

      return {
        code:
          `import * as __refresh from "react-refresh/runtime";\n` +
          `globalThis.$RefreshReg$ = (type, name) => ` +
          `__refresh.register(type, ${JSON.stringify(id)} + " " + name);\n` +
          `globalThis.$RefreshSig$ = __refresh.createSignatureFunctionForTransform;\n` +
          `import.meta.hot.accept(() => __refresh.performReactRefresh());\n` +
          `${code}\n`,
        // The body is still whatever it was — JSX, TypeScript, both — and the
        // prologue is plain JavaScript that any of those parse. Saying nothing
        // leaves the extension to decide, which is right.
        //
        // No source map: it would have to be composed with the one the
        // compiler's own transform makes, and the prologue is a fixed number of
        // lines at the top rather than an edit through the body — so what a
        // stack trace loses is an offset, not a file.
      };
    },
  },
};
