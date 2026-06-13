// Imports a parent-directory module — exercises ../ resolution on a real path.
import { greet, NAME } from "../greet.mjs";

console.log("nested:" + greet(NAME));
