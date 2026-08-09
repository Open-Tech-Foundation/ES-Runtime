/**
 * PostgreSQL says what went wrong in a five-character SQLSTATE, which is a far
 * better thing to map from than a message — it is stable across versions and
 * across locales, where the message is neither.
 */
import { DbErrorCode } from "runtime:db";

/** SQLSTATE → the portable code an application branches on. */
const BY_SQLSTATE: Record<string, string> = {
  "23505": DbErrorCode.UniqueViolation,
  "23503": DbErrorCode.ForeignKeyViolation,
  "23502": DbErrorCode.NotNullViolation,
  "23514": DbErrorCode.CheckViolation,
  "40P01": DbErrorCode.Deadlock,
  "40001": DbErrorCode.SerializationFailure,
  "55P03": DbErrorCode.Busy, // lock_not_available
  "57014": DbErrorCode.Timeout, // query_canceled
  "57P01": DbErrorCode.ConnectionLost, // admin_shutdown
  "57P02": DbErrorCode.ConnectionLost, // crash_shutdown
  "57P03": DbErrorCode.ConnectionLost, // cannot_connect_now
  "28000": DbErrorCode.AuthFailed, // invalid_authorization_specification
  "28P01": DbErrorCode.AuthFailed, // invalid_password
  "42601": DbErrorCode.Syntax,
  "42P01": DbErrorCode.UndefinedTable,
  "42703": DbErrorCode.UndefinedColumn,
  "25006": DbErrorCode.ReadOnly, // read_only_sql_transaction
  "0A000": DbErrorCode.Unsupported, // feature_not_supported
};

/** The fields of an `ErrorResponse` / `NoticeResponse`. */
export interface ServerMessage {
  severity: string;
  code: string;
  message: string;
  detail?: string;
  hint?: string;
  position?: string;
  schema?: string;
  table?: string;
  column?: string;
  constraint?: string;
}

/**
 * The class of a SQLSTATE — its first two characters — for the codes that have
 * no exact entry. `23xxx` is an integrity violation whatever the last three
 * digits say, and answering "some constraint failed" beats answering nothing.
 */
export function portableCode(sqlstate: string): string {
  const exact = BY_SQLSTATE[sqlstate];
  if (exact) return exact;
  switch (sqlstate.slice(0, 2)) {
    case "08":
      return DbErrorCode.ConnectionLost;
    case "28":
      return DbErrorCode.AuthFailed;
    case "40":
      return DbErrorCode.SerializationFailure;
    case "42":
      return DbErrorCode.Syntax;
    default:
      return DbErrorCode.Backend;
  }
}
