// SigV4 against the AWS signing test suite (awslabs/aws-c-auth, Apache-2.0),
// committed under test/fixtures/sigv4 by test/fixtures/fetch-sigv4-suite.sh.
//
// The suite is written for a generic service: its paths are raw and are
// normalized when the case says so. S3 neither double-encodes nor normalizes,
// which the client does itself before calling the signer — so this harness
// builds `canonicalUri` per the case and checks everything the signer owns:
// query ordering and encoding, header canonicalization, the scope, the key
// derivation and both signature forms.
import { file, readDir } from "runtime:fs";
import { hash } from "runtime:hashing";
import { EMPTY_SHA256, signHeaders, signQuery, uriEncodePath } from "../../dist/sigv4.js";
import { is, ok, report } from "./assert.mjs";

const root = new URL("../fixtures/sigv4/", import.meta.url);

/** `request.txt`: a request line, headers (with obs-fold continuation lines), a blank line, a body. */
function parseRequest(text) {
  const [head, ...rest] = text.split("\n\n");
  const lines = head.split("\n");
  // The target may hold a space (get-space-*): it runs to the last " HTTP/".
  const first = lines[0];
  const method = first.slice(0, first.indexOf(" "));
  const target = first.slice(method.length + 1, first.lastIndexOf(" HTTP/"));
  const headers = [];
  for (const line of lines.slice(1)) {
    if (line === "") continue;
    if (/^\s/.test(line) && headers.length > 0) {
      headers[headers.length - 1][1] += ` ${line.trim()}`;
      continue;
    }
    const at = line.indexOf(":");
    headers.push([line.slice(0, at), line.slice(at + 1)]);
  }
  const q = target.indexOf("?");
  const path = q < 0 ? target : target.slice(0, q);
  const query =
    q < 0 || q === target.length - 1
      ? []
      : target
          .slice(q + 1)
          .split("&")
          .map((pair) => {
            const at = pair.indexOf("=");
            const [k, v] = at < 0 ? [pair, ""] : [pair.slice(0, at), pair.slice(at + 1)];
            return [decodeURIComponent(k), decodeURIComponent(v)];
          });
  return { method, path, query, headers, body: rest.join("\n\n") };
}

function normalize(path) {
  const out = [];
  for (const segment of path.split("/")) {
    if (segment === "" || segment === ".") continue;
    if (segment === "..") out.pop();
    else out.push(segment);
  }
  return `/${out.join("/")}${path.endsWith("/") && out.length > 0 ? "/" : ""}`;
}

const text = async (dir, name) => {
  try {
    return (await file(new URL(name, dir)).text()).replace(/\n$/, "");
  } catch {
    return undefined;
  }
};

let cases = 0;
for (const entry of await readDir(root)) {
  if (!entry.isDir) continue;
  const dir = new URL(`${entry.name}/`, root);
  const context = JSON.parse(await text(dir, "context.json"));
  // The session token added after signing is an IoT variant; S3 signs it.
  if (context.omit_session_token) continue;
  const request = parseRequest(await file(new URL("request.txt", dir)).text());
  const credentials = {
    accessKeyId: context.credentials.access_key_id,
    secretAccessKey: context.credentials.secret_access_key,
    sessionToken: context.credentials.token,
  };
  const scope = {
    region: context.region,
    service: context.service,
    date: new Date(context.timestamp),
  };
  const canonicalUri = uriEncodePath(context.normalize ? normalize(request.path) : request.path);
  const input = {
    method: request.method,
    canonicalUri,
    query: request.query,
    headers: request.headers,
  };
  const payloadHash = context.sign_body ? hash("sha256", request.body, "hex") : EMPTY_SHA256;
  cases++;

  const header = signHeaders(input, credentials, scope, {
    payloadHash,
    contentSha256Header: context.sign_body,
  });
  is(
    header.canonicalRequest,
    await text(dir, "header-canonical-request.txt"),
    `${entry.name}: header canonical request`,
  );
  is(
    header.stringToSign,
    await text(dir, "header-string-to-sign.txt"),
    `${entry.name}: header string to sign`,
  );
  is(header.signature, await text(dir, "header-signature.txt"), `${entry.name}: header signature`);

  const expected = await text(dir, "query-signature.txt");
  if (expected !== undefined) {
    const query = signQuery(input, credentials, scope, {
      expiresIn: context.expiration_in_seconds,
      payloadHash,
    });
    is(
      query.canonicalRequest,
      await text(dir, "query-canonical-request.txt"),
      `${entry.name}: query canonical request`,
    );
    is(query.signature, expected, `${entry.name}: query signature`);
  }
}
ok(cases >= 30, `the suite was found (${cases} cases)`);

// What the suite cannot say about S3: a key is encoded once, `/` stays, and
// the characters encodeURIComponent leaves alone are encoded.
is(
  uriEncodePath("/b/a b/c+d/(x)!*'~._-"),
  "/b/a%20b/c%2Bd/%28x%29%21%2A%27~._-",
  "S3 path encoding",
);
is(uriEncodePath("/b/über"), "/b/%C3%BCber", "UTF-8 in a key");

if (report("sigv4") > 0) throw new Error("sigv4 failed");
