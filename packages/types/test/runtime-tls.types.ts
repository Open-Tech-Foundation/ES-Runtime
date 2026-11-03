// A type test for certificate rotation, mutual TLS and revocation (DECISIONS
// D200–D202) across runtime:net, runtime:http and fetch.

import { serve } from "runtime:http";
import type { PeerCertificate, SecureContext } from "runtime:net";
import { connect, listen } from "runtime:net";

declare const pem: string;
declare const bytes: Uint8Array;

// --- a listener that requires client certificates --------------------------------

const listener = listen({
  hostname: "127.0.0.1",
  port: 0,
  secureTransport: "on",
  cert: pem,
  key: bytes,
  requestCert: true,
  ca: pem,
  crl: bytes,
});
const rotated: Promise<void> = listener.setSecureContext({ cert: pem, key: pem });
const context: SecureContext = { ca: bytes, crl: pem };

// --- a client that presents one --------------------------------------------------

const socket = connect(
  { hostname: "db.internal", port: 5432 },
  { secureTransport: "on", cert: pem, key: pem },
);
socket.opened.then((info) => {
  const peer: PeerCertificate | null = info.peerCertificate;
  if (peer !== null) {
    void [peer.subject.CN satisfies string | undefined, peer.subjectaltname, peer.fingerprint256];
    void [peer.raw satisfies Uint8Array, peer.validFrom satisfies number, peer.validTo];
  }
});

// --- runtime:http hands the verified certificate to the handler -----------------

const server = serve(
  { port: 8443, secureTransport: "on", cert: pem, key: pem, requestCert: true, ca: pem },
  (_request, info) => new Response(info.peerCertificate?.subject.CN ?? "anonymous"),
);
const swapped: Promise<void> = server.setSecureContext({ cert: pem, key: pem });

// --- fetch takes Bun's `tls` member ----------------------------------------------

void fetch("https://billing.internal/", { tls: { ca: pem, cert: pem, key: bytes } });
// @ts-expect-error — verification cannot be turned off.
void fetch("https://x/", { tls: { rejectUnauthorized: false } });
// @ts-expect-error — a subject has only the attributes the runtime reads.
void ((peer: PeerCertificate) => peer.subject.emailAddress);

void [rotated, context, swapped];
