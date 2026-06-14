// runtime:fs — modern, Blob-based file I/O. Run with:
//   esrun examples/modules/fs.mjs
// All access is confined to the project root jail; writes need FileWrite, reads
// FileRead (the esrun CLI grants both).
import { file, write, readDir, stat, mkdir, remove } from "runtime:fs";

await mkdir("tmp_demo", { recursive: true });
await write("tmp_demo/note.txt", "hello from runtime:fs");

const f = file("tmp_demo/note.txt");
console.log("text:", await f.text());
console.log("size:", (await stat("tmp_demo/note.txt")).size);
console.log("dir:", (await readDir("tmp_demo")).map((e) => e.name));

// Streaming + web bodies: write() takes any web body (string/Blob/Response/stream).
await write("tmp_demo/copy.txt", file("tmp_demo/note.txt"));

await remove("tmp_demo", { recursive: true });
console.log("cleaned up");
