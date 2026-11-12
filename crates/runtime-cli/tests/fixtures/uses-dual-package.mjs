// Imports a package whose ES module half is marked by a nested
// `{ "type": "module" }` manifest under a root that names no type.
import { half } from "dualkit";

console.log(`dualkit loaded its ${half} half`);
