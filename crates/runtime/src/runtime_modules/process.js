// runtime:process — host process info (DECISIONS D24), aligned in spirit with
// the WinterTC CLI-API proposal. An ES module (not a global), backed by ops
// gated on Capability::Env. Values are snapshotted when the module evaluates.

const ops = globalThis.__ops;

// `env`: a mutable in-process object seeded from the host snapshot. Reads,
// writes, and deletes work in-process; they do not (yet) propagate to the host
// process or future child processes.
const env = {};
for (const [key, value] of ops.process_env()) env[key] = value;

// `args`: the program arguments after the runtime binary and the script/-e code.
const args = Object.freeze(ops.process_args());

// `platform`: the host OS — std::env::consts::OS values ("linux"/"macos"/...).
const platform = ops.process_platform();

// `arch`: the host CPU architecture — std::env::consts::ARCH values
// ("x86_64"/"aarch64"/"arm"/...).
const arch = ops.process_arch();

// `cwd()`: the current working directory (a function — it can change).
function cwd() {
  return ops.process_cwd();
}

// `exit(code = 0)`: record the exit code and halt execution.
function exit(code = 0) {
  ops.process_exit(Number(code) | 0);
}

export { env, args, platform, arch, cwd, exit };
export default { env, args, platform, arch, cwd, exit };
