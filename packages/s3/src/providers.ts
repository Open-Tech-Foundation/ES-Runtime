/**
 * Credential providers for the places AWS hands out temporary credentials:
 * an EC2 instance's metadata service, a container's credentials endpoint (ECS
 * task roles, EKS Pod Identity), a web identity token (EKS IRSA, GitHub
 * Actions OIDC), and a role assumed with other credentials.
 *
 * Each returns a function for `new S3Client({ credentials })`. None runs until
 * the client needs credentials, and the client calls it again five minutes
 * before what it returned expires. None is chosen for you: a program names the
 * one its deployment uses, so credentials never come from a place it did not
 * expect.
 *
 * The protocols are AWS's, as the AWS SDK for JavaScript (v3) implements them,
 * with one difference: the instance metadata provider speaks IMDSv2 only and
 * never falls back to IMDSv1, the unauthenticated form that a server-side
 * request forgery can reach.
 */
import { hash } from "runtime:hashing";
import type { S3Credentials } from "./credentials.js";
import { S3Error, S3ErrorCode } from "./errors.js";
import { signHeaders } from "./sigv4.js";
import { childText, parse } from "./xml.js";

type Provider = () => Promise<S3Credentials>;

function fail(message: string, cause?: unknown): S3Error {
  return new S3Error(message, { code: S3ErrorCode.Credentials, cause });
}

async function readEnv(names: string[]): Promise<Record<string, string | undefined>> {
  let env: Record<string, unknown>;
  try {
    env = (await import("runtime:process")).env as Record<string, unknown>;
  } catch {
    return {};
  }
  let unmask: ((value: unknown) => string) | undefined;
  const out: Record<string, string | undefined> = {};
  for (const name of names) {
    const value = env[name];
    if (value === undefined || value === "") continue;
    if (typeof value === "string") {
      out[name] = value;
    } else {
      unmask ??= (await import("runtime:process")).unmask as (value: unknown) => string;
      out[name] = unmask(value);
    }
  }
  return out;
}

async function readFile(path: string, what: string): Promise<string> {
  try {
    const fs = await import("runtime:fs");
    return (await fs.file(path).text()).trim();
  } catch (cause) {
    throw fail(`${what}: cannot read ${path} (needs --allow-read for it)`, cause);
  }
}

const LOOPBACK = (host: string) =>
  host === "localhost" || host === "[::1]" || /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host);

/** A credentials URL is HTTPS, loopback, or one of `allowed`: a token never crosses a network in the clear. */
function checkUrl(value: string, allowed: string[], what: string): URL {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw fail(`${what}: "${value}" is not a URL`);
  }
  if (url.protocol === "https:") return url;
  if (url.protocol === "http:" && (LOOPBACK(url.hostname) || allowed.includes(url.hostname)))
    return url;
  throw fail(`${what}: ${url.origin} must be HTTPS, loopback or ${allowed.join(" / ")}`);
}

async function request(
  url: URL | string,
  init: RequestInit,
  timeout: number,
  what: string,
): Promise<Response> {
  try {
    return await fetch(url, { ...init, redirect: "error", signal: AbortSignal.timeout(timeout) });
  } catch (cause) {
    throw fail(
      `${what}: ${String(url)} did not answer (${(cause as Error)?.message ?? cause})`,
      cause,
    );
  }
}

/** `{ AccessKeyId, SecretAccessKey, Token, Expiration }`, the JSON both metadata services answer with. */
function fromJson(text: string, what: string): S3Credentials {
  let body: Record<string, unknown>;
  try {
    body = JSON.parse(text);
  } catch (cause) {
    throw fail(`${what}: the answer is not JSON`, cause);
  }
  const { AccessKeyId, SecretAccessKey, Token, Expiration } = body;
  if (
    typeof AccessKeyId !== "string" ||
    typeof SecretAccessKey !== "string" ||
    typeof Token !== "string" ||
    typeof Expiration !== "string"
  ) {
    throw fail(`${what}: the answer lacks AccessKeyId, SecretAccessKey, Token or Expiration`);
  }
  const expiration = new Date(Expiration);
  if (Number.isNaN(expiration.getTime()))
    throw fail(`${what}: Expiration "${Expiration}" is not a date`);
  return {
    accessKeyId: AccessKeyId,
    secretAccessKey: SecretAccessKey,
    sessionToken: Token,
    expiration,
  };
}

