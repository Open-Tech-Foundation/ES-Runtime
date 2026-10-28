// A type test for `runtime:pdf`. Compiled by `tsc -p .`, never run.

import { file } from "runtime:fs";
import { PDF, type PDFEdit, type PDFOutline, type PDFPageText } from "runtime:pdf";

const controller = new AbortController();
const doc: PDF = await PDF.open(new Uint8Array(0), {
  password: "secret",
  strict: false,
  maxDecodedSize: 1_000_000,
  signal: controller.signal,
});
await PDF.open(file("in.pdf"));
await PDF.open(new Blob([]));

const count: number = doc.pageCount;
const version: string | null = doc.version;
const copy: boolean | undefined = doc.permissions?.copy;
const created: Date | undefined = doc.info.creationDate;
const kind: string | undefined = doc.diagnostics[0]?.kind;
const text: string = await doc.text();
const page: PDFPageText = await doc.pageText(0);
const word: string | undefined = page.lines[0]?.words[0]?.text;
const outlines: PDFOutline[] = await doc.outlines();
const font: string | undefined = (await doc.pageText(0, { chars: true })).lines[0]?.chars?.[0]
  ?.font;
const named: string | undefined = (await doc.destinations())[0]?.name;
const xmp: string | undefined = await doc.xmp();
const scripts: number = (await doc.sanitizeReport({ links: true })).javascript;
const data: Uint8Array | undefined = await doc.attachment("a.csv");

const edit: PDFEdit = doc
  .edit()
  .select([2, 0])
  .delete(1)
  .rotate([0], 90)
  .crop(0, [0, 0, 100, 100])
  .setMediaBox([0], [0, 0, 595, 842])
  .setOutlines([{ title: "Intro", page: 0, open: true, children: [{ title: "Part" }] }])
  .append(doc, [0])
  .append(doc.edit())
  .setInfo({ title: "T", author: null, modDate: new Date() })
  .addAttachment("a.csv", new Uint8Array(1), { type: "text/csv" })
  .removeAttachment("b.csv")
  .sanitize({ links: true });
const bytes: Uint8Array = await edit.bytes({
  incremental: false,
  version: "2.0",
  encrypt: { ownerPassword: "o", permissions: { copy: false } },
});
await edit.bytes({ encrypt: false });
const blob: Blob = await edit.blob();
const written: number = await edit.write(file("out.pdf"), { signal: controller.signal });
doc.close();

// @ts-expect-error a path string is not an input
await PDF.open("in.pdf");
// @ts-expect-error versions are 1.7 or 2.0
await edit.bytes({ version: "1.4" });
// @ts-expect-error the constructor is private
new PDF();

export {
  blob,
  bytes,
  copy,
  count,
  created,
  data,
  font,
  kind,
  named,
  outlines,
  scripts,
  text,
  version,
  word,
  written,
  xmp,
};
