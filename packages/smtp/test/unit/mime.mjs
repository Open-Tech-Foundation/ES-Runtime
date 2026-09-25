// The MIME encoders and the message builder. Encoded values were computed
// independently (coreutils base64, Python's quopri and urllib.parse.quote).

import { formatAddress, parseAddress } from "../../dist/mime/address.js";
import { encodeWord, foldHeader, quotedPrintable } from "../../dist/mime/encode.js";
import { buildMessage, formatDate } from "../../dist/mime/message.js";
import { is, ok, report, throws } from "./assert.mjs";

const sevenBit = (text) => [...text].every((c) => c.charCodeAt(0) < 128);

// --- encoders -----------------------------------------------------------------

is(encodeWord("Grüße"), "=?UTF-8?B?R3LDvMOfZQ==?=", "an encoded-word");
{
  // A long subject becomes several words, none over 75 characters, none
  // splitting a character — each decodes on its own.
  const subject = "Grüße aus Köln — ein Betreff, der länger ist als eine Zeile";
  const words = encodeWord(subject).split(" ");
  ok(words.length > 1, "a long value is several words");
  ok(
    words.every((w) => w.length <= 75),
    "no word exceeds 75 characters",
  );
  const decoded = words
    .map((w) =>
      new TextDecoder("utf-8", { fatal: true }).decode(
        Uint8Array.from(atob(w.slice(10, -2)), (c) => c.charCodeAt(0)),
      ),
    )
    .join("");
  is(decoded, subject, "the words decode, each on its own, back to the value");
}

is(quotedPrintable("naïve café = ok\t"), "na=C3=AFve caf=C3=A9 =3D ok=09", "quoted-printable");
{
  const long = quotedPrintable("é".repeat(60));
  ok(
    long.split("\r\n").every((line) => line.length <= 76),
    "quoted-printable lines stay within 76",
  );
  ok(long.includes("=\r\n"), "with soft breaks");
  // Decoding it back proves no soft break landed inside an escape.
  const unsoft = long.replace(/=\r\n/g, "");
  const bytes = [];
  for (let i = 0; i < unsoft.length; i++) {
    if (unsoft[i] === "=") {
      bytes.push(Number.parseInt(unsoft.slice(i + 1, i + 3), 16));
      i += 2;
    } else bytes.push(unsoft.charCodeAt(i));
  }
  is(
    new TextDecoder().decode(new Uint8Array(bytes)),
    "é".repeat(60),
    "and decode back to the text",
  );
}
is(quotedPrintable("a\nb"), "a\r\nb", "hard line breaks stay line breaks");

{
  const folded = foldHeader(
    "To",
    Array.from({ length: 8 }, (_, i) => `person${i}@example.com,`).join(" "),
  );
  ok(
    folded.split("\r\n").every((line) => line.length <= 78),
    "headers fold at 78",
  );
  ok(
    folded
      .split("\r\n")
      .slice(1)
      .every((line) => line.startsWith(" ")),
    "continuation lines start with whitespace",
  );
}

// --- addresses ----------------------------------------------------------------

is(
  parseAddress("Ada Lovelace <ada@example.com>"),
  { name: "Ada Lovelace", address: "ada@example.com" },
  "name and address",
);
is(parseAddress('"Lovelace, Ada" <ada@example.com>').name, "Lovelace, Ada", "a quoted name");
is(
  parseAddress({ name: "Ada", address: "ada@example.com" }).address,
  "ada@example.com",
  "the object form",
);
is(
  parseAddress("ada@bücher.example").address,
  "ada@xn--bcher-kva.example",
  "an IDN domain goes as punycode",
);
is(
  formatAddress({ name: "Lovelace, Ada", address: "a@x.io" }),
  '"Lovelace, Ada" <a@x.io>',
  "a name with a comma is quoted",
);
is(
  formatAddress({ name: "Zoë", address: "z@x.io" }),
  "=?UTF-8?B?Wm/Dqw==?= <z@x.io>",
  "a non-ASCII name is encoded",
);
await throws(
  () => parseAddress("ada@example.com\r\nBcc: everyone@example.com"),
  "a line break in an address is refused",
);
await throws(() => parseAddress("not an address"), "no @ is refused");
await throws(
  () => parseAddress({ name: "x\r\nBcc: y@z.io", address: "a@b.io" }),
  "a line break in a name is refused",
);

// --- messages -----------------------------------------------------------------

const fixed = { date: new Date(Date.UTC(2026, 8, 26, 9, 5, 7)), messageId: "fixed@example.com" };
is(formatDate(fixed.date), "Sat, 26 Sep 2026 09:05:07 +0000", "RFC 5322 date");

