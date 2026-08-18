/**
 * The package's public surface.
 *
 * Everything a consumer can reach is re-exported from here, and `package.json`
 * points `exports` at this file alone. That is the boundary: a module not named
 * here is internal, and can be renamed or deleted without a major version,
 * because nothing could have imported it.
 */
export { greet } from "./greeting.ts";
