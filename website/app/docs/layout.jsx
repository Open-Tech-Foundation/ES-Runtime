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
    ],
  },
  {
    // The development binary. Its own group rather than a Guides entry: what it
    // offers is a second command line, not another API to use from JS.
    title: "Development",
    items: [
      { title: "esdev", path: "/docs/esdev" },
      { title: "Starting a project", path: "/docs/esdev/create" },
      { title: "TypeScript setup", path: "/docs/esdev/typescript" },
      { title: "Bundling", path: "/docs/esdev/build" },
      { title: "Writing a plugin", path: "/docs/esdev/plugins" },
      { title: "The dev loop", path: "/docs/esdev/start" },
      { title: "Testing", path: "/docs/esdev/test" },
      { title: "Debugging", path: "/docs/esdev/debugging" },
      { title: "Tracing permissions", path: "/docs/esdev/permissions" },
    ],
  },
  {
    title: "Guides",
    items: [
      { title: "File handling", path: "/docs/guides/file-handling" },
      { title: "Glob matching", path: "/docs/glob" },
      { title: "Process & Env", path: "/docs/process" },
      { title: "Path handling", path: "/docs/path" },
      { title: "Databases", path: "/docs/db" },
      { title: "Redis", path: "/docs/db/redis" },
      { title: "Drivers & ORMs", path: "/docs/db/authoring" },
      { title: "Sockets", path: "/docs/guides/networking" },
      { title: "UDP", path: "/docs/guides/udp" },
      { title: "Subprocesses", path: "/docs/guides/subprocess" },
      { title: "Workers", path: "/docs/guides/workers" },
      { title: "HTTP server", path: "/docs/http" },
      { title: "WebSockets", path: "/docs/guides/websocket" },
      { title: "URLPattern", path: "/docs/urlpattern" },
      { title: "WebAssembly & WASI", path: "/docs/wasm" },
      { title: "Hashing", path: "/docs/guides/hashing" },
      { title: "Securing the runtime", path: "/docs/guides/securing-runtime" },
      {
        title: "Text serialization",
        items: [
          { title: "XML", path: "/docs/serialization/xml" },
          { title: "YAML", path: "/docs/serialization/yaml" },
          { title: "TOML", path: "/docs/serialization/toml" },
          { title: "JSON Lines", path: "/docs/serialization/jsonl" },
        ],
      },
      {
        title: "Binary serialization",
        items: [
          { title: "MessagePack", path: "/docs/serialization/msgpack" },
          { title: "Protobuf", path: "/docs/serialization/protobuf" },
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
    // Deep behaviour, one page per subsystem: why it works the way it does and
    // what it costs, as opposed to what the API is (/api) or how to use it
    // (Guides).
    title: "Internals",
    items: [
      { title: "HTTP server", path: "/docs/internals/http" },
      { title: "Sockets", path: "/docs/internals/sockets" },
      { title: "The fetch client", path: "/docs/internals/fetch" },
      { title: "WebSockets", path: "/docs/internals/websockets" },
      { title: "Workers", path: "/docs/internals/workers" },
      { title: "The filesystem", path: "/docs/internals/filesystem" },
      { title: "Databases", path: "/docs/internals/database" },
      { title: "Paths", path: "/docs/internals/path" },
      { title: "Serialization", path: "/docs/internals/serialization" },
      { title: "WebCrypto", path: "/docs/internals/crypto" },
      { title: "WASI", path: "/docs/internals/wasi" },
      { title: "The bundler bridge", path: "/docs/internals/bundler" },
    ],
  },
  {
    title: "Runtime",
    items: [
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
