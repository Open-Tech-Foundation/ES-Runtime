// A type test for `runtime:archive`. Compiled by `tsc -p .`, never run.

import { file } from "runtime:fs";
import { Archive, ArchiveEntry, type ArchiveEntryMetadata, type ExtractResult } from "runtime:archive";

const built = new Archive(
  {
    "hello.txt": "Hello",
    "bytes.bin": new Uint8Array([1, 2, 3]),
    "blob.txt": new Blob(["x"]),
    "nested/file.txt": new File(["y"], "file.txt", { lastModified: 1 }),
    "empty/": null,
    "stamped.txt": { data: "z", lastModified: new Date() },
  },
  { format: "zip", compress: "deflate", maxEntries: 1000, maxBytes: 1_000_000 },
);

const gz = new Archive({ "hello.txt": "Hello" }, { compress: "gzip" });
const opened = new Archive(file("bundle.tar.gz"));
const fromBytes = new Archive(new Uint8Array(0), { maxEntries: 10 });

async function use() {
  const bytes: Uint8Array = await built.bytes();
  const blob: Blob = await gz.blob();
  const entries: ArchiveEntry[] = await opened.entries();
  const meta: ArchiveEntryMetadata = entries[0];
  for await (const entry of opened) {
    const streamed: ArchiveEntry = entry;
    void streamed.path;
  }
  const first: Uint8Array = await entries[0].bytes();
  const text: string = await entries[0].text();
  const entryBlob: Blob = await entries[0].blob();
  const stream: ReadableStream<Uint8Array> = entries[0].stream();

  const all: Map<string, File> = await fromBytes.files();
  const prefixed: Map<string, File> = await built.files("nested/");
  const matching: Map<string, File> = await built.files(/\.txt$/);
  const filtered: Map<string, File> = await built.files((path) => path.endsWith(".txt"));

  const extracted: ExtractResult = await opened.extract("out");
  const extractedToFile: ExtractResult = await opened.extract(file("out"), { maxBytes: 10 });

  const written: number = await Archive.write(file("out.zip"), built);
  const writtenAgain: number = await Archive.write("out.tar", {
    "hello.txt": "Hello",
  });

  return [
    bytes,
    blob,
    meta.lastModified,
    first,
    text,
    entryBlob,
    stream,
    all,
    prefixed,
    matching,
    filtered,
    extracted.files,
    extractedToFile.directories,
    written,
    writtenAgain,
  ];
}

// @ts-expect-error — a path string is refused: pass file(path).
new Archive("bundle.tar.gz");
// @ts-expect-error — not a format.
new Archive({}, { format: "rar" });
void use;
