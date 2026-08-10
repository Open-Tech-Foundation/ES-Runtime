import { defineDocsConfig } from "@opentf/web-docs/config";

const GITHUB = "https://github.com/Open-Tech-Foundation/ES-Runtime";

export default defineDocsConfig({
  // Canonical site origin — required for production builds (absolute feed/OG URLs).
  site: { url: "https://esrun.opentechf.org" },

  docs: {
    title: "ES-Runtime",
    // Bump on each release, to match the workspace Cargo.toml.
    version: "v0.22.0",
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
