// Single source of truth for the site's discoverability metadata.
//
// `sitemap.xml` is generated from the actual routes on disk (so it can never go
// stale), and `llms.txt` is generated from the routes + the curated descriptions
// below. When you add a page under app/, the build warns if it has no entry here
// — so new routes get a description instead of being silently dropped. See
// scripts/discoverability.mjs and vite-discoverability.mjs.

export const SITE_URL = "https://esrun.opentechf.org";
export const SITE_NAME = "ES-Runtime";

// The one-line summary (llms.txt blockquote).
export const TAGLINE =
  "esrun — a secure, V8-based, WinterTC-compliant JavaScript runtime. ESM-only, built on Rust.";

// The intro paragraph (llms.txt body).
export const SUMMARY =
  "ES-Runtime (`esrun`) runs standard ECMAScript modules with only the Web-platform APIs of the WinterTC Minimum Common API — `fetch`, `URL`, streams, WebCrypto, encoding, timers, events — and no bespoke globals. Host features (filesystem, network, HTTP server, processes) are exposed as asynchronous `runtime:` standard modules. It is ESM-only and permanently so — no CommonJS, no Node.js compatibility layer, no transpilation. The engine is V8 (with a baked startup snapshot); the host is Rust, so the runtime stays memory-safe and crash-resistant. Startup is fast and the memory footprint is low.";

// Order in which sections appear in llms.txt.
export const SECTION_ORDER = ["Documentation", "API reference", "Comparisons", "Guides"];

// route -> { section, title, description }. Insertion order is preserved within
// a section, so list these the way you want them read.
export const PAGES = {
  // Documentation
  "/docs": { section: "Documentation", title: "Overview", description: "what ES-Runtime is, its design, and how the pieces fit together." },
  "/docs/install": { section: "Documentation", title: "Installation", description: "install the `esrun` CLI (install script, `esrun upgrade` self-update) and run a first module." },
  "/docs/modules": { section: "Documentation", title: "Module system", description: "ESM-only loading — static/dynamic `import`, top-level await, `import.meta`, and the `runtime:` built-in module scheme." },
  "/docs/globals": { section: "Documentation", title: "Global objects", description: "the Web-standard globals available (fetch, URL, streams, WebCrypto, encoding, timers, events) and what is intentionally absent." },
  "/docs/security": { section: "Documentation", title: "Security model", description: "how host I/O is mediated — capability checks enforced at the op boundary." },
  "/docs/scope": { section: "Documentation", title: "Scope & non-goals", description: "a runtime, not a toolchain — explicitly no Node.js compatibility, CommonJS, TypeScript, JSX, bundler, package installer, test runner, FFI, or workers." },
  "/docs/migration": { section: "Documentation", title: "Migration guide", description: "moving from Node.js, Bun, or Deno — equivalents for `process`/env, the filesystem, and the explicit `runtime:` import model (no ambient globals)." },
  "/docs/typescript": { section: "Documentation", title: "TypeScript setup", description: "editor types for the `runtime:*` modules via `esrun types --install` (writes the definitions into `node_modules` and wires `tsconfig.json`)." },
  "/docs/errors": { section: "Documentation", title: "Error diagnostics", description: "how uncaught exceptions and unhandled rejections are reported — stack traces, source positions, and the CLI error format." },

  // API reference
  "/api": { section: "API reference", title: "API overview", description: "the surface area — the `runtime:` standard modules and the CLI." },
  "/api/cli": { section: "API reference", title: "esrun CLI", description: "command-line usage — running a module, `-e` inline snippets, `--timeout`, `esrun types` (and `types --install`), `esrun upgrade`." },
  "/api/process": { section: "API reference", title: "runtime:process", description: "`env`, `args`, `cwd()`, `platform`, `arch`, `exit()`." },
  "/api/path": { section: "API reference", title: "runtime:path", description: "platform-aware path utilities — `join`, `resolve`, `dirname`, `basename`, `parse`, and `file:` URL interop." },
  "/api/fs": { section: "API reference", title: "runtime:fs", description: "Blob-based async file I/O — `file()` handles, `write()`, `readDir`, `stat`, `mkdir`, `remove`, `rename`, and `Glob`; confined to a root jail. Gated on FileRead/FileWrite." },
  "/api/net": { section: "API reference", title: "runtime:net", description: "TCP sockets following the WinterTC Sockets API — `connect()` and `listen()`. Gated on Net/NetListen." },
  "/api/http": { section: "API reference", title: "runtime:http", description: "an HTTP/1.1 server, `serve((request) => response)`, using the Web `Request`/`Response` objects. Gated on NetListen." },
  "/api/parsers": { section: "API reference", title: "runtime:parsers", description: "high-performance native parsers for JSONL, XML, YAML, and TOML via streams and synchronous functions." },
  "/api/websocket": { section: "API reference", title: "runtime:websocket", description: "WebSocket client and server functionality native to the engine." },

  // Comparisons
  "/docs/comparison": { section: "Comparisons", title: "esrun vs Node.js, Bun, Deno", description: "how esrun differs in goals, API surface, module system, and security posture." },
  "/docs/benchmarks": { section: "Comparisons", title: "Benchmarks", description: "cross-runtime benchmarks (startup, memory, crypto, JSON, HTTP, fs, and more) against Node.js, Bun, Deno, and LLRT, with the methodology and honest trade-offs." },

  // Guides
  "/docs/guides/file-handling": { section: "Guides", title: "File handling", description: "reading, writing, streaming, and globbing files with `runtime:fs`." },
  "/docs/glob": { section: "Guides", title: "Glob matching", description: "match and scan paths with the `Glob` API — every supported pattern token (`*`, `**`, `?`, `[abc]`, `{a,b}`, `!`)." },
  "/docs/process": { section: "Guides", title: "Process & Env", description: "reading environment variables, arguments, and the working directory via `runtime:process`." },
  "/docs/path": { section: "Guides", title: "Path handling", description: "building and parsing paths with `runtime:path` — `join`, `resolve`, `dirname`, `basename`, `extname`." },
  "/docs/http": { section: "Guides", title: "HTTP server", description: "serving requests with `runtime:http` `serve(options, handler)` and web `Request`/`Response`; frameworks like Hono work out of the box." },
  "/docs/urlpattern": { section: "Guides", title: "URLPattern", description: "native support for the URLPattern web API for routing and parsing URLs." },
  "/docs/guides/networking": { section: "Guides", title: "Networking", description: "TCP client/server usage with `runtime:net`." },
  "/docs/guides/websocket": { section: "Guides", title: "WebSockets", description: "creating scalable WebSocket servers and clients." },
  "/docs/parsers/jsonl": { section: "Guides", title: "JSONL Parsing", description: "native streaming JSONLines implementation for processing massive logs." },
  "/docs/parsers/toml": { section: "Guides", title: "TOML Parsing", description: "synchronous high-performance TOML processing." },
  "/docs/parsers/xml": { section: "Guides", title: "XML Parsing", description: "synchronous and streaming fast XML processing." },
  "/docs/parsers/yaml": { section: "Guides", title: "YAML Parsing", description: "synchronous robust YAML processing." },
  "/docs/parsers/msgpack": { section: "Guides", title: "MessagePack Parsing", description: "synchronous fast MessagePack processing." },
};

