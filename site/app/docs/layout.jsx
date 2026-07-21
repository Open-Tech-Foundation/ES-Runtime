import { DocsLayout } from "@opentf/web-docs";

import config from "../../otfw.config.js";

// The docs sidebar is an explicit tree (not the folder-derived one). web-docs
// generates its sidebar strictly from the folder structure, but several guide
// pages live at flat URLs (/docs/glob, /docs/process, /docs/path, /docs/http,
// /docs/urlpattern) rather than under /docs/guides/ — so folder-derived grouping
// would scatter them out of the "Guides" section. Passing `nav` keeps the curated
// grouping (mirroring the original hand-authored sidebar) while preserving URLs.
// Group nodes have `items` and no `path`; leaf nodes have `path`.
const NAV = [
  {
    title: "Getting started",
    items: [
      { title: "Overview", path: "/docs" },
      { title: "Installation", path: "/docs/install" },
      { title: "Scope & non-goals", path: "/docs/scope" },
      { title: "Migration guide", path: "/docs/migration" },
      { title: "TypeScript setup", path: "/docs/typescript" },
    ],
  },
  {
    title: "Guides",
    items: [
      { title: "File handling", path: "/docs/guides/file-handling" },
      { title: "Glob matching", path: "/docs/glob" },
      { title: "Process & Env", path: "/docs/process" },
      { title: "Path handling", path: "/docs/path" },
      { title: "Sockets", path: "/docs/guides/networking" },
      { title: "HTTP server", path: "/docs/http" },
      { title: "WebSockets", path: "/docs/guides/websocket" },
      { title: "URLPattern", path: "/docs/urlpattern" },
      { title: "WebAssembly & WASI", path: "/docs/wasm" },
      {
        title: "Text serialization",
        items: [
          { title: "XML Parser", path: "/docs/serialization/xml" },
          { title: "YAML Parser", path: "/docs/serialization/yaml" },
          { title: "TOML Parser", path: "/docs/serialization/toml" },
          { title: "JSONL Parser", path: "/docs/serialization/jsonl" },
        ],
      },
      {
        title: "Binary serialization",
        items: [
          { title: "MessagePack Parser", path: "/docs/serialization/msgpack" },
          { title: "Protobuf Parser", path: "/docs/serialization/protobuf" },
        ],
      },
    ],
  },
  {
    title: "Comparisons",
    items: [
      { title: "vs Node.js · Bun · Deno", path: "/docs/comparison" },
      { title: "Benchmarks", path: "/docs/benchmarks" },
    ],
  },
  {
    title: "Web standard APIs",
    items: [{ title: "Global objects", path: "/docs/globals" }],
  },
  {
    title: "Runtime",
    items: [
      { title: "Embedding (preview)", path: "/docs/embed" },
      { title: "Module system", path: "/docs/modules" },
      { title: "Security model", path: "/docs/security" },
      { title: "Error diagnostics", path: "/docs/errors" },
    ],
  },
];

export default function DocsSectionLayout(props) {
  return (
    <DocsLayout config={config.docs} nav={NAV} frame={false}>
      {props.children}
    </DocsLayout>
  );
}
