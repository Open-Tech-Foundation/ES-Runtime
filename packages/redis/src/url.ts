/**
 * `redis://` and `rediss://`, as options.
 *
 * The format is the IANA-registered one every Redis client accepts:
 * `redis://[[username][:password]@]host[:port][/db][?option=value]`. Two parts
 * of it are worth naming because they are not obvious. The **path** is a
 * database index rather than a database name — Redis databases are numbered —
 * and an empty username with a password (`redis://:secret@host`) is the
 * pre-ACL spelling, which means the `default` user.
 */
import { DbError, DbErrorCode } from "runtime:db";

import type { RedisOptions } from "./connection.js";

export function parseConnectionString(url: string, overrides: RedisOptions = {}): RedisOptions {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    throw new DbError(`${url} is not a redis: connection string`, {
      code: DbErrorCode.Unsupported,
    });
  }

  const scheme = parsed.protocol.replace(/:$/, "").toLowerCase();
  if (scheme !== "redis" && scheme !== "rediss") {
    throw new DbError(`${scheme}: is not a redis scheme — use redis:// or rediss://`, {
      code: DbErrorCode.Unsupported,
    });
  }

  const options: RedisOptions = {};
  if (parsed.hostname !== "") options.host = decodeURIComponent(parsed.hostname);
  if (parsed.port !== "") options.port = Number(parsed.port);
  // An empty username is how the pre-ACL form spells "the default user", so it
  // is left unset rather than passed through as "".
  if (parsed.username !== "") options.username = decodeURIComponent(parsed.username);
  if (parsed.password !== "") options.password = decodeURIComponent(parsed.password);

  const path = parsed.pathname.replace(/^\//, "");
  if (path !== "") {
    const db = Number(path);
    if (!Number.isInteger(db) || db < 0) {
      throw new DbError(
        `the path of a redis: URL is a database index, and ${JSON.stringify(path)} is not one`,
        { code: DbErrorCode.Unsupported },
      );
    }
    options.db = db;
  }

  const search = parsed.searchParams;
  // `?db=` is the other spelling in the wild, and loses to the path when both
  // are given — the path is the registered form.
  const db = search.get("db");
  if (db !== null && db !== "" && options.db === undefined) options.db = Number(db);

  const connectSeconds = search.get("connect_timeout");
  if (connectSeconds !== null && connectSeconds !== "") {
    // Seconds in the URL, milliseconds in the options object — the same split
    // the PostgreSQL driver has, for the same reason: the connection-string
    // convention is seconds and the rest of JavaScript means milliseconds by a
    // timeout. Two spellings on purpose, rather than one quietly meaning the
    // other.
    const seconds = Number(connectSeconds);
    if (Number.isFinite(seconds) && seconds >= 0) options.connectTimeout = seconds * 1000;
  }

  const clientName = search.get("client_name");
  if (clientName !== null && clientName !== "") options.clientName = clientName;

  const protocol = search.get("protocol");
  if (protocol === "2") options.resp3 = false;
  if (protocol === "3") options.resp3 = true;

  // A password belongs in the userinfo, where every tool already looks for it.
  // Accepting a second place to put one means a credential that a URL-redacting
  // logger does not know to strip.
  if (search.has("password")) {
    throw new DbError(
      "put the password in the URL's userinfo (redis://:password@host) or in the options object, not in a query parameter",
      { code: DbErrorCode.Unsupported },
    );
  }

  return {
    host: "localhost",
    port: 6379,
    tls: scheme === "rediss",
    ...stripUndefined(options),
    ...stripUndefined(overrides),
  };
}

function stripUndefined<T extends object>(value: T): T {
  return Object.fromEntries(Object.entries(value).filter(([, v]) => v !== undefined)) as T;
}
