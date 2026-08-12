/**
 * `@opentf/esrun-postgres` — the PostgreSQL driver for `runtime:db`.
 *
 * The package's export *is* the driver. Nothing is registered by importing it,
 * because nothing needs to be: you hand the driver to `connect`, and what comes
 * back is a `PgConnection` with PostgreSQL's own surface on it.
 *
 * ```js
 * import { connect, sql } from "runtime:db";
 * import { driver } from "@opentf/esrun-postgres";
 *
 * const db = await connect("postgres://user:pass@localhost/app", { driver });
 * const rows = await db.query(sql`SELECT * FROM users WHERE id = ${id}`);
 *
 * // A pool is the same call with the same driver.
 * const pool = await connect("postgres://localhost/app", { driver, pool: { max: 20 } });
 * ```
 *
 * Every driver package exports its driver under the name `driver`, and nothing
 * as a default — so the import is the same shape whichever backend it is, and
 * `{ driver }` is the whole of the option. Two drivers in one module are
 * `import { driver as postgres }`.
 *
 * There is no native code here. The driver is JavaScript over `runtime:net`,
 * which is the arrangement `runtime:db` exists to make possible: adding a
 * database to this runtime does not mean adding anything to the runtime.
 */
import { defineDriver, type Driver, type PoolSettings } from "runtime:db";
import { env, unmask } from "runtime:process";

import { PgConnection, POSTGRES_DIALECT, type PgOptions } from "./connection.js";
import { PgPooled, type PgPoolOptions } from "./pool.js";

export { PgConnection, PgPooled, POSTGRES_DIALECT, type PgOptions, type PgPoolOptions };

/**
 * The `PG*` environment variables, which every libpq tool reads.
 *
 * Below the URL and below explicit options, so they are defaults rather than
 * overrides — `psql` behaves the same way, and a program that spelled out a
 * host should get that host whatever the shell exported.
 *
 * **Reading the environment needs the `Env` capability**, and a program running
 * without it is not asking for libpq's defaults. So a refusal here is not an
 * error: it means no defaults, and the connection string stands on its own.
 */
export function environmentDefaults(): PgOptions {
  const options: PgOptions = {};
  try {
    if (env.PGHOST) options.host = String(env.PGHOST);
    if (env.PGPORT) options.port = Number(env.PGPORT);
    if (env.PGUSER) options.user = String(env.PGUSER);
    if (env.PGDATABASE) options.database = String(env.PGDATABASE);
    if (env.PGAPPNAME) options.applicationName = String(env.PGAPPNAME);
    if (env.PGPASSWORD) {
      // `unmask` through, always. It returns a plain string unchanged, so this
      // is correct whether or not the runtime decides PGPASSWORD is a name
      // worth masking — and being wrong about that would put "[secret]" in a
      // startup packet.
      options.password = String(unmask(env.PGPASSWORD));
    }
    const sslmode = env.PGSSLMODE ? String(env.PGSSLMODE) : "";
    if (sslmode === "require" || sslmode === "prefer" || sslmode === "disable") {
      options.sslmode = sslmode;
    }
    if (env.PGCONNECT_TIMEOUT) {
      // libpq's spelling: seconds.
      const seconds = Number(env.PGCONNECT_TIMEOUT);
      if (Number.isFinite(seconds) && seconds >= 0) options.connectTimeout = seconds * 1000;
    }
  } catch {
    // The `Env` capability was not granted. Not an error, and not a reason to
    // fail a connection that named everything it needed.
    return {};
  }
  return options;
}

/**
 * Turns a connection string into options.
 *
 * `postgres://user:password@host:port/database?sslmode=require`, with every
 * part optional. A password in the URL is honoured because libpq's format has
 * always allowed it and every tool emits it — unlike an encryption key, it is
 * the credential the URL exists to carry.
 *
 * Precedence, highest first: explicit options, the URL, the `PG*` environment,
 * then the defaults. Only what the URL actually carried counts as the URL
 * having said anything — `postgres://` names no host, so `PGHOST` still
 * applies.
 */
export function parseConnectionString(url: string, overrides: PgOptions = {}): PgOptions {
  const parsed = new URL(url);
  const options: PgOptions = {};
  if (parsed.hostname !== "") options.host = decodeURIComponent(parsed.hostname);
  if (parsed.port !== "") options.port = Number(parsed.port);
  const database = decodeURIComponent(parsed.pathname.replace(/^\//, ""));
  if (database !== "") options.database = database;
  if (parsed.username !== "") options.user = decodeURIComponent(parsed.username);
  if (parsed.password !== "") options.password = decodeURIComponent(parsed.password);
  // libpq spells these in **seconds**, and every connection string in the wild
  // follows it. The options object stays in milliseconds, which is what the
  // rest of JavaScript means by a timeout — so the two spellings differ on
  // purpose and are documented as differing, rather than one silently meaning
  // the other.
  const connectSeconds = parsed.searchParams.get("connect_timeout");
  if (connectSeconds !== null && connectSeconds !== "") {
    const seconds = Number(connectSeconds);
    if (Number.isFinite(seconds) && seconds >= 0) options.connectTimeout = seconds * 1000;
  }
  const statementMs = parsed.searchParams.get("statement_timeout");
  if (statementMs !== null && statementMs !== "") {
    const ms = Number(statementMs);
    if (Number.isFinite(ms) && ms >= 0) options.statementTimeout = ms;
  }
  // libpq's `sslrootcert` names a *file*; this takes the certificate itself,
  // because reading a file is a capability a connection string should not
  // exercise on the caller's behalf. Read it yourself and pass it in.
  const rootCert = parsed.searchParams.get("sslrootcert");
  if (rootCert !== null && rootCert !== "") options.sslRootCert = rootCert;
  const sslmode = parsed.searchParams.get("sslmode");
  if (sslmode === "require" || sslmode === "prefer" || sslmode === "disable") {
    options.sslmode = sslmode;
  }
  const application = parsed.searchParams.get("application_name");
  if (application !== null) options.applicationName = application;
  return {
    host: "localhost",
    port: 5432,
    ...environmentDefaults(),
    ...stripUndefined(options),
    ...stripUndefined(overrides),
  };
}

function stripUndefined<T extends object>(value: T): T {
  return Object.fromEntries(
    Object.entries(value).filter(([, v]) => v !== undefined),
  ) as T;
}

/**
 * The PostgreSQL driver.
 *
 * Pass it to `connect`. It takes `postgres:` and `postgresql:` URLs — both
 * spellings are in the wild and neither is more correct than the other — and
 * everything a connection string can carry is also accepted as an option, with
 * explicit options winning over the URL and the URL winning over `PG*`.
 */
export const driver: Driver<PgConnection, PgOptions, PgPooled> = defineDriver<
  PgConnection,
  PgOptions,
  PgPooled
>({
  name: "postgres",
  schemes: ["postgres", "postgresql"],
  dialect: POSTGRES_DIALECT,
  async open(url: string, options: PgOptions = {}): Promise<PgConnection> {
    const connection = new PgConnection();
    await connection.open(parseConnectionString(url, options));
    return connection;
  },
  /**
   * Nothing is opened here: connections are made when they are first needed, so
   * a pool costs nothing until something asks it for work.
   */
  pooled(url: string, options: PgOptions = {}, poolOptions: PoolSettings = {}): PgPooled {
    // Parsed once, up front, so a malformed connection string fails where it
    // was written rather than at whichever call happens to open the first
    // connection.
    return new PgPooled(driver, url, parseConnectionString(url, options), poolOptions);
  },
});
