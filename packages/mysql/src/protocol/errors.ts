/**
 * MySQL says what went wrong with a numeric error code and a SQLSTATE. The
 * code is the precise one — `1062` is a duplicate key and nothing else — so it
 * is mapped first, and the SQLSTATE class answers for the codes that have no
 * entry.
 */
import { DbErrorCode } from "runtime:db";

/** Server error code → the portable code an application branches on. */
const BY_CODE: Record<number, string> = {
  1062: DbErrorCode.UniqueViolation, // ER_DUP_ENTRY
  1586: DbErrorCode.UniqueViolation, // ER_DUP_ENTRY_WITH_KEY_NAME
  1451: DbErrorCode.ForeignKeyViolation, // ER_ROW_IS_REFERENCED_2
  1452: DbErrorCode.ForeignKeyViolation, // ER_NO_REFERENCED_ROW_2
  1216: DbErrorCode.ForeignKeyViolation, // ER_NO_REFERENCED_ROW
  1217: DbErrorCode.ForeignKeyViolation, // ER_ROW_IS_REFERENCED
  1048: DbErrorCode.NotNullViolation, // ER_BAD_NULL_ERROR
  1364: DbErrorCode.NotNullViolation, // ER_NO_DEFAULT_FOR_FIELD
  3819: DbErrorCode.CheckViolation, // ER_CHECK_CONSTRAINT_VIOLATED
  4025: DbErrorCode.CheckViolation, // MariaDB's ER_CONSTRAINT_FAILED
  1213: DbErrorCode.Deadlock, // ER_LOCK_DEADLOCK
  1205: DbErrorCode.Busy, // ER_LOCK_WAIT_TIMEOUT
  3572: DbErrorCode.Busy, // ER_LOCK_NOWAIT
  1317: DbErrorCode.Timeout, // ER_QUERY_INTERRUPTED — what KILL QUERY answers
  3024: DbErrorCode.Timeout, // ER_QUERY_TIMEOUT (max_execution_time)
  1969: DbErrorCode.Timeout, // MariaDB's ER_STATEMENT_TIMEOUT
  1045: DbErrorCode.AuthFailed, // ER_ACCESS_DENIED_ERROR
  1044: DbErrorCode.AuthFailed, // ER_DBACCESS_DENIED_ERROR
  1251: DbErrorCode.AuthFailed, // ER_NOT_SUPPORTED_AUTH_MODE
  1064: DbErrorCode.Syntax, // ER_PARSE_ERROR
  1146: DbErrorCode.UndefinedTable, // ER_NO_SUCH_TABLE
  1051: DbErrorCode.UndefinedTable, // ER_BAD_TABLE_ERROR
  1054: DbErrorCode.UndefinedColumn, // ER_BAD_FIELD_ERROR
  1290: DbErrorCode.ReadOnly, // ER_OPTION_PREVENTS_STATEMENT (--read-only)
  1792: DbErrorCode.ReadOnly, // ER_CANT_EXECUTE_IN_READ_ONLY_TRANSACTION
  1040: DbErrorCode.Throttled, // ER_CON_COUNT_ERROR
  1203: DbErrorCode.Throttled, // ER_TOO_MANY_USER_CONNECTIONS
  1226: DbErrorCode.Throttled, // ER_USER_LIMIT_REACHED
  1295: DbErrorCode.Unsupported, // ER_UNSUPPORTED_PS
  1235: DbErrorCode.Unsupported, // ER_NOT_SUPPORTED_YET
  1053: DbErrorCode.ConnectionLost, // ER_SERVER_SHUTDOWN
};

/** The fields of an `ERR` packet. */
export interface ServerError {
  code: number;
  sqlstate: string;
  message: string;
}

/**
 * The portable code for a server error: the exact code where there is one, and
 * the SQLSTATE's class where there is not — `23xxx` is an integrity violation
 * whatever the rest says.
 */
export function portableCode(error: ServerError): string {
  const exact = BY_CODE[error.code];
  if (exact !== undefined) return exact;
  switch (error.sqlstate.slice(0, 2)) {
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
