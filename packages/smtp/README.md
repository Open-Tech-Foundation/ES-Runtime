# @opentf/esrun-smtp

An SMTP client for [esrun](https://esrun.opentechf.org), written entirely in
JavaScript over `runtime:net`. There is no native code in this package, and none
was added to the runtime for it.

```sh
npm install @opentf/esrun-smtp
```

```js
import { env } from "runtime:process";
import { createTransport } from "@opentf/esrun-smtp";

const mail = createTransport({
  host: "smtp.example.com",
  user: "app@example.com",
  password: env.SMTP_PASSWORD,
});

const sent = await mail.send({
  from: "Example App <app@example.com>",
  to: "ada@example.com",
  subject: "Welcome",
  text: "Hello, Ada.",
  html: "<p>Hello, <b>Ada</b>.</p>",
});
// { messageId: "<…@example.com>", accepted: ["ada@example.com"], rejected: [], response: "2.0.0 queued as …" }
```

Run it with the network grant scoped to the server:

```sh
esrun --allow-net=smtp.example.com --allow-env=SMTP_PASSWORD app.js
```

## Connecting

| Option | |
| --- | --- |
| `host` | The server. |
| `port` | Defaults by `security`: 465, 587, or 25. |
| `security` | `"tls"` — TLS from the first byte (port 465). `"starttls"` — plaintext, upgraded with `STARTTLS`, which the server **must** offer. `"none"` — plaintext. Defaults to `"tls"` on port 465 and `"starttls"` otherwise. |
| `user`, `password` | A PLAIN or LOGIN login. |
| `user`, `accessToken` | An XOAUTH2 login (Gmail, Microsoft 365). Obtaining and refreshing the token is the OAuth provider's business. |
| `ca` | Extra trust anchors (PEM), for a server whose certificate a private authority signed. |
| `servername` | The name the certificate must carry, when it is not `host`. |
| `allowPlaintextAuth` | Permits a login over `security: "none"`. Off by default. |
| `name` | What `EHLO` announces. Defaults to `localhost`. |
| `timeout` | Milliseconds per reply. Defaults to 60 000. |
| `dataTimeout` | Milliseconds for the reply to the end of a message. Defaults to 600 000. |
| `maxConnections` | Sessions open at once. Defaults to 2. |
| `maxMessages` | Messages per session before it is replaced. Defaults to 100. |
| `idleTimeout` | Milliseconds an unused session is kept. Defaults to 30 000. |

A URL works too: `createTransport("smtps://user:pass@smtp.example.com")`, with
`smtp:` for STARTTLS on 587 and `?security=none` for plaintext.

### Secure by default

- **Certificates are always verified.** There is no option to turn that off; a
  private authority is added with `ca`.
- **`STARTTLS` is required, not attempted.** A server that does not offer it is
  an error rather than a silent downgrade to plaintext.
- **A login over plaintext is refused** unless `allowPlaintextAuth` is set — for
  a local relay or a test server. Nothing is sent before it is refused.
- **A header value containing a line break is refused**, not stripped: silently
  repairing an injection attempt would hide it.

`verify()` opens a session — connect, TLS, login — and returns it to the pool,
which checks the settings at startup rather than at the first email.

## Messages

| Field | |
| --- | --- |
| `from` | `"ada@example.com"`, `"Ada <ada@example.com>"`, or `{ name, address }`. |
| `to`, `cc`, `bcc`, `replyTo` | One address or a list. `bcc` goes on the envelope only, never in a header. |
| `subject` | Any Unicode; encoded as RFC 2047 words when not ASCII. |
| `text`, `html` | Either or both. Both makes `multipart/alternative`. |
| `attachments` | `{ filename, content, contentType?, cid? }`. `content` is a string, `Uint8Array`, `ArrayBuffer`, or anything with `arrayBuffer()` — a `Blob`, or `runtime:fs`'s `file()`. With `cid`, the attachment is inline, for HTML that shows it as `<img src="cid:…">`. |
| `headers` | Further headers by name. The ones the fields build are refused here. |
| `messageId`, `date` | Default to `<uuid@sender's domain>` and now. |
| `envelope` | `{ from, to }`, overriding the addresses the server is told. |

The message is always 7-bit — quoted-printable text, base64 attachments,
encoded-word headers — so it is deliverable through any relay. Domains are sent
in their ASCII form (`bücher.example` → `xn--bcher-kva.example`). An address
with a non-ASCII local part needs the server's `SMTPUTF8`, and is refused
without it.

`sendRaw(envelope, message)` sends a message built elsewhere, as it is. Its lines
are still normalised to CRLF, dot-stuffed, and checked against SMTP's 998-octet
limit.

## Results and errors

`send()` resolves once the server has accepted the message for at least one
recipient: `accepted` lists who, and `rejected` who not, each with the server's
reply. If every recipient is refused, it throws.

Errors are `SmtpError`s with a `code` from `SmtpErrorCode`, `permanent` — `true`
when trying again will not help (a `5xx` reply, a refused login), `false` when it
may (a `4xx`, a lost connection, a timeout) — and the server's `reply`:

```js
import { SmtpError } from "@opentf/esrun-smtp";

try {
  await mail.send(message);
} catch (e) {
  if (e instanceof SmtpError && !e.permanent) queue.retryLater(message);
  else throw e;
}
```

| Code | |
| --- | --- |
| `ERR_SMTP_CONNECTION` | Could not connect, or the connection was lost. |
| `ERR_SMTP_TLS` | TLS failed, or the certificate did not verify. |
| `ERR_SMTP_AUTH` | The login was refused, or none offered fits the credentials. |
| `ERR_SMTP_PLAINTEXT_AUTH` | A login over plaintext without `allowPlaintextAuth`. |
| `ERR_SMTP_SENDER` | `MAIL FROM` refused. |
| `ERR_SMTP_RECIPIENTS` | Every `RCPT TO` refused; `e.rejected` says how. |
| `ERR_SMTP_MESSAGE` | The message refused after `DATA`. |
| `ERR_SMTP_TOO_LARGE` | Larger than the server's `SIZE`; nothing was sent. |
| `ERR_SMTP_UNSUPPORTED` | Needs `STARTTLS`, `SMTPUTF8` or `8BITMIME`, which the server lacks. |
| `ERR_SMTP_INVALID_MESSAGE` | A header with a line break, no recipient, a line over 998 octets. |
| `ERR_SMTP_TIMEOUT` | No reply within `timeout`. |
| `ERR_SMTP_PROTOCOL` | The server's reply was not SMTP. |
| `ERR_SMTP_CLOSED` | The transport was closed. |

## Sessions

Sessions are pooled: a TLS handshake and a login are most of what one message
costs. Up to `maxConnections` are open at once, each carries up to
`maxMessages`, and an unused one is closed after `idleTimeout`. The idle timer
does not keep the process alive. A pooled session the server has since dropped
is replaced and the message retried on the new one, once. `close()` ends them
all.

With the server's `PIPELINING`, `MAIL FROM`, every `RCPT TO` and `DATA` go in one
write — one round trip for the lot, however many recipients.

## Not yet

DKIM signing, delivery status notifications (DSN) and `CHUNKING`. Most
applications send through a relay — SES, Postmark, Gmail, Microsoft 365 — that
signs for them.
