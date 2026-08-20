// The added runtime:fs surface against a real filesystem, under the real root
// jail. Everything happens in a temp directory this run creates and removes.
import {
  chmod,
  copy,
  file,
  makeTempDir,
  makeTempFile,
  readLink,
  realPath,
  remove,
  symlink,
  truncate,
  write,
} from "runtime:fs";
import { platform } from "runtime:process";

const dir = await makeTempDir({ prefix: "surface-" });
console.log(`TEMPDIR named:${dir.includes("surface-")}`);

// Two calls must not collide, or "temp" means nothing.
const other = await makeTempDir({ prefix: "surface-" });
console.log(`TEMPDIR unique:${dir !== other}`);

const tmpFile = await makeTempFile({ dir, prefix: "f-" });
console.log(`TEMPFILE inside:${tmpFile.startsWith(dir)}`);

await write(`${dir}/src.txt`, "hello world");
const copied = await copy(`${dir}/src.txt`, `${dir}/dst.txt`);
console.log(`COPY bytes:${copied} same:${(await file(`${dir}/dst.txt`).text()) === "hello world"}`);

await truncate(`${dir}/dst.txt`, 5);
console.log(`TRUNCATE text:${await file(`${dir}/dst.txt`).text()}`);

const real = await realPath(`${dir}/./sub/../src.txt`.replace("/sub/..", ""));
console.log(`REALPATH clean:${!real.includes("..") && real.endsWith("src.txt")}`);

// chmod is a Unix mode; Windows can only represent the owner-write bit.
await chmod(`${dir}/src.txt`, 0o600);
console.log(`CHMOD ok:true`);

// A missing target has no honest realPath.
let missing = "none";
try {
  await realPath(`${dir}/nope`);
} catch (e) {
  missing = e.code ?? "threw";
}
console.log(`REALPATH missing:${missing}`);

// readLink needs an actual link; skip where making one needs privileges.
if (platform !== "windows") {
  let target = "unsupported";
  try {
    target = await readLink(`${dir}/src.txt`); // not a link
  } catch {
    target = "not-a-link";
  }
  console.log(`READLINK plain:${target}`);
}

// A link this run makes itself: the target is stored verbatim, the link reads
// through to the file, and the same call twice does not overwrite.
if (platform !== "windows") {
  await symlink("src.txt", `${dir}/link.txt`);
  const stored = await readLink(`${dir}/link.txt`);
  const through = await file(`${dir}/link.txt`).text();
  console.log(`SYMLINK stored:${stored} through:${through === "hello world"}`);

  let again = "overwrote";
  try {
    await symlink("dst.txt", `${dir}/link.txt`);
  } catch (e) {
    again = e.code ?? "threw";
  }
  console.log(`SYMLINK exists:${again}`);

  // A link may be written pointing out of the jail — it is data, and it is what
  // makes a dependency resolving outside the project reproducible — and it
  // still cannot be followed out.
  await symlink("/etc", `${dir}/out`);
  let followed = "read-through";
  try {
    await file(`${dir}/out/hostname`).text();
  } catch (e) {
    followed = e.code ?? "threw";
  }
  console.log(`SYMLINK outward:${await readLink(`${dir}/out`)} followed:${followed}`);
}

// A write that has resolved must be readable in full. Over 64 KiB this takes
// the async path, which used to resolve while the bytes were still in flight —
// so the read below saw an empty or half-written file. Repeated, because it was
// a race that one attempt could pass by luck.
const big = "x".repeat(300_000);
let torn = 0;
for (let attempt = 0; attempt < 10; attempt++) {
  const path = `${dir}/big-${attempt}.bin`;
  await write(path, big);
  if ((await file(path).text()).length !== big.length) torn += 1;
}
console.log(`WRITE readable-in-full:${torn === 0} torn:${torn}`);

await remove(dir, { recursive: true });
await remove(other, { recursive: true });
console.log("FS_SURFACE_OK");
