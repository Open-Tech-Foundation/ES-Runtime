import * as runtime from "react-refresh/runtime";

// This module is inserted only by a hot browser transform. Module caching
// ensures the hook is installed once, before the transformed module's imports.
runtime.injectIntoGlobalHook(globalThis);
globalThis.$RefreshReg$ = () => {};
globalThis.$RefreshSig$ = runtime.createSignatureFunctionForTransform;
