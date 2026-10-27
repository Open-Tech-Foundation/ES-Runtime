// The PDF workloads every runtime runs. Each is JOBS documents handled at
// once, as a server meets uploads, and prints a checksum the runner compares,
// so a runtime cannot look fast by doing less: the "invoice" count for text,
// the output's page count for merge and split.
export const JOBS = 8;

export const WORKLOADS = {
  // Every page's text from a 200-page report.
  text: { input: "report.pdf" },
  // The report appended to itself: 400 pages out.
  merge: { input: "report.pdf" },
  // Its first ten pages as a document of their own.
  split: { input: "report.pdf", pages: 10 },
};

export const count = (text, word) => text.split(word).length - 1;
