// `runtime:net` argument and failure reporting, and `runtime:process` args
// immutability — all guest-facing behaviour that needs the real providers.
import { connect, listen } from "runtime:net";
import { args } from "runtime:process";

const show = (label, fn) => {
  try {
    const r = fn();
    console.log(`${label}:NO-THROW:${r === undefined ? "" : r}`);
  } catch (e) {
    console.log(`${label}:${e.constructor.name}:${e.message.split(":").slice(0, 2).join(":")}`);
  }
};

// A port that is not a port used to become 0, so a typo'd port silently
// connected somewhere else instead of saying so.
show("negative", () => connect({ hostname: "127.0.0.1", port: -1 }));
show("missing", () => connect({ hostname: "127.0.0.1" }));
show("nan", () => connect({ hostname: "127.0.0.1", port: "nope" }));
show("toobig", () => connect({ hostname: "127.0.0.1", port: 70000 }));
show("zero-connect", () => connect({ hostname: "127.0.0.1", port: 0 }));
// listen(0) is the documented way to ask for an ephemeral port and must work.
const l = listen({ hostname: "127.0.0.1", port: 0 });
const { port } = await l.addr;
console.log(`listen-zero:${typeof port === "number" && port > 0}`);
await l.close();

// A bind failure carries the same SocketError shape a connect failure does;
// it used to be a raw Error, so the two reported differently.
try {
  await listen({ hostname: "127.0.0.1", port: 1 }).addr;
  console.log("privileged:NO-THROW");
} catch (e) {
  console.log(`privileged:${e.constructor.name}:${e.message.startsWith("SocketError: ")}`);
}

// `args` is documented as frozen, and now reports as frozen: `Object.isFrozen`
// asks [[IsExtensible]] first, which used to bypass the lazy seeding entirely.
console.log(`args-frozen:${Object.isFrozen(args)}`);
console.log(`args-array:${Array.isArray(args)}`);
try {
  args.push("nope");
  console.log("args-push:NO-THROW");
} catch {
  console.log("args-push:threw");
}
console.log("NET_VALIDATION_OK");
