// The credential providers against fakes of the services they call: the EC2
// instance metadata service, a container credentials endpoint, and STS.

import { hash } from "runtime:hashing";
import { serve } from "runtime:http";
import {
  assumeRole,
  containerCredentials,
  instanceMetadata,
  webIdentity,
} from "../../dist/index.js";
import { signHeaders } from "../../dist/sigv4.js";
import { is, ok, report } from "./assert.mjs";

const here = new URL(".", import.meta.url).pathname;
const EXPIRES = new Date(Date.now() + 3600_000).toISOString();
const creds = (id) =>
  JSON.stringify({
    Code: "Success",
    AccessKeyId: id,
    SecretAccessKey: `${id}-secret`,
    Token: `${id}-token`,
    Expiration: EXPIRES,
  });

async function rejects(fn, includes, what) {
  try {
    await fn();
    ok(false, `${what} — nothing was thrown`);
  } catch (error) {
    is(error?.code, "ERR_S3_CREDENTIALS", what);
    ok(
      String(error?.message).includes(includes),
      `${what}: "${error?.message}" mentions ${includes}`,
    );
  }
}

const seen = [];
let imdsMode = "ok";
let stsAnswer = null;
const server = serve({ port: 0, hostname: "127.0.0.1" }, async (request) => {
  const url = new URL(request.url);
  const body = await request.text();
  seen.push({
    method: request.method,
    path: url.pathname,
    headers: Object.fromEntries(request.headers),
    body,
  });
  // -- IMDS ------------------------------------------------------------------
  if (url.pathname === "/latest/api/token") {
    if (imdsMode === "v1-only") return new Response("", { status: 403 });
    if (
      request.method !== "PUT" ||
      request.headers.get("x-aws-ec2-metadata-token-ttl-seconds") !== "21600"
    ) {
      return new Response("", { status: 400 });
    }
    return new Response("imds-token");
  }
  if (url.pathname.startsWith("/latest/meta-data/iam/security-credentials/")) {
    if (request.headers.get("x-aws-ec2-metadata-token") !== "imds-token")
      return new Response("", { status: 401 });
    const role = url.pathname.slice("/latest/meta-data/iam/security-credentials/".length);
    if (imdsMode === "no-role") return new Response("", { status: 404 });
    if (role === "") return new Response("web-role\n");
    return role === "web-role" ? new Response(creds("IMDS")) : new Response("", { status: 404 });
  }
  // -- container -------------------------------------------------------------
  if (url.pathname === "/creds") {
    const auth = request.headers.get("authorization");
    if (auth !== "container-token-from-file" && auth !== "fixed-token")
      return new Response("{}", { status: 403 });
    return new Response(creds(auth === "fixed-token" ? "FIXED" : "FILE"));
  }
  if (url.pathname === "/creds-bad") return new Response(JSON.stringify({ AccessKeyId: "x" }));
  // -- STS -------------------------------------------------------------------
  if (url.pathname === "/" && request.method === "POST") {
    if (stsAnswer) {
      const answer = stsAnswer;
      stsAnswer = null;
      return answer;
    }
    const form = new URLSearchParams(body);
    const action = form.get("Action");
    if (action === "AssumeRole") {
      // Re-derive the signature as STS would.
      const auth = request.headers.get("authorization") ?? "";
      const m =
        /Credential=([^/]+)\/\d{8}\/([^/]+)\/sts\/aws4_request, SignedHeaders=([^,]+), Signature=(\w+)/.exec(
          auth,
        );
      if (!m)
        return new Response(
          "<ErrorResponse><Error><Code>MissingAuthenticationToken</Code></Error></ErrorResponse>",
          { status: 403 },
        );
      const d = request.headers.get("x-amz-date");
      const date = new Date(
        `${d.slice(0, 4)}-${d.slice(4, 6)}-${d.slice(6, 8)}T${d.slice(9, 11)}:${d.slice(11, 13)}:${d.slice(13, 15)}Z`,
      );
      const headers = m[3]
        .split(";")
        .filter((n) => n !== "x-amz-date" && n !== "x-amz-security-token")
        .map((n) => [n, request.headers.get(n)]);
      const expect = signHeaders(
        { method: "POST", canonicalUri: "/", query: [], headers },
        {
          accessKeyId: "BASE",
          secretAccessKey: "base-secret",
          sessionToken: request.headers.get("x-amz-security-token") ?? undefined,
        },
        { region: m[2], service: "sts", date },
        { payloadHash: hash("sha256", body, "hex"), contentSha256Header: false },
      );
      if (m[1] !== "BASE" || expect.signature !== m[4]) {
        return new Response(
          "<ErrorResponse><Error><Code>SignatureDoesNotMatch</Code><Message>bad</Message></Error></ErrorResponse>",
          { status: 403 },
        );
      }
    } else if (action === "AssumeRoleWithWebIdentity") {
      if (request.headers.has("authorization")) return new Response("signed?", { status: 400 });
      if (!form.get("WebIdentityToken")) return new Response("", { status: 400 });
    } else {
      return new Response("", { status: 400 });
    }
    return new Response(
      `<${action}Response xmlns="https://sts.amazonaws.com/doc/2011-06-15/"><${action}Result><Credentials><SessionToken>sts-token</SessionToken><SecretAccessKey>sts-secret</SecretAccessKey><Expiration>${EXPIRES}</Expiration><AccessKeyId>STS-${form.get("RoleSessionName")}</AccessKeyId></Credentials></${action}Result></${action}Response>`,
      { headers: { "content-type": "text/xml" } },
    );
  }
  return new Response("", { status: 404 });
});
const { port } = await server.addr;
const base = `http://127.0.0.1:${port}`;

