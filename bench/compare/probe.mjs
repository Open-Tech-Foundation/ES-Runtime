// What one runtime actually has, for the capability tables on the comparison
// page (website/app/docs/comparison). Every answer comes from calling the API,
// not from reading documentation. Run by features.sh, which stages the files
// this imports and runs it on esrun, Node.js, Bun and Deno.
//
// Prints one JSON object: row name → "yes", "no", or "experimental" when the
// runtime said so while the probe ran (Node's ExperimentalWarning).

const rt = globalThis.Bun ? "bun" : globalThis.Deno ? "deno" : globalThis.process?.versions?.node ? "node" : "esrun";
const out = { runtime: rt };

const warnings = [];
globalThis.process?.on?.("warning", (w) => w.name === "ExperimentalWarning" && warnings.push(w.message));

async function probe(name, fn) {
  const before = warnings.length;
  let ok;
  try {
    ok = await fn();
  } catch {
    ok = false;
  }
  await new Promise((done) => setTimeout(done, 0)); // warnings are emitted on a later tick
  // A probe may answer "experimental" itself, for a flag the runtime does not warn about.
  out[name] = ok === "experimental" ? ok : !ok ? "no" : warnings.length > before ? "experimental" : "yes";
}

const http = rt === "esrun" ? await import("runtime:http") : null;
const hashing = rt === "esrun" ? await import("runtime:hashing") : null;
const bunKeys = rt === "bun" ? Object.keys(Bun).join(" ") : "";

// Language, modules and Web standards
await probe("temporal", () => Temporal.Now.instant() instanceof Temporal.Instant);
await probe("jsonImport", async () => (await import("./data.json", { with: { type: "json" } })).default.ok === true);
await probe("wasmImport", async () => (await import("./add.wasm")).add(2, 3) === 5);
await probe("wasi", async () => {
  const { WASI } = await import(rt === "esrun" ? "runtime:wasi" : "node:wasi");
  const wasi = new WASI({ version: "preview1", args: [], env: {} });
  const bytes = await readBytes("./start.wasm");
  const imports = wasi.getImportObject?.() ?? { wasi_snapshot_preview1: wasi.wasiImport };
  const { instance } = await WebAssembly.instantiate(bytes, imports);
  wasi.start(instance); // writes "ok\n" to stdout through fd_write
  return new Uint32Array(instance.exports.memory.buffer)[2] === 3; // bytes written
});

// HTTP: the helpers a server needs, in the runtime itself
await probe("cookieApi", () =>
  rt === "esrun" ? typeof http.CookieMap === "function" : rt === "bun" ? typeof Bun.CookieMap === "function" : typeof globalThis.CookieMap === "function");
await probe("signedCookies", () =>
  rt === "esrun" ? typeof http.CookieMap.prototype.setSigned === "function" && typeof http.CookieMap.prototype.setSealed === "function"
    : rt === "bun" ? /sign|seal/i.test(Object.getOwnPropertyNames(Bun.CookieMap.prototype).join(" ")) : false);
await probe("csrf", () => (rt === "esrun" ? typeof http.CSRF.generate === "function" : rt === "bun" ? typeof Bun.CSRF?.generate === "function" : false));
await probe("webhooks", () => (rt === "esrun" ? typeof http.Webhook === "function" : /webhook/i.test(bunKeys)));
await probe("totp", () => (rt === "esrun" ? typeof hashing.totp.generate === "function" : /totp|otp\b/i.test(bunKeys)));
await probe("udp", async () => {
  if (rt === "esrun") {
    const sock = (await import("runtime:net")).bind({ hostname: "127.0.0.1", port: 0 });
    sock.close();
    return true;
  }
  if (rt === "bun") {
    const sock = await Bun.udpSocket({});
    sock.close();
    return true;
  }
  const sock = (await import("node:dgram")).createSocket("udp4");
  await new Promise((ok) => sock.bind(0, "127.0.0.1", ok));
  sock.close();
  return true;
});

// QUIC, the transport HTTP/3 runs on. Deno's is behind --unstable-net, which
// features.sh passes, so finding it means unstable.
await probe("quicApi", async () =>
  rt === "deno" ? typeof Deno.QuicEndpoint === "function" && "experimental"
    : rt === "node" ? Object.keys(await import("node:quic")).length > 0 : false);

// Workers: the web Worker, or Node's worker_threads where there is none
await probe("workerUnref", async () => {
  if (rt === "node") {
    const { Worker } = await import("node:worker_threads");
    const w = new Worker("./worker.js");
    const has = typeof w.unref === "function";
    await w.terminate();
    return has;
  }
  const w = new Worker(new URL("./worker.js", import.meta.url), { type: "module" });
  const has = typeof w.unref === "function";
  w.terminate();
  return has;
});

// Node.js compatibility
await probe("nodeBuiltins", async () => typeof (await import("node:fs")).readFileSync === "function");
await probe("ffi", async () =>
  rt === "bun" ? typeof (await import("bun:ffi")).dlopen === "function"
    : rt === "deno" ? typeof Deno.dlopen === "function"
      : Object.keys(await import("node:ffi")).length > 0);
await probe("napi", () => typeof globalThis.process?.dlopen === "function");

// Permissions, from JavaScript
await probe("permissionQuery", async () =>
  rt === "esrun" ? typeof (await import("runtime:process")).permissions.has === "function"
    : rt === "deno" ? typeof Deno.permissions.query === "function" : typeof process.permission?.has === "function");
await probe("permissionDrop", async () =>
  rt === "esrun" ? typeof (await import("runtime:process")).permissions.drop === "function"
    : rt === "deno" ? typeof Deno.permissions.revoke === "function" : typeof process.permission?.drop === "function");

async function readBytes(path) {
  if (rt === "esrun") return (await import("runtime:fs")).file(path).bytes();
  if (rt === "deno") return Deno.readFile(path);
  return (await import("node:fs")).readFileSync(path);
}

console.log(JSON.stringify(out));
