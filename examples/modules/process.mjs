// runtime:process — host process info. Run with:
//   FOO=bar esrun examples/modules/process.mjs one two three
import { env, args, platform, cwd, exit } from "runtime:process";

console.log("platform:", platform);
console.log("cwd:", cwd());
console.log("args:", args);
console.log("env.FOO:", env.FOO);

if (args.includes("--fail")) {
  console.error("failing as requested");
  exit(1); // sets the process exit code and halts immediately
}