async function retrying<T>(attempts: number, run: () => Promise<T>): Promise<T> {
  let last: unknown;
  for (let attempt = 0; attempt < attempts; attempt++) {
    try {
      return await run();
    } catch (error) {
      last = error;
      if ((error as { retry?: boolean }).retry === false) break;
      if (attempt + 1 < attempts) await new Promise((r) => setTimeout(r, 100 * 2 ** attempt));
    }
  }
  throw last;
}

function noRetry(error: S3Error): S3Error {
  return Object.assign(error, { retry: false });
}

export interface InstanceMetadataOptions {
  /** Default `http://169.254.169.254`; `http://[fd00:ec2::254]` on an IPv6-only instance. */
  endpoint?: string;
  /** Milliseconds per request. Default 1000. */
  timeout?: number;
}

/**
 * Credentials of the IAM role attached to this EC2 instance, from the instance
 * metadata service, over IMDSv2. Needs `--allow-net=169.254.169.254`.
 */
export function instanceMetadata(options: InstanceMetadataOptions = {}): Provider {
  const what = "instance metadata";
  const base = checkUrl(
    options.endpoint ?? "http://169.254.169.254",
    ["169.254.169.254", "[fd00:ec2::254]"],
    what,
  );
  const timeout = options.timeout ?? 1000;
  const at = (path: string) => new URL(path, base);
  return () =>
    retrying(3, async () => {
      const tokenResponse = await request(
        at("/latest/api/token"),
        { method: "PUT", headers: { "x-aws-ec2-metadata-token-ttl-seconds": "21600" } },
        timeout,
        what,
      );
      if (!tokenResponse.ok) {
        // No IMDSv1 fallback: an instance that refuses the token has v2 required
        // off, or a hop limit the container cannot cross; either way, say so.
        throw fail(`${what}: the IMDSv2 token request was refused (${tokenResponse.status})`);
      }
      const token = await tokenResponse.text();
      const headers = { "x-aws-ec2-metadata-token": token };
      const roles = await request(
        at("/latest/meta-data/iam/security-credentials/"),
        { headers },
        timeout,
        what,
      );
      if (roles.status === 404)
        throw noRetry(fail(`${what}: no IAM role is attached to this instance`));
      if (!roles.ok) throw fail(`${what}: listing the role failed (${roles.status})`);
      const role = (await roles.text()).split("\n")[0]?.trim();
      if (!role) throw noRetry(fail(`${what}: no IAM role is attached to this instance`));
      const credentials = await request(
        at(`/latest/meta-data/iam/security-credentials/${encodeURIComponent(role)}`),
        { headers },
        timeout,
        what,
      );
      if (!credentials.ok)
        throw fail(`${what}: reading role ${role} failed (${credentials.status})`);
      return fromJson(await credentials.text(), what);
    });
}

export interface ContainerCredentialsOptions {
  /** The full URL. Default `AWS_CONTAINER_CREDENTIALS_RELATIVE_URI` (on `169.254.170.2`), then `AWS_CONTAINER_CREDENTIALS_FULL_URI`. */
  uri?: string;
  /** The `Authorization` value. Default `AWS_CONTAINER_AUTHORIZATION_TOKEN`. */
  token?: string;
  /** A file holding it, read on every refresh. Default `AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE`; wins over `token`. */
  tokenFile?: string;
  timeout?: number;
}

/**
 * Credentials from a container's credentials endpoint: an ECS task role, or
 * EKS Pod Identity. Reads AWS's variables unless told otherwise, so it needs
 * `--allow-env` for them, `--allow-net` for the endpoint
 * (`169.254.170.2` on ECS, `169.254.170.23` on EKS), and `--allow-read` for a
 * token file.
 */
