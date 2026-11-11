/**
 * Credentials: given, or fetched by a provider and cached until they near
 * their expiry.
 *
 * A value may be a `Secret` from `runtime:process`'s `env` — that is what
 * `env.AWS_SECRET_ACCESS_KEY` is, since the name ends in `_KEY` — and is
 * unmasked only at signing time. `runtime:process` is imported lazily: loading
 * it needs the `env` capability, and a program that passes plain strings should
 * not need a grant it never uses.
 */
import { S3Error, S3ErrorCode } from "./errors.js";

/** A string, or a masked `Secret` from `runtime:process`'s `env`. */
export type SecretValue = string | { toString(): string };

export interface S3Credentials {
  accessKeyId: SecretValue;
  secretAccessKey: SecretValue;
  sessionToken?: SecretValue | undefined;
  /** When temporary credentials stop working; they are refreshed five minutes before. */
  expiration?: Date | undefined;
}

/** Called for credentials when none are cached or the cached ones are about to expire. */
export type CredentialsProvider = () => S3Credentials | Promise<S3Credentials>;

export interface ResolvedCredentials {
  accessKeyId: string;
  secretAccessKey: string;
  sessionToken: string | undefined;
  expiration: Date | undefined;
}

/** Refresh this long before `expiration`, so a request signed now is not refused in flight. */
const REFRESH_MARGIN_MS = 5 * 60 * 1000;

let unmaskFn: ((value: unknown) => string) | undefined;

async function reveal(value: SecretValue | undefined, name: string): Promise<string | undefined> {
  if (value === undefined || value === null) return undefined;
  if (typeof value === "string") return value;
  if (unmaskFn === undefined) {
    try {
      unmaskFn = (await import("runtime:process")).unmask as (value: unknown) => string;
    } catch (cause) {
      throw new S3Error(`${name} is not a string, and runtime:process could not unmask it`, {
        code: S3ErrorCode.Credentials,
        cause,
      });
    }
  }
  const out = unmaskFn(value);
  if (typeof out !== "string") {
    throw new S3Error(`${name} must be a string or a Secret`, { code: S3ErrorCode.Credentials });
  }
  return out;
}

async function resolve(credentials: S3Credentials): Promise<ResolvedCredentials> {
  if (credentials === null || typeof credentials !== "object") {
    throw new S3Error("the credentials provider returned no credentials", {
      code: S3ErrorCode.Credentials,
    });
  }
  const accessKeyId = await reveal(credentials.accessKeyId, "accessKeyId");
  const secretAccessKey = await reveal(credentials.secretAccessKey, "secretAccessKey");
  if (!accessKeyId || !secretAccessKey) {
    throw new S3Error("credentials need both accessKeyId and secretAccessKey", {
      code: S3ErrorCode.Credentials,
    });
  }
  const expiration = credentials.expiration;
  if (
    expiration !== undefined &&
    !(expiration instanceof Date && !Number.isNaN(expiration.getTime()))
  ) {
    throw new S3Error("credentials.expiration must be a valid Date", {
      code: S3ErrorCode.Credentials,
    });
  }
  return {
    accessKeyId,
    secretAccessKey,
    sessionToken: (await reveal(credentials.sessionToken, "sessionToken")) || undefined,
    expiration,
  };
}

/**
 * Hands out credentials, asking the provider at most once at a time and only
 * when the cached ones are missing or within five minutes of expiring.
 */
export class CredentialCache {
  readonly #provider: CredentialsProvider;
  #cached: ResolvedCredentials | undefined;
  #pending: Promise<ResolvedCredentials> | undefined;

  constructor(provider: CredentialsProvider) {
    this.#provider = provider;
  }

  get(now: number = Date.now()): Promise<ResolvedCredentials> {
    const cached = this.#cached;
    if (
      cached &&
      (cached.expiration === undefined || cached.expiration.getTime() - now > REFRESH_MARGIN_MS)
    ) {
      return Promise.resolve(cached);
    }
    this.#pending ??= (async () => {
      try {
        let given: S3Credentials;
        try {
          given = await this.#provider();
        } catch (cause) {
          if (cause instanceof S3Error) throw cause;
          throw new S3Error(
            `the credentials provider failed: ${(cause as Error)?.message ?? cause}`,
            {
              code: S3ErrorCode.Credentials,
              cause,
            },
          );
        }
        const fresh = await resolve(given);
        this.#cached = fresh;
        return fresh;
      } finally {
        this.#pending = undefined;
      }
    })();
    return this.#pending;
  }

  /** Forget the cached credentials, so the next request asks the provider (after `ExpiredToken`). */
  invalidate(): void {
    this.#cached = undefined;
  }
}
