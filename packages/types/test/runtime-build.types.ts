// A type test for `runtime:build`'s `resolve`. Compiled by `tsc -p .`, never
// run.

import { resolve } from "runtime:build";

const root = new URL("./", import.meta.url);
const fromUrl: string = resolve("@opentf/web", root);
const fromString: string = resolve("@opentf/web", root.href);

// @ts-expect-error — `from` is required: without it there is nothing to resolve from.
resolve("@opentf/web");

export { fromString, fromUrl };
