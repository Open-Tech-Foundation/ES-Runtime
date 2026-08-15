import { defineDocsConfig } from "@opentf/web-docs/config";

import versions from "./src/versions.js";

const GITHUB = "https://github.com/Open-Tech-Foundation/ES-Runtime";

export default defineDocsConfig({
  // Canonical site origin — required for production builds (absolute feed/OG URLs).
  site: { url: "https://esrun.opentechf.org" },

  docs: {
    title: "ES-Runtime",
    // The navbar badge is `esrun`'s — it is the product a visitor came for, and
    // one badge is all the chrome has room for. `esdev` ships its own version
    // where it is the subject: components/Versions.jsx.
    version: `v${versions.esrun}`,
    github: GITHUB,
    // Enables the per-page "Edit this page" link (with `lastUpdated`).
    repoUrl: GITHUB,
    // Top-level navbar links (shared across the whole site).
    nav: [
      { label: "Home", href: "/" },
      { label: "Docs", href: "/docs" },
      { label: "API", href: "/api" },
    ],
    // Footer chrome is rendered by components/SiteFooter.jsx.
    footer: {},
    // Navbar search (Pagefind index built during `otfw build --ssg`).
    search: { provider: "pagefind" },
    // Per-page "Last updated" line, from the file's last git commit.
    lastUpdated: true,
  },
});
