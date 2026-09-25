// The client against a real mail server that is not ours: Mailpit, with a
// certificate from a private authority and a password file. Every message is
// read back through Mailpit's API and checked as the server parsed it — which
// is the evidence that the MIME this client writes is MIME another
// implementation reads the same way.
//
//   eval "$(test/mailpit-server.sh)" && esrun --allow-imports --allow-net --allow-env test/mailpit.mjs
import { env } from "runtime:process";
import { createTransport, SmtpErrorCode } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

if (env.SMTP_STARTTLS === undefined) {
  console.log("skip mailpit — run test/mailpit-server.sh first (it prints the environment)");
  (await import("runtime:process")).exit(0);
}

const ca = env.SMTP_CA;
const [host, starttlsPort] = env.SMTP_STARTTLS.split(":");
const [, tlsPort] = env.SMTP_TLS.split(":");

async function api(base, path, init) {
  const response = await fetch(`${base}/api/v1${path}`, init);
  if (!response.ok) throw new Error(`${path}: ${response.status}`);
  const type = response.headers.get("content-type") ?? "";
  return type.includes("json") ? response.json() : response.text();
}

/** The one message the server holds, as it parsed it. */
async function received(base, messageId) {
  const { messages } = await api(base, "/messages");
  const summary = messages.find((m) => `<${m.MessageID}>` === messageId);
  ok(summary !== undefined, `the server has ${messageId}`);
  if (summary === undefined) return {};
  return {
    ...(await api(base, `/message/${summary.ID}`)),
    raw: await api(base, `/message/${summary.ID}/raw`),
  };
}

async function rejects(fn, code, what) {
  try {
    await fn();
    ok(false, `${what} — nothing was thrown`);
  } catch (e) {
    is(e.code, code, `${what} (${e.message.slice(0, 80)})`);
  }
}

for (const base of [env.SMTP_STARTTLS_API, env.SMTP_TLS_API])
  await api(base, "/messages", { method: "DELETE" });

// --- STARTTLS, a login, and every part a message can have -------------------------

{
  const mail = createTransport({
    host,
    port: Number(starttlsPort),
    security: "starttls",
    ca,
    user: "app",
    password: "s3cret",
  });
  const sent = await mail.send({
    from: { name: "Zoë from App", address: "app@example.com" },
    to: "ada@example.com",
    cc: "Grace Hopper <grace@example.com>",
    bcc: "audit@example.com",
    replyTo: "support@example.com",
    subject: "Grüße — your receipt ☕",
    text: "Hello Ada,\n\nYour receipt is attached.\n.\nThat line was a lone dot.",
    html: '<p>Hello <b>Ada</b>,</p><p><img src="cid:logo"></p>',
    attachments: [
      {
        filename: "logo.png",
        content: new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10]),
        contentType: "image/png",
        cid: "logo",
      },
      { filename: "résumé 2026.pdf", content: "%PDF-1.4 fake", contentType: "application/pdf" },
    ],
    headers: { "X-Campaign": "Été 2026" },
  });
  is(
    sent.accepted,
    ["ada@example.com", "grace@example.com", "audit@example.com"],
    "STARTTLS: every recipient accepted",
  );

  const got = await received(env.SMTP_STARTTLS_API, sent.messageId);
  is(got.Subject, "Grüße — your receipt ☕", "the encoded subject decodes");
  is(got.From?.Name, "Zoë from App", "the encoded display name decodes");
  is(got.From?.Address, "app@example.com", "the sender");
  is(got.Cc?.[0]?.Name, "Grace Hopper", "Cc, with its name");
  is(
    got.Bcc?.map((b) => b.Address),
    ["audit@example.com"],
    "Bcc reached the server on the envelope",
  );
  // Mailpit prepends its own Bcc, Return-Path and Received on receipt; the
  // headers this client wrote start at Date.
  const written = got.raw.slice(got.raw.indexOf("\r\nDate:") + 2).split("\r\n\r\n")[0];
  ok(
    written.startsWith("Date:") && !/^Bcc:/im.test(written),
    "and is in none of the headers the client wrote",
  );
  is(got.ReplyTo?.[0]?.Address, "support@example.com", "Reply-To");
  is(
    got.Text?.replace(/\r\n/g, "\n").trim(),
    "Hello Ada,\n\nYour receipt is attached.\n.\nThat line was a lone dot.",
    "the text part, lone dot included",
  );
  ok(got.HTML?.includes("<b>Ada</b>"), "the HTML part");
  is(
    got.Attachments?.map((a) => [a.FileName, a.ContentType]),
    [["résumé 2026.pdf", "application/pdf"]],
    "the attachment and its RFC 2231 filename",
  );
  is(
    got.Inline?.map((a) => [a.FileName, a.ContentID]),
    [["logo.png", "logo"]],
    "the inline image, by Content-ID",
  );
  ok(/^X-Campaign: =\?UTF-8\?B\?/m.test(got.raw), "the custom header, encoded");
  await mail.close();
}

// --- implicit TLS -------------------------------------------------------------------------

{
  const mail = createTransport(`smtps://app:s3cret@${host}:${tlsPort}`);
  // A URL cannot carry a certificate; the options form can, so build one from it.
  const withCa = createTransport({
    host,
    port: Number(tlsPort),
    security: "tls",
    ca,
    user: "app",
    password: "s3cret",
  });
  const sent = await withCa.send({
    from: "app@example.com",
    to: "ada@example.com",
    subject: "Implicit TLS",
    text: "Over 465-style TLS.",
  });
  const got = await received(env.SMTP_TLS_API, sent.messageId);
  is(got.Subject, "Implicit TLS", "implicit TLS: delivered");
  await rejects(
    () => mail.verify(),
    SmtpErrorCode.Tls,
    "implicit TLS without the private CA is refused",
  );
  await withCa.close();
}

// --- a raw message, byte for byte ----------------------------------------------------------

{
  const mail = createTransport({
    host,
    port: Number(starttlsPort),
    ca,
    user: "app",
    password: "s3cret",
  });
  const body = "Line one\r\n.starts with a dot\r\n..two dots\r\nlast";
  const raw = `From: app@example.com\r\nTo: ada@example.com\r\nSubject: Raw\r\nMessage-ID: <raw-1@example.com>\r\n\r\n${body}`;
  await mail.sendRaw({ from: "app@example.com", to: ["ada@example.com"] }, raw);
  const got = await received(env.SMTP_STARTTLS_API, "<raw-1@example.com>");
  ok(
    got.raw?.endsWith(`${body}\r\n`) || got.raw?.endsWith(body),
    "a raw message's body arrives byte for byte, dots and all",
  );
  await mail.close();
}

// --- refusals from a real server ----------------------------------------------------------

await rejects(
  () =>
    createTransport({
      host,
      port: Number(starttlsPort),
      ca,
      user: "app",
      password: "wrong",
    }).verify(),
  SmtpErrorCode.Auth,
  "a wrong password",
);
await rejects(
  () =>
    createTransport({ host, port: Number(starttlsPort), user: "app", password: "s3cret" }).verify(),
  SmtpErrorCode.Tls,
  "STARTTLS without the private CA",
);
await rejects(
  () =>
    createTransport({
      host,
      port: Number(starttlsPort),
      security: "none",
      user: "app",
      password: "s3cret",
    }).verify(),
  SmtpErrorCode.PlaintextAuth,
  "a login without TLS is refused before the server is asked",
);

const failures = report("mailpit");
(await import("runtime:process")).exit(failures > 0 ? 1 : 0);