// -- instance metadata ------------------------------------------------------

{
  const c = await instanceMetadata({ endpoint: base })();
  is(c.accessKeyId, "IMDS", "IMDSv2: token, role, credentials");
  is(c.sessionToken, "IMDS-token", "with the session token");
  ok(c.expiration instanceof Date && c.expiration.toISOString() === EXPIRES, "and the expiration");
  const token = seen.find((s) => s.path === "/latest/api/token");
  is(token?.method, "PUT", "the token is a PUT");

  imdsMode = "v1-only";
  const before = seen.length;
  await rejects(
    () => instanceMetadata({ endpoint: base })(),
    "IMDSv2",
    "a refused token is an error, never an IMDSv1 fallback",
  );
  ok(
    seen.slice(before).every((s) => s.path === "/latest/api/token"),
    "no unauthenticated metadata request was made",
  );
  imdsMode = "no-role";
  await rejects(
    () => instanceMetadata({ endpoint: base })(),
    "no IAM role",
    "an instance with no role says so",
  );
  imdsMode = "ok";
  await rejects(
    () => instanceMetadata({ endpoint: "http://example.com" })(),
    "must be HTTPS",
    "a plain-HTTP metadata host that is not the metadata service",
  );
}

// -- container --------------------------------------------------------------

{
  const fromFile = await containerCredentials({
    uri: `${base}/creds`,
    tokenFile: `${here}container-token.txt`,
  })();
  is(fromFile.accessKeyId, "FILE", "the token file is sent as Authorization");
  const fixed = await containerCredentials({ uri: `${base}/creds`, token: "fixed-token" })();
  is(fixed.accessKeyId, "FIXED", "a fixed token");
  const both = await containerCredentials({
    uri: `${base}/creds`,
    token: "fixed-token",
    tokenFile: `${here}container-token.txt`,
  })();
  is(both.accessKeyId, "FILE", "the file wins over the token, as the SDKs do");
  await rejects(
    () => containerCredentials({ uri: `${base}/creds`, token: "wrong" })(),
    "403",
    "a refused token",
  );
  await rejects(
    () => containerCredentials({ uri: `${base}/creds`, token: "a\r\nInjected: x" })(),
    "line break",
    "a token with CRLF is refused",
  );
  await rejects(
    () => containerCredentials({ uri: `${base}/creds-bad` })(),
    "lacks",
    "an answer without the fields",
  );
  await rejects(
    () => containerCredentials({ uri: "http://10.0.0.5/creds" })(),
    "must be HTTPS",
    "plain HTTP to an arbitrary host is refused",
  );
  await rejects(
    () => containerCredentials()(),
    "no endpoint",
    "with no URI and no variable, it says which variables it read",
  );
}

// -- web identity -----------------------------------------------------------

