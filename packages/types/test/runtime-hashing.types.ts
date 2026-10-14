// A type test for `runtime:hashing`'s TOTP (D164). Compiled by `tsc -p .`, never run.

import { password, totp } from "runtime:hashing";

const secret: string = totp.generateSecret();
const uri: string = totp.uri({ secret, issuer: "Acme", account: "alice@example.com" });
const code: string = totp.generate(secret, { time: new Date(), digits: 6 });
const step: number | null = totp.verify(code, secret, { window: 1, after: null });
const codes: string[] = totp.backupCodes(10);
const stored: Promise<string> = password.hash(codes[0]);
// @ts-expect-error — authenticator codes are 6, 7 or 8 digits.
totp.generate(secret, { digits: 9 });
// @ts-expect-error — a URI needs the issuer.
totp.uri({ secret, account: "alice" });
void [uri, step, stored];
