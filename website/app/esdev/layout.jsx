import { DocsLayout } from "@opentf/web-docs";

import config from "../../otfw.config.js";

// Explicit sidebar tree for the /esdev section, matching the top-level API
// section while keeping the development binary's command and workflow pages
// together.
const NAV = [
  {
    title: "esdev",
    items: [
      { title: "Overview", path: "/esdev" },
      { title: "Starting a project", path: "/esdev/create" },
      { title: "TypeScript setup", path: "/esdev/typescript" },
      { title: "Bundling", path: "/esdev/build" },
      { title: "Writing a plugin", path: "/esdev/plugins" },
      { title: "The dev loop", path: "/esdev/start" },
      { title: "Testing", path: "/esdev/test" },
      { title: "Debugging", path: "/esdev/debugging" },
      { title: "Tracing permissions", path: "/esdev/permissions" },
    ],
  },
];

export default function EsdevSectionLayout(props) {
  return (
    <DocsLayout config={config.docs} nav={NAV} frame={false}>
      {props.children}
    </DocsLayout>
  );
}
