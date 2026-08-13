/**
 * The loader's result, as a `<script>` the browser reads before the bundle
 * runs — which is what lets the client's first render match the markup the
 * server sent, rather than fetching everything a second time.
 *
 * It imports nothing, which is what makes it testable: `esdev test` runs each
 * file unbundled, and anything that reaches React reaches CommonJS, which the
 * runtime does not load.
 */
export function serialize(data: unknown): string {
  // `<` is escaped because a string in the data could otherwise close the
  // script tag — `{ "body": "</script><script>…" }` is the oldest injection
  // there is, and JSON.stringify does nothing about it.
  const json = JSON.stringify(data).replace(/</g, "\\u003c");
  return `<script>window.__DATA__=${json}</script>`;
}
