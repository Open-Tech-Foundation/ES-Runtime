/**
 * The blocking commands, and where each of them keeps its timeout.
 *
 * A blocking command holds the connection for as long as it blocks. That is
 * inherent — the server sends no reply until it has one, and a connection is
 * one conversation — so a bounded `BLPOP key 5` is a five-second stall the
 * caller asked for, and that is theirs to choose.
 *
 * **A timeout of `0` means "forever"**, and that is not a stall but a stuck
 * connection: nothing else on it will ever run, and through a pool the
 * connection is gone for the life of the process while every other caller
 * fails on `acquireTimeout` with a message about pool exhaustion rather than
 * about the cause. That form is refused.
 *
 * The tables exist because Redis puts the timeout in three different places,
 * which is exactly the sort of thing that is wrong in a comment and right in
 * code with tests.
 */
import type { CommandArg } from "./resp.js";

/** `BLPOP key [key …] timeout` — the timeout is the **last** argument. */
const TIMEOUT_LAST = new Set([
  "BLPOP",
  "BRPOP",
  "BLMOVE",
  "BRPOPLPUSH",
  "BZPOPMIN",
  "BZPOPMAX",
  // `WAIT numreplicas timeout` and `WAITAOF numlocal numreplicas timeout` are
  // blocking in the same way and spell 0 as "forever" the same way, even though
  // their timeout is in milliseconds rather than seconds.
  "WAIT",
  "WAITAOF",
]);

/** `BLMPOP timeout numkeys key …` — the timeout comes **first**. */
const TIMEOUT_FIRST = new Set(["BLMPOP", "BZMPOP"]);

/** `XREAD [COUNT n] [BLOCK ms] STREAMS …` — behind a keyword. */
const BLOCK_KEYWORD = new Set(["XREAD", "XREADGROUP"]);

/** Every command that can block, for the documentation and the tests. */
export const BLOCKING_COMMANDS: readonly string[] = [
  ...TIMEOUT_LAST,
  ...TIMEOUT_FIRST,
  ...BLOCK_KEYWORD,
];

function isForever(arg: CommandArg | undefined): boolean {
  if (arg === undefined) return false;
  // A malformed command — no timeout where one belongs — is the server's to
  // reject, with its own message. `Number(undefined)` is NaN, which is not 0.
  const value = typeof arg === "bigint" ? Number(arg) : Number(arg as never);
  return value === 0;
}

/**
 * Whether `args` is a blocking command asked to block **indefinitely**.
 *
 * `null` when it is not blocking at all or is bounded; otherwise the command's
 * name, so the caller can say which one in the error.
 */
export function blocksForever(args: readonly CommandArg[]): string | null {
  const first = args[0];
  if (typeof first !== "string") return null;
  const name = first.toUpperCase();

  if (TIMEOUT_LAST.has(name)) {
    return isForever(args[args.length - 1]) ? name : null;
  }
  if (TIMEOUT_FIRST.has(name)) {
    return isForever(args[1]) ? name : null;
  }
  if (BLOCK_KEYWORD.has(name)) {
    // The options come before `STREAMS`, and everything after it is keys and
    // IDs — a stream legitimately called `BLOCK` must not be read as the
    // option, which is why this stops rather than scanning the whole command.
    for (let i = 1; i < args.length; i++) {
      const token = args[i];
      if (typeof token !== "string") continue;
      const upper = token.toUpperCase();
      if (upper === "STREAMS") return null;
      if (upper === "BLOCK") return isForever(args[i + 1]) ? name : null;
    }
    return null;
  }
  return null;
}

/** The refusal, phrased so the fix is in the message. */
export function foreverMessage(name: string): string {
  return (
    `${name} with a timeout of 0 blocks forever, and would hold this connection ` +
    `for the life of the process — every other command on it, including a pool's ` +
    `other callers, would queue behind it and never run. Give it a timeout, or ` +
    `open a connection for it with { blocking: true }, which says that tying ` +
    `that one up is the point.`
  );
}
