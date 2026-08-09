/**
 * `@opentf/esrun-postgres` — a PostgreSQL backend for `runtime:db`.
 *
 * Importing this package registers the `postgres:` and `postgresql:` schemes.
 * Nothing else is required:
 *
 * ```js
 * import "@opentf/esrun-postgres";
 * import { connect, sql } from "runtime:db";
 *
 * const db = await connect("postgres://user:pass@localhost/app");
 * ```
 *
 * There is no native code here. The driver is JavaScript over `runtime:net`,
 * which is the arrangement `runtime:db` exists to make possible: adding a
 * database to this runtime does not mean adding anything to the runtime.
 */
import { registerBackend } from "runtime:db";

import { PgConnection, POSTGRES_DIALECT, type PgOptions } from "./connection.js";

export { PgConnection, POSTGRES_DIALECT, type PgOptions };

/**
 * Turns a connection string into options.
 *
 * `postgres://user:password@host:port/database?sslmode=require`, with every
 * part optional. A password in the URL is honoured because libpq's format has
 * always allowed it and every tool emits it — unlike an encryption key, it is
 * the credential the URL exists to carry.
 */
export function parseConnectionString(url: string, overrides: PgOptions = {}): PgOptions {
  const parsed = new URL(url);
  const options: PgOptions = {
    host: decodeURIComponent(parsed.hostname || "localhost"),
    port: parsed.port === "" ? 5432 : Number(parsed.port),
  };
  const database = decodeURIComponent(parsed.pathname.replace(/^\//, ""));
  if (database !== "") options.database = database;
  if (parsed.username !== "") options.user = decodeURIComponent(parsed.username);
  if (parsed.password !== "") options.password = decodeURIComponent(parsed.password);
  const sslmode = parsed.searchParams.get("sslmode");
  if (sslmode === "require" || sslmode === "prefer" || sslmode === "disable") {
    options.sslmode = sslmode;
  }
  const application = parsed.searchParams.get("application_name");
  if (application !== null) options.applicationName = application;
  return { ...stripUndefined(options), ...stripUndefined(overrides) };
}

function stripUndefined<T extends object>(value: T): T {
  return Object.fromEntries(
    Object.entries(value).filter(([, v]) => v !== undefined),
  ) as T;
}

/** Opens a connection without going through `runtime:db`'s registry. */
export async function connect(url: string, options: PgOptions = {}): Promise<PgConnection> {
  const connection = new PgConnection();
  await connection.open(parseConnectionString(url, options));
  return connection;
}

for (const scheme of ["postgres", "postgresql"]) {
  registerBackend(scheme, (url, options) => connect(url, options as PgOptions));
}
