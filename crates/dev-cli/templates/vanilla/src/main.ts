import { hello } from "./hello.ts";

const heading = document.createElement("h1");
heading.textContent = hello("world");
document.getElementById("app")?.append(heading);
