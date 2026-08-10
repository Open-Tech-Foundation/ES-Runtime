/**
 * Redis says what went wrong in a leading word — `WRONGTYPE`, `NOAUTH`,
 * `LOADING` — followed by prose.
 *
 * That word is the closest thing Redis has to an error code, and it is what
 * this maps from. It is a weaker source than PostgreSQL's SQLSTATE: it is not
 * standardized, a plain `ERR` covers most of the surface, and the interesting
 * detail is often only in the prose. So the prefix is tried first and a few
 * `ERR` messages are matched afterwards, which is as far as it goes — the
 * original is always kept on `backendCode`, because a portable name is a
 * summary and this vocabulary loses more in summary than most.
 */
import { DbErrorCode } from "runtime:db";

/** Error prefix → the portable code an application branches on. */
const BY_PREFIX: Record<string, string> = {
  // Authentication and authorization. `NOPERM` is an ACL refusal rather than a
  // failed login, but both answer the same question — this connection may not
  // do that — and an application retries neither.
  NOAUTH: DbErrorCode.AuthFailed,
  WRONGPASS: DbErrorCode.AuthFailed,
  NOPERM: DbErrorCode.AuthFailed,

  // Transient, and the caller should try again.
  LOADING: DbErrorCode.Busy, // still reading the dataset from disk
  BUSY: DbErrorCode.Busy, // a script is running and will not yield
  TRYAGAIN: DbErrorCode.Busy, // a cluster slot is resharding
  TIMEOUT: DbErrorCode.Timeout,

  // A replica, asked to write.
  READONLY: DbErrorCode.ReadOnly,

  // `BUSYGROUP` is "that consumer group already exists", which is the same
  // shape of answer a unique index gives: you asked to create something that is
  // already there.
  BUSYGROUP: DbErrorCode.UniqueViolation,

  // The server shedding load rather than failing the command: it is at its
  // client limit. The caller backs off and retries, which is what `Throttled`
  // means — `Busy` is one resource held by someone else.
  MAXCLIENTS: DbErrorCode.Throttled,

  // Cluster redirects. This driver does not follow them (see the README), and
  // the message says so rather than leaving a bare `MOVED 3999 …` to be
  // interpreted by whoever reads the log.
  MOVED: DbErrorCode.Unsupported,
  ASK: DbErrorCode.Unsupported,
  CROSSSLOT: DbErrorCode.Unsupported,
};

/**
 * Messages worth classifying that arrive under a bare `ERR`.
 *
 * Matching on prose is the thing the PostgreSQL driver's notes warn against,
 * and it is done here only where Redis leaves no alternative — these have no
 * prefix of their own. The strings are the stable part of messages that have
 * not changed since Redis 2.
 */
const BY_MESSAGE: [RegExp, string][] = [
  [/unknown command/i, DbErrorCode.Syntax],
  [/wrong number of arguments/i, DbErrorCode.Syntax],
  [/syntax error/i, DbErrorCode.Syntax],
  [/value is not an integer or out of range/i, DbErrorCode.Syntax],
  [/DB index is out of range/i, DbErrorCode.Syntax],
  // Refused a write because it is at `maxmemory`. Not a constraint and not
  // transient in the way `LOADING` is, but the caller can back off and retry,
  // which is what `Busy` means to one.
  [/OOM command not allowed/i, DbErrorCode.Busy],
  // Redis sends this one under a bare `ERR`, with no prefix of its own.
  [/max number of clients reached/i, DbErrorCode.Throttled],
];

/**
 * The portable code for an error reply.
 *
 * `WRONGTYPE` is deliberately **not** in the table. It is the most Redis-specific
 * failure there is — you ran a list command against a hash — and none of the
 * portable codes means that. Mapping it to the nearest one would tell an
 * application something false; `ERR_DB_BACKEND` with `backendCode: "WRONGTYPE"`
 * tells it the truth, which is that this needs Redis-specific handling.
 */
export function portableCode(prefix: string, message: string): string {
  const exact = BY_PREFIX[prefix];
  if (exact !== undefined) return exact;
  for (const [pattern, code] of BY_MESSAGE) {
    if (pattern.test(message)) return code;
  }
  return DbErrorCode.Backend;
}

/** Where a cluster told us to go instead. */
export interface Redirect {
  /** `MOVED` — the slot has moved for good; `ASK` — just this one command. */
  readonly kind: "MOVED" | "ASK";
  readonly slot: number;
  readonly host: string;
  readonly port: number;
}

/**
 * Parses `MOVED 3999 127.0.0.1:6381` into somewhere to go.
 *
 * IPv6 endpoints are spelled with the port after the last colon and the address
 * possibly containing several, so the split is from the right. An empty host —
 * which a cluster sends while a node's address is not yet known — is not
 * somewhere anyone can be redirected to, so it is refused rather than dialled.
 */
export function parseRedirect(prefix: string, message: string): Redirect | null {
  if (prefix !== "MOVED" && prefix !== "ASK") return null;
  const parts = message.trim().split(/\s+/);
  if (parts.length < 3) return null;
  const slot = Number(parts[1]);
  const endpoint = parts[2]!;
  const colon = endpoint.lastIndexOf(":");
  if (!Number.isInteger(slot) || colon <= 0) return null;
  const host = endpoint.slice(0, colon);
  const port = Number(endpoint.slice(colon + 1));
  if (host === "" || !Number.isInteger(port) || port <= 0) return null;
  return { kind: prefix, slot, host, port };
}

/** A cluster redirect, spelled so the reader knows what to do about it. */
export function redirectMessage(prefix: string, message: string): string {
  return (
    `${message} — this server is part of a cluster and redirected the command. ` +
    `@opentf/esrun-redis does not follow ${prefix} redirects; connect to the node that owns the key, ` +
    `or use a proxy that presents the cluster as a single server.`
  );
}
