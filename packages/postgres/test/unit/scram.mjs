// SCRAM-SHA-256 against RFC 7677 §3's published vectors.
//
// The exchange is a chain of HMACs and an XOR, and every step of it is a place
// to be subtly wrong in a way that still produces plausible-looking base64. The
// only check worth having is the one the specification publishes.
import { scram } from "../../dist/protocol/scram.js";
import { exit } from "runtime:process";
import { is, report } from "./assert.mjs";

const session = scram("pencil", { username: "user", nonce: "rOprNGfwEbeRWgbNEkqO" });

is(
  session.initial,
  "n,,n=user,r=rOprNGfwEbeRWgbNEkqO",
  "client-first-message matches RFC 7677",
);

const serverFirst =
  "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096";
const final = await session.final(serverFirst);

is(
  final.message,
  "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=",
  "client-final-message matches RFC 7677",
);

// Mutual authentication: the client must reject a server that cannot prove it
// knows the password, or the handshake only ever authenticated one direction.
let verified = "no error";
try {
  final.verify("v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=");
  verified = "accepted";
} catch (e) {
  verified = `rejected: ${e.message}`;
}
is(verified, "accepted", "the RFC's server signature verifies");

let wrong = "accepted";
try {
  final.verify("v=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
} catch {
  wrong = "rejected";
}
is(wrong, "rejected", "a wrong server signature is rejected");

// A server that does not echo the client's nonce is not the server the exchange
// started with.
let echoed = "accepted";
try {
  await scram("pencil", { nonce: "aaaa" }).final("r=bbbb,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096");
} catch {
  echoed = "rejected";
}
is(echoed, "rejected", "a nonce that does not extend the client's is rejected");

let malformed = "accepted";
try {
  await scram("pencil", { nonce: "aaaa" }).final("nonsense");
} catch {
  malformed = "rejected";
}
is(malformed, "rejected", "a malformed challenge is rejected");

if (report("scram") > 0) exit(1);