export function containerCredentials(options: ContainerCredentialsOptions = {}): Provider {
  const what = "container credentials";
  return async () => {
    const env = await readEnv([
      "AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
      "AWS_CONTAINER_CREDENTIALS_FULL_URI",
      "AWS_CONTAINER_AUTHORIZATION_TOKEN",
      "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
    ]);
    const relative = env.AWS_CONTAINER_CREDENTIALS_RELATIVE_URI;
    const uri =
      options.uri ??
      (relative !== undefined
        ? `http://169.254.170.2${relative}`
        : env.AWS_CONTAINER_CREDENTIALS_FULL_URI);
    if (uri === undefined) {
      throw fail(
        `${what}: no endpoint — AWS_CONTAINER_CREDENTIALS_RELATIVE_URI or _FULL_URI is unset or not granted`,
      );
    }
    const url = checkUrl(uri, ["169.254.170.2", "169.254.170.23", "[fd00:ec2::23]"], what);
    const tokenFile = options.tokenFile ?? env.AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE;
    const fixed = options.token ?? env.AWS_CONTAINER_AUTHORIZATION_TOKEN;
    return retrying(3, async () => {
      const token = tokenFile !== undefined ? await readFile(tokenFile, what) : fixed;
      if (token !== undefined && /[\r\n]/.test(token))
        throw noRetry(fail(`${what}: the token holds a line break`));
      const response = await request(
        url,
        token === undefined ? {} : { headers: { authorization: token } },
        options.timeout ?? 1000,
        what,
      );
      const text = await response.text();
      if (!response.ok) {
        const error = fail(`${what}: ${url.origin} answered ${response.status}`);
        throw response.status < 500 ? noRetry(error) : error;
      }
      return fromJson(text, what);
    });
  };
}

interface StsOptions {
  /** STS's region; default `AWS_REGION`, else `us-east-1`. The regional endpoint is used. */
  region?: string;
  /** An STS endpoint of your own (a VPC endpoint). HTTPS, or loopback. */
  endpoint?: string;
  roleSessionName?: string;
  /** 900 to 43200 seconds; STS's default is 3600. */
  durationSeconds?: number;
  timeout?: number;
}

const SESSION_NAME = /^[\w+=,.@-]{2,64}$/;

function stsParams(action: string, options: StsOptions, roleArn: string): Array<[string, string]> {
  const name = options.roleSessionName ?? `esrun-s3-${Date.now()}`;
  if (!SESSION_NAME.test(name))
    throw fail(`roleSessionName "${name}" is not 2–64 of [A-Za-z0-9_+=,.@-]`);
  const params: Array<[string, string]> = [
    ["Action", action],
    ["Version", "2011-06-15"],
    ["RoleArn", roleArn],
    ["RoleSessionName", name],
  ];
  if (options.durationSeconds !== undefined) {
    const d = options.durationSeconds;
    if (!Number.isInteger(d) || d < 900 || d > 43_200)
      throw fail("durationSeconds is 900 to 43200");
    params.push(["DurationSeconds", String(d)]);
  }
  return params;
}

async function stsUrl(options: StsOptions): Promise<{ url: URL; region: string }> {
  const region = options.region ?? (await readEnv(["AWS_REGION"])).AWS_REGION ?? "us-east-1";
  if (!/^[a-z0-9-]{1,64}$/.test(region)) throw fail(`region "${region}" is not a region name`);
  const url = checkUrl(options.endpoint ?? `https://sts.${region}.amazonaws.com/`, [], "STS");
  return { url, region };
}

async function callSts(
  url: URL,
  body: string,
  headers: Record<string, string>,
  timeout: number,
  action: string,
): Promise<S3Credentials> {
  return retrying(3, async () => {
    const response = await request(url, { method: "POST", headers, body }, timeout, action);
    const text = await response.text();
    if (!response.ok) {
      let code = `${response.status}`;
      let message = "";
      try {
        const error = parse(text, action).ErrorResponse;
        const inner = (error as Record<string, unknown> | undefined)?.Error;
        code = childText(inner, "Code") ?? code;
        message = childText(inner, "Message") ?? "";
      } catch {
        // not XML: the status is all there is
      }
      const error = fail(`${action}: ${code}${message ? `: ${message}` : ""}`);
      const transient =
        response.status >= 500 || code === "IDPCommunicationError" || code === "Throttling";
      throw transient ? error : noRetry(error);
    }
    const root = parse(text, action)[`${action}Response`];
    const credentials = (root as Record<string, unknown> | undefined)?.[`${action}Result`];
    const c = (credentials as Record<string, unknown> | undefined)?.Credentials;
    const accessKeyId = childText(c, "AccessKeyId");
    const secretAccessKey = childText(c, "SecretAccessKey");
    const sessionToken = childText(c, "SessionToken");
    const expiration = new Date(childText(c, "Expiration") ?? "");
    if (!accessKeyId || !secretAccessKey || !sessionToken || Number.isNaN(expiration.getTime())) {
      throw noRetry(fail(`${action}: the answer has no credentials`));
    }
    return { accessKeyId, secretAccessKey, sessionToken, expiration };
  });
}

