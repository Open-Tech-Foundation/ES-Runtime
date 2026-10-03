import { defineDocsConfig } from "@opentf/web-docs/config";

export default defineDocsConfig({
  // Canonical site origin — required for production builds (sitemap, feeds).
  // Set this to your deployed origin.
  site: { url: "https://example.com" },

  docs: {
    title: "{{name}}",
    dir: "docs",
    nav: [{ label: "Docs", href: "/docs" }],
    footer: { text: "© 2026 {{name}}" },
    // Static search, indexed into dist/_search/ after the release prerender.
    search: { provider: "otf" },
    // Per-page "Last updated" (from git) and "Edit this page" (GitHub). Set repoUrl to
    // your repository root; links use <repoUrl>/edit/main/<source-path>.
    repoUrl: null, // e.g. "https://github.com/you/your-repo"
    lastUpdated: true,
  },
});
