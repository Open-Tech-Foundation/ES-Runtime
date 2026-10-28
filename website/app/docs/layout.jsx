import { DocsLayout } from "@opentf/afw-docs";

import config from "../../afw.config.js";

// The docs sidebar is an explicit tree (not the folder-derived one). afw-docs
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
      {
        title: "Migration guide",
        items: [
          { title: "Overview", path: "/docs/migration" },
          { title: "From Node.js", path: "/docs/migration/node" },
          { title: "From Bun", path: "/docs/migration/bun" },
          { title: "From Deno", path: "/docs/migration/deno" },
        ],
      },
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
      { title: "UDP", path: "/docs/guides/udp" },
      { title: "Sending email", path: "/docs/guides/email" },
      {
        title: "Messaging",
        items: [
          { title: "RabbitMQ", path: "/docs/guides/rabbitmq" },
          { title: "MQTT", path: "/docs/guides/mqtt" },
          { title: "NATS", path: "/docs/guides/nats" },
        ],
      },
      { title: "Subprocesses", path: "/docs/guides/subprocess" },
      { title: "Workers", path: "/docs/guides/workers" },
      { title: "Durable workers", path: "/docs/guides/durable-workers" },
      {
        title: "HTTP server",
        items: [
          { title: "Overview", path: "/docs/http" },
          { title: "Basics", path: "/docs/http/basics" },
          { title: "Lifecycle & shutdown", path: "/docs/http/lifecycle" },
          { title: "Static files", path: "/docs/http/static" },
          { title: "HTTPS, HTTP/2 & trailers", path: "/docs/http/protocols" },
          { title: "Identity & deployment", path: "/docs/http/production" },
        ],
      },
      { title: "WebSockets", path: "/docs/guides/websocket" },
      { title: "URLPattern", path: "/docs/urlpattern" },
      { title: "WebAssembly & WASI", path: "/docs/wasm" },
      { title: "Hashing", path: "/docs/guides/hashing" },
      { title: "Webhooks", path: "/docs/guides/webhooks" },
      { title: "Images", path: "/docs/guides/images" },
      { title: "PDFs", path: "/docs/guides/pdf" },
      { title: "Archives", path: "/docs/guides/archive" },
      { title: "Request context", path: "/docs/guides/context" },
      { title: "Observing the runtime", path: "/docs/guides/diagnostics" },
      {
        title: "Securing the runtime",
        items: [
          { title: "Overview", path: "/docs/guides/securing-runtime" },
          { title: "Granting capabilities", path: "/docs/guides/securing-runtime/capabilities" },
          { title: "Deployment commands", path: "/docs/guides/securing-runtime/commands" },
          { title: "Hardening checklist", path: "/docs/guides/securing-runtime/hardening" },
        ],
      },
      {
        title: "Text serialization",
        items: [
          { title: "XML", path: "/docs/serialization/xml" },
          { title: "YAML", path: "/docs/serialization/yaml" },
          { title: "TOML", path: "/docs/serialization/toml" },
          { title: "CSV", path: "/docs/serialization/csv" },
          { title: "JSONC", path: "/docs/serialization/jsonc" },
          { title: "JSON5", path: "/docs/serialization/json5" },
          { title: "JSON Lines", path: "/docs/serialization/jsonl" },
        ],
      },
      {
        title: "Binary serialization",
        items: [
          { title: "MessagePack", path: "/docs/serialization/msgpack" },
          { title: "CBOR", path: "/docs/serialization/cbor" },
          { title: "Protobuf", path: "/docs/serialization/protobuf" },
        ],
      },
    ],
  },
  {
    // One section for every database, so a reader looking for theirs finds it
    // by name: the shared API first, then SQL engines, then key-value stores,
    // then building a driver.
    title: "Databases",
    items: [
      { title: "Overview", path: "/docs/db" },
      { title: "SQLite", path: "/docs/db/sqlite" },
      { title: "PostgreSQL", path: "/docs/db/postgres" },
      { title: "MySQL & MariaDB", path: "/docs/db/mysql" },
      { title: "MongoDB", path: "/docs/db/mongodb" },
      {
        title: "Redis",
        items: [
          { title: "Overview", path: "/docs/db/redis" },
          { title: "Concepts & connections", path: "/docs/db/redis/core" },
          { title: "Transactions & blocking", path: "/docs/db/redis/transactions" },
          { title: "Operations & reliability", path: "/docs/db/redis/operations" },
          { title: "Compatibility & limits", path: "/docs/db/redis/compatibility" },
        ],
      },
      { title: "Valkey", path: "/docs/db/valkey" },
      {
        title: "Drivers & ORMs",
        items: [
          { title: "Overview", path: "/docs/db/authoring" },
          { title: "Driver implementation", path: "/docs/db/authoring/backend" },
          { title: "Rows & value types", path: "/docs/db/authoring/data" },
          { title: "Production pitfalls", path: "/docs/db/authoring/production" },
          { title: "ORM checklist", path: "/docs/db/authoring/checklist" },
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
      {
        title: "HTTP server",
        items: [
          { title: "Overview", path: "/docs/internals/http" },
          { title: "Connections & limits", path: "/docs/internals/http/connections" },
          { title: "Handoff & draining", path: "/docs/internals/http/handoff" },
          { title: "Identity & observability", path: "/docs/internals/http/identity" },
          { title: "Cookies & CSRF", path: "/docs/internals/http/cookies" },
          { title: "Webhooks", path: "/docs/internals/http/webhooks" },
          { title: "Compared", path: "/docs/internals/http/comparison" },
        ],
      },
      { title: "Sockets", path: "/docs/internals/sockets" },
      { title: "The fetch client", path: "/docs/internals/fetch" },
      { title: "WebSockets", path: "/docs/internals/websockets" },
      { title: "Workers", path: "/docs/internals/workers" },
      { title: "Durable workers", path: "/docs/internals/durable-workers" },
      { title: "Async context", path: "/docs/internals/context" },
      { title: "Diagnostics", path: "/docs/internals/diagnostics" },
      { title: "The filesystem", path: "/docs/internals/filesystem" },
      { title: "Databases", path: "/docs/internals/database" },
      { title: "SMTP", path: "/docs/internals/smtp" },
      { title: "RabbitMQ", path: "/docs/internals/rabbitmq" },
      { title: "MQTT", path: "/docs/internals/mqtt" },
      { title: "NATS", path: "/docs/internals/nats" },
      { title: "Paths", path: "/docs/internals/path" },
      { title: "Serialization", path: "/docs/internals/serialization" },
      { title: "WebCrypto", path: "/docs/internals/crypto" },
      { title: "Images", path: "/docs/internals/images" },
      { title: "PDF", path: "/docs/internals/pdf" },
      { title: "Archives", path: "/docs/internals/archive" },
      { title: "WASI", path: "/docs/internals/wasi" },
      { title: "The bundler bridge", path: "/docs/internals/bundler" },
      { title: "Tailwind CSS", path: "/docs/internals/tailwind" },
    ],
  },
  {
    title: "Deployment",
    items: [
      { title: "Overview", path: "/docs/deployment" },
      { title: "Containers", path: "/docs/deployment/containers" },
      { title: "The RUNTIME_ENV convention", path: "/docs/deployment/runtime-env" },
      { title: "Docker Compose", path: "/docs/deployment/compose" },
      { title: "Linux server (systemd)", path: "/docs/deployment/systemd" },
      { title: "Reverse proxy and TLS", path: "/docs/deployment/proxy-and-tls" },
    ],
  },
  {
    title: "Runtime",
    items: [
      { title: "Module system", path: "/docs/modules" },
      { title: "import.meta", path: "/docs/guides/import-meta" },
      {
        title: "🔒 Security",
        items: [
          { title: "Overview", path: "/docs/security" },
          { title: "Permissions & capabilities", path: "/docs/security/permissions" },
          { title: "Dependency policy", path: "/docs/security/dependency-policy" },
          { title: "Processes and workers", path: "/docs/security/processes" },
          { title: "Network security", path: "/docs/security/networking" },
          { title: "Filesystem and secrets", path: "/docs/security/filesystem" },
          { title: "Cryptography and hardening", path: "/docs/security/cryptography" },
        ],
      },
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
