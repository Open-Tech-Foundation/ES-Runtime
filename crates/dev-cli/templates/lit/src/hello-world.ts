import { LitElement, css, html } from "lit";
import { hello } from "./hello.ts";

export class HelloWorld extends LitElement {
  static override styles = css`
    h1 {
      font-size: 2rem;
    }
  `;

  static override properties = {
    name: { type: String },
  };

  declare name: string;

  constructor() {
    super();
    this.name = "world";
  }

  override render() {
    return html`<h1>${hello(this.name)}</h1>`;
  }
}

customElements.define("hello-world", HelloWorld);
