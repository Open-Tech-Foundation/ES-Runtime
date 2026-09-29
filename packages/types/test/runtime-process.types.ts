// A type test for `runtime:process`'s timer references. Compiled by
// `tsc -p .`, never run.

import { refTimer, unrefTimer } from "runtime:process";

const heartbeat = setInterval(() => {}, 1000);
unrefTimer(heartbeat);
refTimer(heartbeat);
unrefTimer(setTimeout(() => {}, 10));

// @ts-expect-error — a timer id, as setTimeout returned it.
unrefTimer("heartbeat");

// `parseArgs`: values are typed from the declared options.
import { parseArgs } from "runtime:process";

const parsed = parseArgs({
  options: {
    port: { type: "string", short: "p", default: "8080" },
    verbose: { type: "boolean", short: "v" },
    tag: { type: "string", multiple: true },
  },
  allowPositionals: true,
});
const port: string = parsed.values.port;
const verbose: boolean | undefined = parsed.values.verbose;
const tags: string[] | undefined = parsed.values.tag;
const rest: string[] = parsed.positionals;
// @ts-expect-error — an option that was not declared.
parsed.values.missing;
// @ts-expect-error — a boolean is not a string.
const wrong: string = parsed.values.verbose;

const withTokens = parseArgs({ args: ["-v"], options: { v: { type: "boolean" } }, tokens: true });
for (const token of withTokens.tokens) {
  if (token.kind === "option") {
    const raw: string = token.rawName;
    void raw;
  }
}

const loose = parseArgs({ strict: false });
const anything: string | boolean | (string | boolean)[] | undefined = loose.values.whatever;
void [port, verbose, tags, rest, wrong, anything];

// `stdin`: lines are strings, a question may meet the end of input.
import { stdin } from "runtime:process";

async function stdinTypes() {
  const answer: string | null = await stdin.question("Name? ");
  for await (const line of stdin.lines()) {
    const text: string = line;
    void text;
  }
  const reader = stdin.readable.getReader();
  const chunk: Uint8Array | undefined = (await reader.read()).value;
  const raw: boolean = stdin.setRawMode(true).isRaw;
  // @ts-expect-error — a question can meet the end of input.
  const always: string = await stdin.question();
  const name: string | null = prompt("Name?", "anon");
  const sure: boolean = confirm("Sure?");
  void [answer, chunk, raw, always, name, sure];
}
void stdinTypes;
