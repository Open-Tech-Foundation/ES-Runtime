// Runs after `esdev build` (`"then": "run"` in esdev.json): renders the app
// into dist/index.html, so the page has content before any script loads.
import { write } from "runtime:fs";
import { join } from "runtime:path";
import { here, renderPage } from "./render.tsx";

await write(join(here, "index.html"), await renderPage());
console.log("prerendered dist/index.html");
