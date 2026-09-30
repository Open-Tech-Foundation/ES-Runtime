/**
 * `@opentf/esrun-mysql` — the MySQL (and MariaDB) driver for `runtime:db`.
 *
 * The driver is now built into the runtime (DECISIONS D147): its protocol runs
 * in Rust, and `import { mysql } from "runtime:db"` is the same driver. This
 * package re-exports it, so code written against the package keeps working.
 *
 * ```js
 * import { connect, sql } from "runtime:db";
 * import { driver } from "@opentf/esrun-mysql";
 *
 * const db = await connect("mysql://user:pass@localhost/app", { driver });
 * ```
 */
export type {
  MySqlConnection,
  MySqlOptions,
  MySqlPooled,
  MySqlPoolOptions,
  MySqlRow,
  MySqlValue,
} from "runtime:db";
export { mysql as driver } from "runtime:db";