// Static trailing sections of llms.txt (not derived from routes).
export const FOOTER = [
  {
    section: "Source",
    body: "- [GitHub repository](https://github.com/Open-Tech-Foundation/ES-Runtime): source code, issues, releases, and the canonical API docs.",
  },
  {
    section: "Full content",
    body: `- [llms-full.txt](${SITE_URL}/llms-full.txt): the full documentation (README + API reference) inlined as a single markdown file for one-fetch ingestion.`,
  },
];

// llms-full.txt — the canonical repository docs (README + docs/API.md) inlined
// as one markdown file for single-fetch LLM ingestion. `sources` are relative to
// the repo root and concatenated with a `---` rule between them.
export const FULL_DOC = {
  tagline:
    "A secure, V8-based, WinterTC-compliant, capability-secured embeddable JavaScript runtime. ESM-only, built on Rust. Available as the `esrun` CLI and as an embeddable Rust library.",
  note: "This file inlines ES-Runtime's canonical documentation (the repository README and docs/API.md) for single-fetch LLM ingestion, generated at build time. Source of truth: https://github.com/Open-Tech-Foundation/ES-Runtime.",
  sources: ["README.md", "docs/API.md"],
};

// Non-route paths to also list in the sitemap.
export const SITEMAP_EXTRA = [
  { path: "/llms.txt", priority: "0.5" },
  { path: "/llms-full.txt", priority: "0.5" },
];

// Routes to leave out of the sitemap (e.g. a 404 page), if any.
export const SITEMAP_EXCLUDE = new Set([]);
