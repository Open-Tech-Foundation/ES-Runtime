// Provided by esdev's build, not by a package.
declare const process: { readonly env: { readonly NODE_ENV: "development" | "production" } };

declare module "react-refresh/runtime" {
  export function injectIntoGlobalHook(global: unknown): void;
  export function createSignatureFunctionForTransform(): (type: unknown) => unknown;
  export function register(type: unknown, id: string): void;
  export function performReactRefresh(): void;
}
