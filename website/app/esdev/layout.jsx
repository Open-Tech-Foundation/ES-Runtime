import { DocsLayout } from "@opentf/web-docs";

import config from "../../otfw.config.js";

// Explicit sidebar tree for the /esdev section. The groups follow the order in
// which a developer normally learns the tool: start a project, build it, run
// the loop, then test and diagnose it.
const NAV = [
  {
    title: "Getting started",
    items: [
      { title: "Overview", path: "/esdev" },
      { title: "Starting a project", path: "/esdev/create" },
      { title: "TypeScript setup", path: "/esdev/typescript" },
    ],
  },
  {
    title: "Build",
    items: [
      { title: "Bundling", path: "/esdev/build" },
      { title: "Project builds", path: "/esdev/build/project" },
      { title: "Browser builds", path: "/esdev/build/browser" },
      { title: "Libraries", path: "/esdev/build/library" },
      {
        title: "Writing plugins",
        items: [
          { title: "Overview", path: "/esdev/plugins" },
          { title: "Lifecycle & hooks", path: "/esdev/plugins/lifecycle" },
          { title: "Dependencies & modules", path: "/esdev/plugins/dependencies" },
          { title: "Context & ordering", path: "/esdev/plugins/context" },
          { title: "Runtime & troubleshooting", path: "/esdev/plugins/runtime" },
        ],
      },
    ],
  },
  {
    title: "Development loop",
    items: [
      { title: "The dev loop", path: "/esdev/start" },
      { title: "Hot module replacement", path: "/esdev/start/hmr" },
      { title: "Previewing a release", path: "/esdev/start/preview" },
      { title: "Watch mode", path: "/esdev/watch" },
    ],
  },
  {
    title: "Testing",
    items: [
      { title: "Overview", path: "/esdev/test" },
      { title: "Writing tests", path: "/esdev/test/writing" },
      { title: "Running & isolating", path: "/esdev/test/running" },
      { title: "Snapshots", path: "/esdev/test/snapshots" },
      { title: "Mocks & fake timers", path: "/esdev/test/mocks" },
      { title: "Configuration", path: "/esdev/test/configuration" },
      { title: "Global setup", path: "/esdev/test/global-setup" },
      { title: "Reporters", path: "/esdev/test/reporters" },
      { title: "Coverage", path: "/esdev/test/coverage" },
      { title: "Imports", path: "/esdev/test/imports" },
      { title: "DOM testing", path: "/esdev/test/dom" },
      { title: "DOM parity", path: "/esdev/test/dom/parity" },
      { title: "Browser testing", path: "/esdev/test/browser" },
    ],
  },
  {
    title: "Diagnostics",
    items: [
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
