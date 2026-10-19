/**
 * Subjects: the only namespace NATS has, and the two ways to write in it.
 *
 * A **subject** is a dot-separated list of tokens. A **subscription subject**
 * may carry wildcards — `*` matches one token, `>` matches one or more and must
 * be last — while a **publish subject** is concrete and may not. The rules are
 * small but sharp: a token is never empty, so `a..b` and a trailing dot are
 * refused; a wildcard owns its whole token, so `a*` is a literal token and not a
 * pattern; and the `$`-prefixed space the server keeps for itself (`$SYS`,
 * `$JS.API`, `_INBOX` is not `$`-prefixed but is reserved by convention) is
 * never reached by a filter that does not itself begin with `$`. Catching this
 * here means a bad subject is a clear local error rather than a `-ERR` that
 * closes the connection.
 */

import { ProtocolError } from "./errors.js";

/** The alphabet for an inbox token: URL-safe, and never a subject delimiter. */
const INBOX_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

/** Whether a single subject token carries a byte that cannot appear in one. */
function checkToken(token: string, what: string): void {
  if (token.length === 0) {
    throw new ProtocolError(`a ${what} has an empty token: tokens are never empty`);
  }
  for (const char of token) {
    if (char === " " || char === "\t" || char === "\r" || char === "\n") {
      throw new ProtocolError(`a ${what} token contains whitespace: ${JSON.stringify(token)}`);
    }
    if (char === ".") {
      throw new ProtocolError(`a ${what} token contains a dot: ${JSON.stringify(token)}`);
    }
  }
}

/**
 * Throws unless `subject` is a valid subject.
 *
 * With `wildcards` (a subscription), `*` and `>` are the two patterns the
 * specification allows, and `>` must be the final token. Without it (a publish),
 * either character is refused outright.
 */
export function validateSubject(subject: string, wildcards: boolean): void {
  if (typeof subject !== "string") {
    throw new ProtocolError(`a subject must be a string; got ${typeof subject}`);
  }
  if (subject.length === 0) {
    throw new ProtocolError("a subject must be at least one token");
  }
  const tokens = subject.split(".");
  for (let i = 0; i < tokens.length; i++) {
    const token = tokens[i]!;
    const last = i === tokens.length - 1;
    if (token === ">") {
      if (!wildcards) {
        throw new ProtocolError(`a publish subject must not contain a wildcard: ${subject}`);
      }
      if (!last) {
        throw new ProtocolError(`\`>\` must be the last token of a subject: ${subject}`);
      }
      continue;
    }
    if (token === "*") {
      if (!wildcards) {
        throw new ProtocolError(`a publish subject must not contain a wildcard: ${subject}`);
      }
      continue;
    }
    if (token.includes("*") || token.includes(">")) {
      throw new ProtocolError(
        `a wildcard must occupy a whole token, not part of one: ${JSON.stringify(token)}`,
      );
    }
    checkToken(token, wildcards ? "subject filter" : "subject");
  }
}

/** Throws unless `queue` is a valid queue-group name. */
export function validateQueue(queue: string): void {
  if (typeof queue !== "string" || queue.length === 0) {
    throw new ProtocolError("a queue group must be a non-empty string");
  }
  for (const char of queue) {
    if (char === " " || char === "\t" || char === "\r" || char === "\n") {
      throw new ProtocolError(`a queue group contains whitespace: ${JSON.stringify(queue)}`);
    }
  }
}

/**
 * Whether a subscription `filter` matches a concrete `subject`, as the server
 * would decide it.
 *
 * The client does not need this to receive messages — the server routes, and a
 * `MSG` names the subscription by its sid — but it is the rule the tests and a
 * stub broker reason about, and the `$`-reserved case is easy to get wrong.
 */
export function subjectMatches(filter: string, subject: string): boolean {
  const filterTokens = filter.split(".");
  const subjectTokens = subject.split(".");

  // A filter that does not begin in the reserved space never reaches it.
  if (subject.startsWith("$") && !filter.startsWith("$")) return false;

  for (let i = 0; i < filterTokens.length; i++) {
    const token = filterTokens[i]!;
    if (token === ">") return i < subjectTokens.length;
    if (i >= subjectTokens.length) return false;
    if (token !== "*" && token !== subjectTokens[i]) return false;
  }
  return filterTokens.length === subjectTokens.length;
}

/**
 * A fresh inbox subject, `_INBOX.<token>`.
 *
 * Request/reply in NATS is a publish with a reply subject and a subscription to
 * it. The token is long enough to be collision-free across a fleet of callers
 * without coordination; the prefix is the convention every client uses.
 */
export function createInbox(prefix: string = "_INBOX"): string {
  let token = "";
  for (let i = 0; i < 22; i++) {
    token += INBOX_ALPHABET[Math.floor(Math.random() * INBOX_ALPHABET.length)];
  }
  return `${prefix}.${token}`;
}
