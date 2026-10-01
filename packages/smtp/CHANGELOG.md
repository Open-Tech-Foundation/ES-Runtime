# Changelog for `@opentf/esrun-smtp`

All notable changes to **`@opentf/esrun-smtp`**, the SMTP client for ES Runtime,
are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

This package is versioned **separately from `esrun`**: it is an ordinary npm
package written entirely in JavaScript over `runtime:net`. See the root
[CHANGELOG.md](../../CHANGELOG.md) for the runtime itself.

## [Unreleased]

## [0.2.2] - 2026-10-01

_Dependency updates._

## [0.2.1] - 2026-10-01

_Dependency updates._

## [0.2.0] - 2026-09-29

### Added

- **DKIM signing** (DECISIONS D136, amended). `createTransport({ dkim })` signs
  every message, raw ones included, with `rsa-sha256` or `ed25519-sha256`
  (RFC 8463), chosen by the key: a PKCS#8 or PKCS#1 PEM, or a `CryptoKey`.
  `relaxed/relaxed`, the whole body, and the identity headers oversigned; a list
  of signers adds one signature each. A key that cannot sign fails with the new
  `ERR_SMTP_DKIM` before anything is sent, or at `verify()`. Checked against
  RFC 8463's published signatures byte for byte, and on the message Mailpit
  stored.

## [0.1.2] - 2026-09-28

_Dependency updates._

## [0.1.1] - 2026-09-25

### Added

- **The SMTP client** (DECISIONS D136). `createTransport()` connects with
  implicit TLS or a required `STARTTLS`, certificates always verified and a
  private authority added with `ca`; logs in with PLAIN, LOGIN or XOAUTH2, and
  refuses to do so over plaintext unless `allowPlaintextAuth` says otherwise.
  `send()` builds an RFC 5322 message — text and HTML, attachments, inline
  images, encoded-word headers, RFC 2231 filenames, always 7-bit — and sends it
  with `PIPELINING`, `SIZE`, `8BITMIME` and `SMTPUTF8` where the server offers
  them. `sendRaw()` sends a message built elsewhere. Sessions are pooled, a
  dropped one is replaced, and every error carries a code and whether a retry
  can help. Verified against a scriptable server and against Mailpit with a
  private certificate authority.
