/**
 * `@opentf/esrun-mysql` — the MySQL driver for `runtime:db`.
 *
 * The package's export *is* the driver: hand it to `connect`, and what comes
 * back is a `MySqlConnection` with MySQL's own surface on it.
 *
 * ```js
 * import { connect, sql } from "runtime:db";
 * import { driver } from "@opentf/esrun-mysql";
 *
 * const db = await connect("mysql://user:pass@localhost/app", { driver });
 * const rows = await db.query(sql`SELECT * FROM users WHERE id = ${id}`);
 *
 * // A pool is the same call with the same driver.
 * const pool = await connect("mysql://localhost/app", { driver, pool: { max: 20 } });
 * ```
 *
 * There is no native code here. The driver is JavaScript over `runtime:net`,
 * which is the arrangement `runtime:db` exists to make possible: adding a
 * database to this runtime does not mean adding anything to the runtime.
 * MariaDB speaks the same protocol, and the driver takes `mariadb:` URLs too.
 */
import { type Driver, defineDriver, type PoolSettings } from "runtime:db";
import { env, unmask } from "runtime:process";

import { MYSQL_DIALECT, MySqlConnection, type MySqlOptions } from "./connection.js";
import { MySqlPooled, type MySqlPoolOptions } from "./pool.js";

export { MYSQL_DIALECT, MySqlConnection, type MySqlOptions, MySqlPooled, type MySqlPoolOptions };

/**
 * The environment variables the `mysql` client reads: `MYSQL_HOST`,
 * `MYSQL_TCP_PORT` and `MYSQL_PWD`.
 *
 * Below the URL and below explicit options, so they are defaults rather than
 * overrides. Reading them needs the `Env` capability, and a program running
 * without it is not asking for them — so a refusal means no defaults, not an
 * error.
 */
export function environmentDefaults(): MySqlOptions {
  const options: MySqlOptions = {};
  try {
    if (env.MYSQL_HOST) options.host = String(env.MYSQL_HOST);
    if (env.MYSQL_TCP_PORT) options.port = Number(env.MYSQL_TCP_PORT);
    // `unmask` through, always: a masked value must not reach the handshake as
    // the literal text "[secret]".
    if (env.MYSQL_PWD) options.password = String(unmask(env.MYSQL_PWD));
  } catch {
    return {};
  }
  return options;
}

/**
 * Turns a connection string into options.
 *
 * `mysql://user:password@host:port/database?ssl-mode=REQUIRED`, with every
 * part optional. `ssl-mode` takes MySQL's own spellings (`DISABLED`,
 * `PREFERRED`, `REQUIRED`), and `sslmode` the lower-case ones the PostgreSQL
 * driver uses, so a URL written for either reads the same.
 *
 * Precedence, highest first: explicit options, the URL, the environment, then
 * the defaults.
 */
export function parseConnectionString(url: string, overrides: MySqlOptions = {}): MySqlOptions {
  const parsed = new URL(url);
  const options: MySqlOptions = {};
  if (parsed.hostname !== "") options.host = decodeURIComponent(parsed.hostname);
  if (parsed.port !== "") options.port = Number(parsed.port);
  const database = decodeURIComponent(parsed.pathname.replace(/^\//, ""));
  if (database !== "") options.database = database;
  if (parsed.username !== "") options.user = decodeURIComponent(parsed.username);
  if (parsed.password !== "") options.password = decodeURIComponent(parsed.password);
  const mode = (
    parsed.searchParams.get("ssl-mode") ??
    parsed.searchParams.get("sslmode") ??
    ""
  ).toLowerCase();
  if (mode === "disabled" || mode === "disable") options.sslmode = "disable";
  else if (mode === "preferred" || mode === "prefer") options.sslmode = "prefer";
  else if (mode === "required" || mode === "require") options.sslmode = "require";
  // Seconds, as the `mysql` client spells it; the option is milliseconds.
  const connectSeconds =
    parsed.searchParams.get("connect-timeout") ?? parsed.searchParams.get("connect_timeout");
  if (connectSeconds !== null && connectSeconds !== "") {
    const seconds = Number(connectSeconds);
    if (Number.isFinite(seconds) && seconds >= 0) options.connectTimeout = seconds * 1000;
  }
  const retrieval = parsed.searchParams.get("allowPublicKeyRetrieval");
  if (retrieval !== null) options.allowPublicKeyRetrieval = retrieval === "true";
  // A certificate, not a path: reading a file is a capability a connection
  // string should not exercise on the caller's behalf.
  const rootCert = parsed.searchParams.get("ssl-ca") ?? parsed.searchParams.get("sslrootcert");
  if (rootCert !== null && rootCert !== "") options.sslRootCert = rootCert;
  return {
    host: "localhost",
    port: 3306,
    ...environmentDefaults(),
    ...stripUndefined(options),
    ...stripUndefined(overrides),
  };
}

function stripUndefined<T extends object>(value: T): T {
  return Object.fromEntries(Object.entries(value).filter(([, v]) => v !== undefined)) as T;
}

/**
 * The MySQL driver. Pass it to `connect`. It takes `mysql:` and `mariadb:`
 * URLs, and everything a connection string can carry is also accepted as an
 * option, with explicit options winning.
 */
export const driver: Driver<MySqlConnection, MySqlOptions, MySqlPooled> = defineDriver<
  MySqlConnection,
  MySqlOptions,
  MySqlPooled
>({
  name: "mysql",
  schemes: ["mysql", "mariadb"],
  dialect: MYSQL_DIALECT,
  async open(url: string, options: MySqlOptions = {}): Promise<MySqlConnection> {
    const connection = new MySqlConnection();
    await connection.open(parseConnectionString(url, options));
    return connection;
  },
  /** Nothing is opened here: connections are made when first needed. */
  pooled(url: string, options: MySqlOptions = {}, poolOptions: PoolSettings = {}): MySqlPooled {
    return new MySqlPooled(driver, url, parseConnectionString(url, options), poolOptions);
  },
});
