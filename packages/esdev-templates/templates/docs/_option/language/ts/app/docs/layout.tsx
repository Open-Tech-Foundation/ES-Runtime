import { DocsLayout } from "@opentf/afw-docs";
import config from "../../afw.config.js";

export default function Layout(props: { children: unknown }) {
  return (
    <DocsLayout config={config.docs} frame={false}>
      {props.children}
    </DocsLayout>
  );
}
