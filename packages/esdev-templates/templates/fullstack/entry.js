// Client entry for `esdev start` / `esdev build`.
// Same lines as the SPA starter: the route map and its loader patterns come
// from the `@afw/routes` virtual module, and server-rendered markup (when
// the server served this page) is adopted instead of rebuilt.
import { mountApp } from "@opentf/afw";
import { guard, pages, loaderRoutes } from "@afw/routes";

mountApp({ pages, guard, loaders: loaderRoutes, target: document.getElementById("app") });
