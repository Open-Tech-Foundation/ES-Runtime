// The login encodings, against values computed independently, and the choice
// of mechanism.

import { base64, choose, plain, unbase64, xoauth2 } from "../../dist/protocol/auth.js";
import { is, report } from "./assert.mjs";

// `printf '\0tim\0tanstaaftanstaaf' | base64` — RFC 4616 §4's example credentials.
is(plain("tim", "tanstaaftanstaaf"), "AHRpbQB0YW5zdGFhZnRhbnN0YWFm", "PLAIN");
// Non-ASCII goes as UTF-8, which `btoa` alone would refuse.
is(unbase64(plain("zoë", "pässword")), "\0zoë\0pässword", "PLAIN carries UTF-8");
// Google's documented example: user=someuser@example.com, token ya29.vF9dft4qmTc2Nvb3RlckBhdHRhdmlzdGEuY29tCg
is(
  xoauth2("someuser@example.com", "ya29.vF9dft4qmTc2Nvb3RlckBhdHRhdmlzdGEuY29tCg"),
  "dXNlcj1zb21ldXNlckBleGFtcGxlLmNvbQFhdXRoPUJlYXJlciB5YTI5LnZGOWRmdDRxbVRjMk52YjNSbGNrQmhkSFJoZG1semRHRXVZMjl0Q2cBAQ==",
  "XOAUTH2",
);
is(base64("Username:"), "VXNlcm5hbWU6", "LOGIN's prompt, the other way round");

is(choose(["LOGIN", "PLAIN"], { user: "u", password: "p" }), "PLAIN", "PLAIN is preferred");
is(choose(["LOGIN"], { user: "u", password: "p" }), "LOGIN", "LOGIN when that is all there is");
is(
  choose(["PLAIN", "XOAUTH2"], { user: "u", accessToken: "t" }),
  "XOAUTH2",
  "a token means XOAUTH2",
);
// A token must not fall back to a password login the caller did not ask for.
is(
  choose(["PLAIN", "LOGIN"], { user: "u", accessToken: "t" }),
  null,
  "a token with no XOAUTH2 is no login",
);
is(
  choose(["CRAM-MD5"], { user: "u", password: "p" }),
  null,
  "an unsupported mechanism is no login",
);

if (report("auth") > 0) (await import("runtime:process")).exit(1);
