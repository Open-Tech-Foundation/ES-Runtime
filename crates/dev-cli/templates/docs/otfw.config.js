import { defineDocsConfig } from "@opentf/web-docs/config";

export default defineDocsConfig({
  // Canonical site origin — required for production builds.
  // Set this to your deployed origin, e.g. "https://example.com".
  site: { url: null },

  docs: {
    title: "My Docs",
    dir: "docs",
    nav: [{ label: "Docs", href: "/docs" }],
    footer: { text: "© 2026 My Project" },
    // Per-page "Last updated" (from git) and "Edit this page" (GitHub). Set repoUrl to
    // your repository root; links use <repoUrl>/edit/main/<source-path>.
    repoUrl: null, // e.g. "https://github.com/you/your-repo"
    lastUpdated: true,
  },
});
