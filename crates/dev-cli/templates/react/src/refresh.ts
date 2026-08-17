/**
 * React Fast Refresh, booted before React is.
 *
 * # Why this file exists at all
 *
 * `esdev start --hot` can replace a changed module in the running page. That is
 * enough for a plain module and not enough for a component: re-running a module
 * makes new function identities, and React treats a new identity as a different
 * component — so it unmounts the old tree and every `useState` in it goes with
 * it. The edit lands and the form you were filling in is empty.
 *
 * Fast Refresh is React's answer. esdev applies the transform and wraps each
 * component module (`"refresh": "react"` in `esdev.json`); this is the other
 * half, and it is here rather than in esdev because it is the half that knows
 * what React is.
 *
 * # Why it is imported first
 *
 * `injectIntoGlobalHook` has to run **before React loads**, because what it
 * installs is the hook React reads as it initialises. ES modules evaluate in
 * import order, so `entry.client.tsx` imports this one above everything else,
 * and that import must stay where it is.
 *
 * # It is not in your production bundle
 *
 * The whole file is behind a `NODE_ENV` check that the build replaces with a
 * literal, so a release build drops it, its import of `react-refresh` with it.
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
