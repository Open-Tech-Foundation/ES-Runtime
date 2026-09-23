import { define, html, mount } from "@opentf/micro-ui";
import { hello } from "./hello.ts";

define("x-hello", () => () => html`<h1>${hello("world")}</h1>`);

const root = document.getElementById("app");
if (root) mount(root, "x-hello");
