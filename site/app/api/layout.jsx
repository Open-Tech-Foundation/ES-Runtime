import { DocsLayout } from "@opentf/web-docs";

import config from "../../otfw.config.js";

// Explicit sidebar tree for the /api section (mirrors the original ApiShell). A
// single "Reference" group; passing `nav` keeps the curated order + group label.
const NAV = [
  {
    title: "Reference",
    items: [
      { title: "Overview", path: "/api" },
      { title: "CLI", path: "/api/cli" },
      { title: "runtime:process", path: "/api/process" },
      { title: "runtime:path", path: "/api/path" },
      { title: "runtime:fs", path: "/api/fs" },
      { title: "runtime:net", path: "/api/net" },
      { title: "runtime:http", path: "/api/http" },
      { title: "runtime:websocket", path: "/api/websocket" },
      { title: "runtime:serialization", path: "/api/serialization" },
    ],
  },
];

export default function ApiSectionLayout(props) {
  return (
    <DocsLayout config={config.docs} nav={NAV} frame={false}>
      {props.children}
    </DocsLayout>
  );
}
