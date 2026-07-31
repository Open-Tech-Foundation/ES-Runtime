// Signal handling against the real OS: the parent test sends a genuine SIGTERM
// once READY is printed. Nothing here simulates a signal.
import { onSignal, offSignal, signals } from "runtime:process";

console.log(`AVAILABLE ${signals().join(",")}`);

let count = 0;
const handler = (name) => {
  count += 1;
  console.log(`GOT ${name} count:${count}`);
  // Removing the last handler stops watching, which releases the pump — the
  // process can then exit normally instead of waiting for a signal forever.
  offSignal("SIGTERM", handler);
  console.log("RELEASED");
};

onSignal("SIGTERM", handler);
console.log("READY");
