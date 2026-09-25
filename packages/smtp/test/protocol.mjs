// The client against a scriptable server (server.mjs): every path through a
// session, including each way a server can refuse.
import { listen } from "runtime:net";
import { createTransport, SmtpErrorCode } from "../dist/index.js";
import { startServer } from "./server.mjs";
import { is, ok, report } from "./unit/assert.mjs";

const plain = (port, extra = {}) =>
  createTransport({ host: "127.0.0.1", port, security: "none", timeout: 2000, ...extra });
const message = { from: "app@example.com", to: "ada@example.com", subject: "Hi", text: "Hello." };

async function rejects(fn, code, what) {
  try {
    await fn();
    ok(false, `${what} — nothing was thrown`);
    return null;
  } catch (e) {
    is(e.code, code, what);
    return e;
  }
}

// --- a send, pipelined and not -------------------------------------------------

for (const pipelining of [true, false]) {
  const server = await startServer(pipelining ? {} : { extensions: ["SIZE 1048576", "8BITMIME"] });
  const mail = plain(server.port);
  const sent = await mail.send({
    ...message,
    to: ["ada@example.com", "grace@example.com"],
    bcc: "audit@example.com",
  });
  const label = pipelining ? "pipelined" : "one command at a time";
  is(
    sent.accepted,
    ["ada@example.com", "grace@example.com", "audit@example.com"],
    `${label}: every recipient accepted`,
  );
  is(sent.rejected, [], `${label}: none rejected`);
  ok(sent.response.startsWith("queued as"), `${label}: the server's queue reply`);
  ok(/^<.+@example\.com>$/.test(sent.messageId), `${label}: a Message-ID`);
  const [got] = server.state.messages;
  ok(
    got.data.includes("Subject: Hi") && got.data.endsWith("Hello."),
    `${label}: the message arrived whole`,
  );
  ok(/SIZE=\d+/.test(got.params), `${label}: SIZE is declared on MAIL FROM`);
  await mail.close();
  server.close();
}

// --- a server too old for EHLO ---------------------------------------------------

{
  const server = await startServer({ ehlo: 502 });
  const mail = plain(server.port);
  await mail.send(message);
  ok(
    server.state.commands.some((c) => c.startsWith("HELO ")),
    "EHLO refused: HELO instead",
  );
  is(server.state.messages.length, 1, "and the message still goes");
  await mail.close();
  server.close();
}

// --- refusals --------------------------------------------------------------------

{
  const server = await startServer({ rcpt: (a) => (a.startsWith("bad") ? 550 : 250) });
  const mail = plain(server.port);
  const sent = await mail.send({ ...message, to: ["bad@example.com", "ada@example.com"] });
  is(sent.accepted, ["ada@example.com"], "a refused recipient: the rest still get it");
  is(
    sent.rejected.map((r) => [r.address, r.reply.code, r.reply.enhanced]),
    [["bad@example.com", 550, "5.1.1"]],
    "and the refusal is reported",
  );

  const e = await rejects(
    () => mail.send({ ...message, to: "bad@example.com" }),
    SmtpErrorCode.Recipients,
    "every recipient refused",
  );
  ok(e?.permanent === true, "is permanent");
  is(e?.rejected?.length, 1, "and lists who");
  // The session was reset, not abandoned: the next message goes on it.
  await mail.send(message);
  is(server.state.connections, 1, "the session survives a refused transaction");
  await mail.close();
  server.close();
}

{
  const server = await startServer({ mail: 550 });
  const e = await rejects(
    () => plain(server.port).send(message),
    SmtpErrorCode.Sender,
    "a refused sender",
  );
  ok(e?.permanent === true && e?.command === "MAIL FROM", "names the command and is permanent");
  server.close();
}

{
  const server = await startServer({ final: 554 });
  const e = await rejects(
    () => plain(server.port).send(message),
    SmtpErrorCode.Message,
    "refused content",
  );
  is(e?.reply?.code, 554, "carries the reply");
  server.close();
}

{
  const server = await startServer({ mail: 451 });
  const e = await rejects(() => plain(server.port).send(message), SmtpErrorCode.Sender, "a 4xx");
  ok(e?.permanent === false, "is transient");
  server.close();
}

{
  const server = await startServer({ extensions: ["PIPELINING", "SIZE 200"] });
  await rejects(
    () => plain(server.port).send({ ...message, text: "x".repeat(500) }),
    SmtpErrorCode.TooLarge,
    "over SIZE",
  );
  ok(
    !server.state.commands.some((c) => c.startsWith("MAIL")),
    "is refused before anything is sent",
  );
  server.close();
}

// --- security ------------------------------------------------------------------

{
  const server = await startServer();
  const mail = createTransport({ host: "127.0.0.1", port: server.port, timeout: 2000 });
  const e = await rejects(
    () => mail.send(message),
    SmtpErrorCode.Unsupported,
    "no STARTTLS on a starttls transport",
  );
  ok(e?.message.includes('security: "none"'), "says how to opt out");
  is(server.state.messages.length, 0, "and nothing was sent in plaintext");
  server.close();
}

