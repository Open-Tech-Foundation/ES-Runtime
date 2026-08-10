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

/** A cluster redirect, spelled so the reader knows what to do about it. */
export function redirectMessage(prefix: string, message: string): string {
  return (
    `${message} — this server is part of a cluster and redirected the command. ` +
    `@opentf/esrun-redis does not follow ${prefix} redirects; connect to the node that owns the key, ` +
    `or use a proxy that presents the cluster as a single server.`
  );
}
