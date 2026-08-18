/**
 * Every path the static build writes as a file.
 *
 * A route table with a `:slug` in it does not know its own URLs — only the data
 * behind it does — so the expansion happens here, against that data, rather
 * than in the build script where it would drift.
 *
 * **A path left out of this list is not a broken page.** It is served the shell
 * and matched in the browser instead, its loader run there — which is what a
 * page behind a login wants.
 */
export async function staticPaths(): Promise<string[]> {
  return ["/"];
}
