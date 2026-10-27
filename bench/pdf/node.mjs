// Node.js, Bun and Deno — no runtime has PDF support built in, so each runs
// what a JavaScript application uses: pdf-lib to edit and pdf.js
// (pdfjs-dist) to read text.
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname } from "node:path";
import process from "node:process";
import { PDFDocument } from "pdf-lib";
import { getDocument } from "pdfjs-dist/legacy/build/pdf.mjs";
import { JOBS, WORKLOADS, count } from "./workload.mjs";

// pdf.js reads the standard 14 fonts' metrics from files it ships.
const fonts = `${dirname(createRequire(import.meta.url).resolve("pdfjs-dist/package.json"))}/standard_fonts/`;

const [workload, dir] = process.argv.slice(2);
const w = WORKLOADS[workload];
const bytes = await readFile(`${dir}/${w.input}`);

const run = {
  async text() {
    const task = getDocument({ data: new Uint8Array(bytes), isEvalSupported: false, disableFontFace: true, standardFontDataUrl: fonts });
    const doc = await task.promise;
    let text = "";
    for (let i = 1; i <= doc.numPages; i++) {
      const page = await doc.getPage(i);
      const content = await page.getTextContent();
      text += content.items.map((item) => item.str).join(" ") + "\f";
    }
    await task.destroy();
    return count(text, "invoice");
  },
  async merge() {
    const doc = await PDFDocument.load(bytes);
    const out = await PDFDocument.create();
    for (const source of [doc, doc]) {
      const pages = await out.copyPages(source, source.getPageIndices());
      for (const page of pages) out.addPage(page);
    }
    return (await PDFDocument.load(await out.save())).getPageCount();
  },
  async split() {
    const doc = await PDFDocument.load(bytes);
    const out = await PDFDocument.create();
    const first = Array.from({ length: w.pages }, (_, i) => i);
    for (const page of await out.copyPages(doc, first)) out.addPage(page);
    return (await PDFDocument.load(await out.save())).getPageCount();
  },
}[workload];

const results = await Promise.all(Array.from({ length: JOBS }, run));
console.log(results.reduce((a, b) => a + b, 0));
