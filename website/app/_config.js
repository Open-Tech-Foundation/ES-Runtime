import { defineRoute } from "@opentf/afw";

// Every page is built to HTML at build time and adopted by the browser: the
// site is static, served from a CDN.
export default defineRoute({ render: "static" });