{
  const built = await buildMessage({
    ...fixed,
    from: "App <app@example.com>",
    to: ["ada@example.com", "grace@example.com"],
    cc: "ada@example.com",
    bcc: "audit@example.com",
    subject: "Hello",
    text: "Hi there.",
  });
  const [head, body] = built.text.split("\r\n\r\n");
  is(
    built.envelope,
    { from: "app@example.com", to: ["ada@example.com", "grace@example.com", "audit@example.com"] },
    "the envelope has every recipient once",
  );
  ok(!/audit@/.test(built.text), "Bcc is on the envelope and nowhere in the message");
  ok(head.includes("Message-ID: <fixed@example.com>"), "the Message-ID");
  ok(head.includes("Content-Type: text/plain; charset=utf-8"), "a text-only message is one part");
  ok(head.includes("Content-Transfer-Encoding: 7bit"), "ASCII text goes as 7bit");
  is(body, "Hi there.", "and unchanged");
  ok(sevenBit(built.text), "the whole message is 7-bit");
}

{
  const built = await buildMessage({
    ...fixed,
    from: "app@example.com",
    to: "ada@example.com",
    subject: "Café ☕",
    text: "Plain.",
    html: '<p>Rich <img src="cid:logo"></p>',
    attachments: [
      {
        filename: "logo.png",
        content: new Uint8Array([137, 80, 78, 71]),
        contentType: "image/png",
        cid: "logo",
      },
      { filename: "résumé 2026.pdf", content: "PDF", contentType: "application/pdf" },
    ],
  });
  const t = built.text;
  const order = [
    "multipart/mixed",
    "multipart/related",
    "multipart/alternative",
    "text/plain",
    "text/html",
    "image/png",
    "application/pdf",
  ].map((type) => t.indexOf(type));
  ok(
    order.every((at, i) => at > (order[i - 1] ?? -1)),
    "mixed › related › alternative › text, html, then the files",
  );
  ok(t.includes("Subject: =?UTF-8?B?"), "a non-ASCII subject is encoded");
  ok(t.includes("Content-ID: <logo>"), "the inline image has its Content-ID");
  ok(t.includes("Content-Disposition: inline"), "and is inline");
  ok(
    t.includes("filename*=UTF-8''r%C3%A9sum%C3%A9%202026.pdf"),
    "a non-ASCII filename uses RFC 2231",
  );
  ok(t.includes("iVBORw=="), "the attachment is base64");
  ok(sevenBit(t), "and the message is still 7-bit throughout");
  // Every boundary opened is closed.
  for (const [, marker] of t.matchAll(/boundary="([^"]+)"/g))
    ok(t.includes(`--${marker}--`), `boundary ${marker.slice(-6)} is closed`);
}

await throws(() => buildMessage({ from: "a@b.io", subject: "x" }), "no recipients is refused");
await throws(
  () => buildMessage({ from: "a@b.io", to: "c@d.io", subject: "Hi\r\nBcc: e@f.io" }),
  "a line break in the subject is refused",
);
await throws(
  () => buildMessage({ from: "a@b.io", to: "c@d.io", headers: { "X-Tag": "a\nb" } }),
  "a line break in a header is refused",
);
await throws(
  () => buildMessage({ from: "a@b.io", to: "c@d.io", headers: { From: "x@y.io" } }),
  "a header the fields build is refused",
);
await throws(
  () => buildMessage({ from: "a@b.io", to: "c@d.io", headers: { "Bad Name": "x" } }),
  "a header name with a space is refused",
);

{
  const built = await buildMessage({
    from: "a@b.io",
    to: "c@d.io",
    headers: { "X-Campaign": "Été" },
  });
  ok(built.text.includes("X-Campaign: =?UTF-8?B?"), "a custom header's non-ASCII value is encoded");
  ok(
    /Message-ID: <[0-9a-f-]{36}@b\.io>/.test(built.text),
    "the default Message-ID is on the sender's domain",
  );
}
ok(
  (await buildMessage({ from: "ada@b.io", to: "zoë@d.io" })).smtpUtf8,
  "a non-ASCII local part needs SMTPUTF8",
);
ok(
  !(await buildMessage({ from: "ada@b.io", to: "z@bücher.example" })).smtpUtf8,
  "an IDN domain does not",
);

{
  // Anything with arrayBuffer() — runtime:fs's file() is not a Blob, but is
  // read like one — and its own type is the default Content-Type.
  const blobLike = {
    type: "text/csv",
    arrayBuffer: async () => new TextEncoder().encode("a,b").buffer,
  };
  const built = await buildMessage({
    from: "a@b.io",
    to: "c@d.io",
    attachments: [{ filename: "t.csv", content: blobLike }],
  });
  ok(
    built.text.includes('Content-Type: text/csv; name="t.csv"'),
    "a Blob-like attachment keeps its type",
  );
  ok(built.text.includes(btoa("a,b")), "and its bytes");
  const blob = await buildMessage({
    from: "a@b.io",
    to: "c@d.io",
    attachments: [{ content: new Blob(["x"], { type: "text/plain" }) }],
  });
  ok(blob.text.includes("Content-Type: text/plain\r\n"), "a Blob's type is used");
}

if (report("mime") > 0) (await import("runtime:process")).exit(1);
