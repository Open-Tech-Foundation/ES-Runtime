// Client entry for `esdev start` / `esdev build`.
// Docs pages are routes like any other: the map comes from the
// `@otfw/routes` virtual module, and prerendered markup is adopted.
import { mountApp } from "@opentf/web";
import { guard, pages, loaderRoutes } from "@otfw/routes";

mountApp({ pages, guard, loaders: loaderRoutes, target: document.getElementById("app") });
