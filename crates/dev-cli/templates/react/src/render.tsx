import { file } from "runtime:fs";
import { dirname, fromFileURL, join } from "runtime:path";
import { renderToString } from "react-dom/server.browser";
import { App } from "./App.tsx";

/** `dist`: where the running bundle and the built index.html are. */
export const here = dirname(fromFileURL(import.meta.url));

/** The built index.html, with the app rendered into it. */
export async function renderPage(): Promise<string> {
  const template = await file(join(here, "index.html")).text();
  return template.replace("<!--app-->", renderToString(<App />));
}
