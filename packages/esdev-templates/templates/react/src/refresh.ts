// React Fast Refresh during `esdev start`: edits keep component state.
// Release builds drop it, since NODE_ENV is replaced with "production".
export {};

if (process.env.NODE_ENV !== "production") {
  const runtime = await import("react-refresh/runtime");
  runtime.injectIntoGlobalHook(window);
  const globals = window as unknown as Record<string, unknown>;
  globals.$RefreshReg$ = () => {};
  globals.$RefreshSig$ = runtime.createSignatureFunctionForTransform;
}
