/**
 * `@opentf/esrun-postgres` — the PostgreSQL driver for `runtime:db`.
 *
 * The driver is now built into the runtime (DECISIONS D147): its wire protocol
 * runs in Rust, and `import { postgres } from "runtime:db"` is the same
 * driver. This package re-exports it, so code written against the package
 * keeps working unchanged.
 *
 * ```js
 * import { connect, sql } from "runtime:db";
 * import { driver } from "@opentf/esrun-postgres";
 *
 * const db = await connect("postgres://user:pass@localhost/app", { driver });
 * ```
 */

export type {
  PgConnection,
  PgOptions,
  PgPooled,
  PgPoolOptions,
  PgRow,
  PgServerMessage,
  PgValue,
} from "runtime:db";
export { postgres as driver } from "runtime:db";
