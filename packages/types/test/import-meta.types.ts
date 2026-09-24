// A type test for `import.meta.resolve`'s `parent`. Compiled by `tsc -p .`,
// never run.

const root = new URL("./", import.meta.url);

const fromHere: string = import.meta.resolve("@opentf/web");
const fromRoot: string = import.meta.resolve("@opentf/web", root);
const fromString: string = import.meta.resolve("@opentf/web", root.href);

// @ts-expect-error — a parent is a URL, not a path.
import.meta.resolve("@opentf/web", 42);

export { fromHere, fromRoot, fromString };
