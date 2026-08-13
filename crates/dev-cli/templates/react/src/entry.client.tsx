/**
 * The browser's entry — named by the `<script type="module">` in index.html,
 * which is what makes it a build input.
 */
import { createRoot, hydrateRoot } from "react-dom/client";
import { App } from "./app/App.tsx";
import { match, type RouteData } from "./app/routes.ts";

const root = document.getElementById("root");
const route = match(location.pathname);
if (!root || !route) {
  throw new Error(`nothing to render at ${location.pathname}`);
}

// What the server already fetched, so the first render matches the markup it
// sent. Without it the client would render a different tree and React would
// throw the server's HTML away.
const data = (globalThis as { __DATA__?: RouteData }).__DATA__ ?? (await route.loader());

// **Hydrate what the server rendered, or render from scratch if it did not.**
// The same bundle serves all three ways of shipping this app: hydration for the
// server-rendered and prerendered pages, a cold render for the single-page
// build, where index.html arrives with an empty root.
if (root.firstChild) {
  hydrateRoot(root, <App route={route} data={data} />);
} else {
  createRoot(root).render(<App route={route} data={data} />);
}
