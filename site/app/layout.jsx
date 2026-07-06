import { Footer, Navbar } from "@opentf/web-docs";

import config from "../otfw.config.js";

// Site-wide shell: the web-docs Navbar + Footer wrap every route (landing, docs,
// api). Docs/api sections render `DocsLayout` with `frame={false}` so they slot
// their sidebar · content · TOC grid inside this shared chrome.
export default function RootLayout(props) {
  return (
    <div class="otfw-shell">
      <Navbar config={config.docs} />
      <div class="otfw-shell-body">{props.children}</div>
      <Footer config={config.docs} />
    </div>
  );
}