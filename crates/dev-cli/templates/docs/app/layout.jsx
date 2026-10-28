import { Footer, Navbar } from "@opentf/afw-docs";
import config from "../afw.config.js";

export default function Layout(props) {
  return (
    <div class="afw-shell">
      <Navbar config={config.docs} />
      <div class="afw-shell-body">{props.children}</div>
      <Footer config={config.docs} />
    </div>
  );
}
