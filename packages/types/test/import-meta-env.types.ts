// A type test for `import.meta.env`, as esdev compiles it into a bundle.
//
// `@ts-expect-error` fails the build when the error it names stops happening,
// so a declaration that widened to `any` breaks this file rather than passing.

const mode: "development" | "production" | "test" = import.meta.env.MODE;
// @ts-expect-error — `"test"` is a mode too
const built: "development" | "production" = import.meta.env.MODE;
const dev: boolean = import.meta.env.DEV;
const prod: boolean = import.meta.env.PROD;

// A public variable may be missing from the build.
const api: string | undefined = import.meta.env.PUBLIC_API_URL;
// @ts-expect-error — not known to be set
const sure: string = import.meta.env.PUBLIC_API_URL;

// Only `PUBLIC_*` is compiled in; anything else is not there to read.
// @ts-expect-error — not a public name
import.meta.env.DATABASE_URL;

// @ts-expect-error — compiled in, not assignable
import.meta.env.MODE = "production";

// Destructuring works, as the build replaces the object whole.
const { PUBLIC_REGION } = import.meta.env;
const region: string | undefined = PUBLIC_REGION;

export { api, built, dev, mode, prod, region, sure };
