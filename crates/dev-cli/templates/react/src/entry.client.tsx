/**
 * The browser's entry — named by the `<script type="module">` in index.html,
 * which is what makes it a build input.
 *
 * **One bundle serves all three ways of shipping this app.** It hydrates the
 * markup a server rendered, hydrates the markup the prerender step wrote to a
 * file, and renders from nothing when a single-page build hands it an empty
 * root. The difference is one branch, below.
 */
// **First, and it has to stay first.** It installs the hook React reads as
// it initialises, and ES modules evaluate in import order. See src/refresh.ts.
import "./refresh.ts";

import { StrictMode } from "react";
import { createRoot, hydrateRoot } from "react-dom/client";
import { RouterProvider, createBrowserRouter, type HydrationState } from "react-router";

import { routes } from "./routes.tsx";

declare global {
  interface Window {
    /**
     * Written into the document by `<StaticRouterProvider>`. react-router
     * emits it but does not declare it, because nothing in the library reads
     * it — the application does, here.
     */
    __staticRouterHydrationData?: HydrationState;
  }
}

const container = document.getElementById("root");
if (!container) {
  throw new Error("index.html has no #root for the app to render into");
}

// What the server's loaders already produced, serialised into the document by
// `<StaticRouterProvider>`. With it the router hydrates without running a
// single loader again; without it — a single-page build — the router runs them
// itself on the first render.
const router = createBrowserRouter(routes, {
  hydrationData: window.__staticRouterHydrationData,
});

const app = (
  <StrictMode>
    <RouterProvider router={router} />
  </StrictMode>
);

// A root with something in it was rendered by the server or written by the
// prerender step, and React should adopt that markup rather than replace it.
// An empty one is the single-page build, where there is nothing to adopt.
if (container.firstChild) {
  hydrateRoot(container, app);
} else {
  createRoot(container).render(app);
}