{
  const c = await webIdentity({
    endpoint: base,
    roleArn: "arn:aws:iam::123456789012:role/web",
    tokenFile: `${here}web-identity-token.txt`,
    roleSessionName: "ci-run",
  })();
  is(c.accessKeyId, "STS-ci-run", "AssumeRoleWithWebIdentity");
  is(c.sessionToken, "sts-token", "returns the session token");
  const sent = new URLSearchParams(seen[seen.length - 1].body);
  is(sent.get("WebIdentityToken"), "oidc.jwt.from-file", "the token file's contents, trimmed");
  is(sent.get("Version"), "2011-06-15", "STS's API version");
  ok(!("authorization" in seen[seen.length - 1].headers), "and it is not signed");
  let calls = 0;
  await webIdentity({
    endpoint: base,
    roleArn: "arn:aws:iam::123456789012:role/web",
    token: () => `fresh-${++calls}`,
  })();
  is(
    new URLSearchParams(seen[seen.length - 1].body).get("WebIdentityToken"),
    "fresh-1",
    "a token function is asked on refresh",
  );
  ok(
    /^esrun-s3-\d+$/.test(new URLSearchParams(seen[seen.length - 1].body).get("RoleSessionName")),
    "a default session name",
  );
  // run.sh sets AWS_ROLE_ARN and AWS_WEB_IDENTITY_TOKEN_FILE, as EKS does.
  const fromEnv = await webIdentity({ endpoint: base, roleSessionName: "from-env" })();
  is(fromEnv.accessKeyId, "STS-from-env", "AWS_ROLE_ARN and AWS_WEB_IDENTITY_TOKEN_FILE are read");
  const envSent = new URLSearchParams(seen[seen.length - 1].body);
  is(
    envSent.get("RoleArn"),
    "arn:aws:iam::123456789012:role/from-env",
    "the role from the environment",
  );
  is(envSent.get("WebIdentityToken"), "oidc.jwt.from-file", "the token file from the environment");
  stsAnswer = new Response(
    "<ErrorResponse><Error><Type>Sender</Type><Code>InvalidIdentityToken</Code><Message>Couldn't retrieve verification key</Message></Error></ErrorResponse>",
    { status: 400 },
  );
  await rejects(
    () =>
      webIdentity({ endpoint: base, roleArn: "arn:aws:iam::123456789012:role/web", token: "t" })(),
    "InvalidIdentityToken",
    "STS's error code is reported",
  );
  await rejects(
    () =>
      webIdentity({ endpoint: base, roleArn: "arn:x", token: "t", roleSessionName: "has space" })(),
    "roleSessionName",
    "a bad session name is refused before sending",
  );
  await rejects(
    () => webIdentity({ endpoint: base, roleArn: "arn:x", token: "t", durationSeconds: 60 })(),
    "durationSeconds",
    "durationSeconds under 900 is refused",
  );
  await rejects(
    () => webIdentity({ endpoint: "http://sts.example.com", roleArn: "arn:x", token: "t" })(),
    "must be HTTPS",
    "a token is never sent to STS over plain HTTP",
  );
}

// -- assume role ------------------------------------------------------------

{
  const c = await assumeRole({
    endpoint: base,
    region: "eu-west-1",
    roleArn: "arn:aws:iam::123456789012:role/target",
    roleSessionName: "assumed",
    externalId: "ext-1",
    credentials: { accessKeyId: "BASE", secretAccessKey: "base-secret" },
  })();
  is(c.accessKeyId, "STS-assumed", "AssumeRole is signed for sts and accepted");
  is(
    new URLSearchParams(seen[seen.length - 1].body).get("ExternalId"),
    "ext-1",
    "externalId is sent",
  );
  const chained = await assumeRole({
    endpoint: base,
    roleArn: "arn:aws:iam::123456789012:role/target",
    roleSessionName: "chained",
    credentials: async () => ({
      accessKeyId: "BASE",
      secretAccessKey: "base-secret",
      sessionToken: "base-token",
    }),
  })();
  is(chained.accessKeyId, "STS-chained", "with a provider and a session token");
  await rejects(
    () =>
      assumeRole({
        endpoint: base,
        roleArn: "arn:x:role/t",
        credentials: { accessKeyId: "BASE", secretAccessKey: "wrong" },
      })(),
    "SignatureDoesNotMatch",
    "a wrong base secret is STS's refusal",
  );
}

await server.stop();
if (report("providers") > 0) throw new Error("providers failed");
