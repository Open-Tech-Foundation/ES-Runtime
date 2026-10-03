// Client entry for `esdev start` / `esdev build`.
// The route map comes from the `@otfw/routes` virtual module; components
// compile on load through the project's esdev plugin.
import { mountApp } from "@opentf/web";
import { guard, pages, loaderRoutes } from "@otfw/routes";

mountApp({ pages, guard, loaders: loaderRoutes, target: document.getElementById("app") });
