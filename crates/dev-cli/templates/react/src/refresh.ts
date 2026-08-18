/**
 * React Fast Refresh, booted before React is.
 *
 * Re-running a module makes new function identities, and React treats a new
 * identity as a different component — so without this an edit unmounts the tree
 * and every `useState` in it. esdev applies the transform (`"refresh": "react"`
 * in esdev.json); this is the half that knows what React is.
 *
 * **The import of this file must stay first in entry.client.tsx**:
 * `injectIntoGlobalHook` installs the hook React reads as it initialises, and
 * modules evaluate in import order. The whole file is behind a `NODE_ENV`
 * check the build replaces with a literal, so a release build drops it.
 */
// `export {}` makes this a module, which is what allows the top-level `await`
// below — and it is a module in every sense that matters already, since the
// import above it is what the whole file exists to perform.
export {};

if (process.env.NODE_ENV !== "production") {
  const runtime = await import("react-refresh/runtime");

  runtime.injectIntoGlobalHook(window);

  // The two globals the transform's output calls.
  //
  // `$RefreshSig$` is **the real one**, not a stub, and that is not a detail:
  // the transform hoists `var _s = $RefreshSig$()` to the very top of a module,
  // above the wrapper esdev puts around it. So this is the value that call sees,
  // and a stub there means no hook signature is ever recorded — which React
  // reads as "this component's hooks may have changed", and it remounts instead
  // of refreshing. The symptom is a component that loses its state on every
  // edit, which looks exactly like Fast Refresh not being installed at all.
  //
  // `$RefreshReg$` can stay inert here, because registration happens inside the
  // module body, which is after the wrapper has pointed it at that module.
  (window as unknown as Record<string, unknown>).$RefreshReg$ = () => {};
  (window as unknown as Record<string, unknown>).$RefreshSig$ =
    runtime.createSignatureFunctionForTransform;
}