{
  const server = await startServer();
  await rejects(
    () => plain(server.port, { user: "user", password: "secret" }).send(message),
    SmtpErrorCode.PlaintextAuth,
    "a login over plaintext is refused",
  );
  ok(!server.state.commands.some((c) => c.startsWith("AUTH")), "and never sent");
  server.close();
}

for (const [extensions, credentials, mechanism] of [
  [["AUTH PLAIN LOGIN"], { user: "user", password: "secret" }, "PLAIN"],
  [["AUTH LOGIN"], { user: "user", password: "secret" }, "LOGIN"],
  [["AUTH PLAIN XOAUTH2"], { user: "user", accessToken: "token" }, "XOAUTH2"],
]) {
  const server = await startServer({ extensions });
  const mail = plain(server.port, { ...credentials, allowPlaintextAuth: true });
  await mail.send(message);
  is(server.state.auth, mechanism, `${mechanism} login`);
  ok(server.state.messages[0]?.authed, `${mechanism}: authenticated before sending`);
  await mail.close();
  server.close();
}

{
  const server = await startServer();
  const e = await rejects(
    () =>
      plain(server.port, {
        user: "user",
        password: "wrong-password",
        allowPlaintextAuth: true,
      }).send(message),
    SmtpErrorCode.Auth,
    "a wrong password",
  );
  ok(
    !e?.message.includes("wrong-password") && !e?.message.includes(btoa("\0user\0wrong-password")),
    "the error does not carry the secret",
  );
  await rejects(
    () =>
      plain(server.port, { user: "user", accessToken: "expired", allowPlaintextAuth: true }).send(
        message,
      ),
    SmtpErrorCode.Auth,
    "a refused XOAUTH2 token",
  );
  server.close();
}

// --- 8-bit content and the wire format -------------------------------------------

{
  const server = await startServer({ extensions: ["PIPELINING"] });
  const mail = plain(server.port);
  await rejects(
    () =>
      mail.sendRaw({ from: "a@example.com", to: ["b@example.com"] }, "Subject: café\r\n\r\nbody"),
    SmtpErrorCode.Unsupported,
    "8-bit raw content without 8BITMIME",
  );
  // A built message is 7-bit however much non-ASCII it holds, so it goes.
  await mail.send({ ...message, subject: "Café ☕", text: "Grüße" });
  is(server.state.messages.length, 1, "a built message needs no 8BITMIME");
  await mail.close();
  server.close();
}

{
  const server = await startServer();
  const mail = plain(server.port);
  await mail.sendRaw(
    { from: "a@example.com", to: ["b@example.com"] },
    "Subject: x\r\n\r\ncafé\r\n.hidden\r\n.",
  );
  const [got] = server.state.messages;
  ok(/BODY=8BITMIME/.test(got.params), "8-bit raw content declares BODY=8BITMIME");
  ok(
    got.data.endsWith("café\r\n.hidden\r\n."),
    "lines starting with a dot arrive intact (stuffed on the wire)",
  );
  await mail.close();
  server.close();
}

// --- the pool ----------------------------------------------------------------------

{
  const server = await startServer();
  const mail = plain(server.port);
  await mail.send(message);
  await mail.send(message);
  await Promise.all([mail.send(message), mail.send(message), mail.send(message)]);
  is(server.state.messages.length, 5, "five messages sent");
  ok(
    server.state.connections <= 2,
    `over at most maxConnections sessions (${server.state.connections})`,
  );
  await mail.close();
  server.close();
}

{
  // A server that hangs up after each message: the pooled session is dead by
  // the second send, which is retried on a fresh one rather than failed.
  const server = await startServer({ dropAfter: 1 });
  const mail = plain(server.port, { maxConnections: 1 });
  await mail.send(message);
  await mail.send(message);
  is(server.state.messages.length, 2, "a dropped pooled session is replaced");
  await mail.close();
  server.close();
}

{
  const server = await startServer();
  const mail = plain(server.port, { maxMessages: 2, maxConnections: 1 });
  for (let i = 0; i < 4; i++) await mail.send(message);
  is(server.state.connections, 2, "maxMessages replaces the session");
  await mail.verify();
  await mail.close();
  await rejects(
    () => mail.send(message),
    SmtpErrorCode.Closed,
    "a closed transport refuses to send",
  );
  server.close();
}

// --- a server that never answers -----------------------------------------------------

{
  const silent = listen({ hostname: "127.0.0.1", port: 0 });
  const { port } = await silent.addr;
  const held = [];
  (async () => {
    for await (const socket of silent) held.push(socket);
  })().catch(() => {});
  const started = Date.now();
  await rejects(
    () => plain(port, { timeout: 200 }).send(message),
    SmtpErrorCode.Timeout,
    "no greeting times out",
  );
  ok(Date.now() - started < 2000, "on time");
  for (const socket of held) await socket.close();
  silent.close();
}

{
  await rejects(() => plain(1).send(message), SmtpErrorCode.Connection, "nothing listening");
}

const failures = report("protocol");
(await import("runtime:process")).exit(failures > 0 ? 1 : 0);
