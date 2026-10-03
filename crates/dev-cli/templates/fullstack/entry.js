// Client entry for `esdev start` / `esdev build`.
// Same lines as the SPA starter: the route map and its loader patterns come
// from the `@otfw/routes` virtual module, and server-rendered markup (when
// the server served this page) is adopted instead of rebuilt.
import { mountApp } from "@opentf/web";
import { guard, pages, loaderRoutes } from "@otfw/routes";

mountApp({ pages, guard, loaders: loaderRoutes, target: document.getElementById("app") });
