import { Navbar } from "@opentf/afw-docs";

import SiteFooter from "../components/SiteFooter.jsx";
import config from "../afw.config.js";

// Site-wide SEO defaults, merged under every route's own `metadata`: the
// template brands each page title, and the description stands in for a page
// that has none. Canonical and og:url come from each page's path.
export const metadata = {
  titleTemplate: "%s — ES-Runtime",
  description:
    "V8-based ECMAScript runtime, WinterTC-compliant, I/O-injectable, capability-secured. ESM-only, built on Rust.",
  openGraph: { siteName: "ES-Runtime", type: "website" },
  meta: [{ property: "og:locale", content: "en_US" }],
};

// Site-wide shell: the afw-docs Navbar + Footer wrap every route (landing, docs,
// api/esdev). Docs/api/esdev sections render `DocsLayout` with `frame={false}` so they slot
// their sidebar · content · TOC grid inside this shared chrome.
export default function RootLayout(props) {
  return (
    <div class="afw-shell">
      <Navbar config={config.docs} />
      <div class="afw-shell-body">{props.children}</div>
      <SiteFooter />
    </div>
  );
}