const form = (params: Array<[string, string]>) => new URLSearchParams(params).toString();

export interface WebIdentityOptions extends StsOptions {
  /** Default `AWS_ROLE_ARN`. */
  roleArn?: string;
  /** A file holding the token, read on every refresh. Default `AWS_WEB_IDENTITY_TOKEN_FILE`. */
  tokenFile?: string;
  /** The token itself, or a function that returns a fresh one. */
  token?: string | (() => string | Promise<string>);
}

/**
 * Credentials for a role, in exchange for an OpenID Connect token: EKS IAM
 * roles for service accounts, GitHub Actions, any OIDC provider the role
 * trusts. Calls STS `AssumeRoleWithWebIdentity`, which takes no AWS
 * credentials. Reads `AWS_ROLE_ARN`, `AWS_WEB_IDENTITY_TOKEN_FILE`,
 * `AWS_ROLE_SESSION_NAME` and `AWS_REGION` unless told otherwise.
 */
export function webIdentity(options: WebIdentityOptions = {}): Provider {
  const action = "AssumeRoleWithWebIdentity";
  return async () => {
    const env = await readEnv([
      "AWS_ROLE_ARN",
      "AWS_WEB_IDENTITY_TOKEN_FILE",
      "AWS_ROLE_SESSION_NAME",
    ]);
    const roleArn = options.roleArn ?? env.AWS_ROLE_ARN;
    if (!roleArn) throw fail(`${action}: no roleArn, and AWS_ROLE_ARN is unset or not granted`);
    let token: string | undefined;
    if (typeof options.token === "function") token = await options.token();
    else if (options.token !== undefined) token = options.token;
    else {
      const file = options.tokenFile ?? env.AWS_WEB_IDENTITY_TOKEN_FILE;
      if (!file)
        throw fail(`${action}: no token, and AWS_WEB_IDENTITY_TOKEN_FILE is unset or not granted`);
      token = await readFile(file, action);
    }
    if (!token) throw fail(`${action}: the token is empty`);
    const sessionName = options.roleSessionName ?? env.AWS_ROLE_SESSION_NAME;
    const resolved =
      sessionName === undefined ? options : { ...options, roleSessionName: sessionName };
    const params = stsParams(action, resolved, roleArn);
    params.push(["WebIdentityToken", token]);
    const { url } = await stsUrl(options);
    return callSts(
      url,
      form(params),
      { "content-type": "application/x-www-form-urlencoded; charset=utf-8" },
      options.timeout ?? 5000,
      action,
    );
  };
}

export interface AssumeRoleOptions extends StsOptions {
  roleArn: string;
  /** The credentials that may assume the role: given, or a provider. */
  credentials: S3Credentials | (() => S3Credentials | Promise<S3Credentials>);
  /** For a role that requires one (a third party's account). */
  externalId?: string;
}

/** Credentials for `roleArn`, assumed with other credentials (STS `AssumeRole`, signed). */
export function assumeRole(options: AssumeRoleOptions): Provider {
  const action = "AssumeRole";
  return async () => {
    const source =
      typeof options.credentials === "function" ? await options.credentials() : options.credentials;
    const params = stsParams(action, options, options.roleArn);
    if (options.externalId !== undefined) params.push(["ExternalId", options.externalId]);
    const body = form(params);
    const { url, region } = await stsUrl(options);
    const { CredentialCache } = await import("./credentials.js");
    const resolved = await new CredentialCache(() => source).get();
    const contentType = "application/x-www-form-urlencoded; charset=utf-8";
    const signed = signHeaders(
      {
        method: "POST",
        canonicalUri: url.pathname,
        query: [],
        headers: [
          ["host", url.host],
          ["content-type", contentType],
        ],
      },
      resolved,
      { region, service: "sts", date: new Date() },
      { payloadHash: hash("sha256", body, "hex"), contentSha256Header: false },
    );
    const headers: Record<string, string> = { "content-type": contentType };
    for (const [name, value] of signed.headers) headers[name] = value;
    return callSts(url, body, headers, options.timeout ?? 5000, action);
  };
}
