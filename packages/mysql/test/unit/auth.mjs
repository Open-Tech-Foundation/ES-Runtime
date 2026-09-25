// The two password plugins against vectors computed independently (Python's
// hashlib), since a server only ever says yes or no.
import { exit } from "runtime:process";
import { cachingSha2, nativePassword, passwordBytes } from "../../dist/protocol/auth.js";
import { is, report } from "./assert.mjs";

const hex = (bytes) => [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
const scramble = Uint8Array.from({ length: 20 }, (_, i) => i + 1);

is(
  hex(await nativePassword("secret", scramble)),
  "b32bb3a583e1340c0a1108d58b1be49781ad8c2f",
  "mysql_native_password",
);
is(
  hex(await cachingSha2("secret", scramble)),
  "746ebe205d56a0707acb3e796e834e0dd7b1d61743b26bd5202c7a623230c7c9",
  "caching_sha2_password",
);
is((await nativePassword("", scramble)).length, 0, "an empty password sends nothing (native)");
is((await cachingSha2("", scramble)).length, 0, "an empty password sends nothing (sha2)");
is([...passwordBytes("é")], [0xc3, 0xa9, 0], "the full-auth password is UTF-8 and NUL-terminated");

if (report("auth") > 0) exit(1);
