/**
 * Topic names, topic filters, and the rules that tell them apart.
 *
 * MQTT has one namespace and two ways to write in it. A **topic name** — what a
 * PUBLISH carries — is a concrete path and may not contain a wildcard. A
 * **topic filter** — what a SUBSCRIBE carries — may, and the two wildcards have
 * positions the specification pins down: `+` matches exactly one level and must
 * occupy a whole level on its own, while `#` matches every level from there on
 * and must be the last character, either alone or right after a `/`. A filter
 * that breaks either rule is a protocol error, and a broker would refuse the
 * subscription anyway, so it is caught here where the message can still be
 * useful.
 *
 * Two smaller rules ride along. A topic must be at least one character and at
 * most 65535 bytes of UTF-8 (the string form's own prefix is two octets). And a
 * leading wildcard does not match a topic that begins with `$`: the `$`-prefixed
 * space is reserved for the broker's own use, so `#` does not swallow `$SYS/...`
 * and a subscriber that wants it says `$SYS/#`.
 */

import { ProtocolError } from "./errors.js";

const ENCODER = new TextEncoder();

/** The largest topic, in UTF-8 bytes — the string form's two-octet prefix. */
const MAX_TOPIC_BYTES = 65535;

function checkLength(what: string, value: string): void {
  if (value.length === 0) {
    throw new ProtocolError(`a ${what} must be at least one character`);
  }
  if (ENCODER.encode(value).length > MAX_TOPIC_BYTES) {
    throw new ProtocolError(`a ${what} is at most ${MAX_TOPIC_BYTES} bytes of UTF-8`);
  }
}

/** Throws unless `name` is a valid topic name (no wildcards). */
export function validateTopicName(name: string): void {
  if (typeof name !== "string") {
    throw new ProtocolError(`a topic name must be a string; got ${typeof name}`);
  }
  checkLength("topic name", name);
  if (name.includes("+") || name.includes("#")) {
    throw new ProtocolError(`a topic name must not contain a wildcard: ${JSON.stringify(name)}`);
  }
}

/**
 * Splits a shared-subscription filter, `$share/{group}/{filter}`, or returns
 * `null` when it is an ordinary filter.
 */
export function parseSharedFilter(filter: string): { group: string; filter: string } | null {
  if (!filter.startsWith("$share/")) return null;
  const rest = filter.slice("$share/".length);
  const slash = rest.indexOf("/");
  if (slash <= 0) {
    throw new ProtocolError(
      `a shared subscription is $share/{group}/{filter}: ${JSON.stringify(filter)}`,
    );
  }
  return { group: rest.slice(0, slash), filter: rest.slice(slash + 1) };
}

/** Throws unless `filter` is a valid topic filter, wildcards and all. */
export function validateTopicFilter(filter: string): void {
  if (typeof filter !== "string") {
    throw new ProtocolError(`a topic filter must be a string; got ${typeof filter}`);
  }
  const shared = parseSharedFilter(filter);
  const levels = (shared === null ? filter : shared.filter).split("/");
  const last = levels.length - 1;
  for (let i = 0; i < levels.length; i++) {
    const level = levels[i]!;
    if (level === "#") {
      if (i !== last) {
        throw new ProtocolError(
          `\`#\` must be the last level of a filter: ${JSON.stringify(filter)}`,
        );
      }
    } else if (level.includes("#")) {
      throw new ProtocolError(`\`#\` must occupy a whole level: ${JSON.stringify(filter)}`);
    } else if (level.includes("+") && level !== "+") {
      throw new ProtocolError(`\`+\` must occupy a whole level: ${JSON.stringify(filter)}`);
    }
  }
  const target = shared === null ? filter : shared.filter;
  checkLength("topic filter", target);
  if (target.startsWith("$share/")) {
    throw new ProtocolError("a shared subscription must not nest inside another");
  }
}

/**
 * Whether `filter` matches `topic`, as a broker would decide it.
 *
 * The client does not need this to receive messages — the broker routes — but
 * it is the rule the subscription layer and the tests reason about, and it is
 * only a dozen lines to state once, here, next to the wildcard rules it shares.
 */
export function topicMatches(filter: string, topic: string): boolean {
  const shared = parseSharedFilter(filter);
  const pattern = shared === null ? filter : shared.filter;
  const filterLevels = pattern.split("/");
  const topicLevels = topic.split("/");

  // A leading wildcard never reaches the reserved $-prefixed space.
  if (topic.startsWith("$") && (filterLevels[0] === "+" || filterLevels[0] === "#")) {
    return false;
  }

  for (let i = 0; i < filterLevels.length; i++) {
    const level = filterLevels[i]!;
    if (level === "#") return true;
    if (i >= topicLevels.length) return false;
    if (level !== "+" && level !== topicLevels[i]) return false;
  }
  return filterLevels.length === topicLevels.length;
}
