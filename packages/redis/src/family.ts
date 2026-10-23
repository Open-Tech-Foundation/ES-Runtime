/**
 * Which server a client is for: what it is called, and how its URLs are spelled.
 *
 * Valkey forked Redis and kept the protocol, so one RESP implementation serves
 * both, and what differs between them is only names — the schemes a URL may
 * use, what `connection.backend` reports, what an error calls the server. A
 * family is those names, and every driver in this package is built from one.
 * Redis is the default everywhere, so code that never names a family sees
 * exactly what it saw before families existed.
 */
import { DbError, DbErrorCode, Dialect } from "runtime:db";

export interface ServerFamily {
  /** The backend's name: `connection.backend`, the driver's name, and messages. */
  readonly name: string;
  /**
   * Every scheme a URL may use, in the order errors list them. The first is a
   * plaintext one, and is what URLs built inside the client are spelled with.
   */
  readonly schemes: readonly string[];
  /** The schemes in `schemes` that mean TLS from the first byte. The first builds URLs. */
  readonly tlsSchemes: readonly string[];
  readonly dialect: Dialect;
}

/** Makes a family. The dialect is built here, because its messages name the server. */
export function defineFamily(family: Omit<ServerFamily, "dialect">): ServerFamily {
  const plain = family.schemes.filter((scheme) => !family.tlsSchemes.includes(scheme));
  if (plain[0] !== family.schemes[0] || family.tlsSchemes.length === 0) {
    throw new TypeError(
      "a server family needs a plaintext scheme first and at least one TLS scheme",
    );
  }
  return Object.freeze({ ...family, dialect: respDialect(family.name) });
}

/** The scheme a URL built inside the client uses. */
export function schemeFor(family: ServerFamily, tls: boolean): string {
  return tls ? family.tlsSchemes[0]! : family.schemes[0]!;
}

/**
 * A RESP server's dialect, which is mostly a list of things it is not.
 *
 * `queryText: false` and `queryAst: true` are the two that matter: this backend
 * refuses SQL by name, and takes command arrays. Everything else follows from
 * Redis rather than from a choice made here — there are no placeholders because
 * there is no statement to put them in, and no savepoints because there is
 * nothing to nest.
 */
function respDialect(name: string): Dialect {
  return new Dialect({
    name,
    // A query builder that reaches for this is building SQL for a backend that
    // has none. Failing loudly beats returning `$1` and letting it be
    // concatenated into a string nothing will ever parse.
    placeholder: () => {
      throw new DbError(
        `the ${name} backend has no SQL and no placeholders — pass a command array to queryAst()`,
        { code: DbErrorCode.QueryForm },
      );
    },
    supports: {
      returning: false,
      savepoints: false,
      namedParameters: false,
      queryText: false,
      queryAst: true,
      // Pub/sub, behind `runtime:db`'s portable subscription surface.
      subscriptions: true,
      // MULTI/EXEC is not a transaction in the sense `transaction(fn)` promises.
      // It queues commands and applies them together, but a command that fails at
      // EXEC time does not roll back the ones beside it — so a `transaction()`
      // built on it would commit half a body that threw. Declared absent rather
      // than approximated; see the README.
      transactions: false,
    },
  });
}

/** Redis: `redis://` and `rediss://`, the IANA-registered pair. */
export const REDIS: ServerFamily = defineFamily({
  name: "redis",
  schemes: ["redis", "rediss"],
  tlsSchemes: ["rediss"],
});
