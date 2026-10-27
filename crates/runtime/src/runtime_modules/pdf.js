// runtime:pdf — read, edit, encrypt and extract text from PDFs (DECISIONS D190).
//
// otf-pdf behind a handle: `PDF.open` parses once on the host and the reads
// reuse what it parsed. Edits are an immutable chain from `doc.edit()`; its
// terminal re-opens the same bytes on the host, applies the steps and saves,
// so one document can feed several outputs and a read never sees an edit.
//
//   import { PDF } from "runtime:pdf";
//   const doc = await PDF.open(upload);
//   const text = await doc.text();
//   const first = await doc.edit().select([0]).bytes();
//
// Input is bytes, a Blob, or a runtime:fs file() — never a path string, which
// would read a file without passing the filesystem provider. Bytes in and out
// need no capability; a file() source needs FileRead and a file() destination
// FileWrite, exactly as reading or writing that file would.

const ops = globalThis.__ops;
const FILE_BODY = globalThis.__internal.fileBody;
const BLOB_BYTES = globalThis.__internal.bytes;

// Passed as the constructor's first argument; module-private, so guest code
// cannot build a document around a handle it did not open.
const INTERNAL = Symbol("internal");

// otf-pdf's failure names, as ERR_PDF_* codes.
const ERROR_CODES = {
  invalid_password: "ERR_PDF_INVALID_PASSWORD",
  unsupported_encryption: "ERR_PDF_ENCRYPTION_UNSUPPORTED",
  limit: "ERR_PDF_LIMIT",
  malformed: "ERR_PDF_MALFORMED",
  incremental_refused: "ERR_PDF_INCREMENTAL_REFUSED",
};

const PERMISSIONS = [
  "print",
  "modify",
  "copy",
  "annotate",
  "fillForms",
  "accessibility",
  "assemble",
  "printHighQuality",
];
const INFO_TEXT = ["title", "author", "subject", "keywords", "creator", "producer"];
const INFO_DATES = ["creationDate", "modDate"];
const SANITIZE = ["javascript", "externalActions", "links", "attachments", "xfa", "multimedia", "metadata"];

// A document's handle and cancel token are released when it is collected
// without close(). The held value names them, never the document.
const handles = new FinalizationRegistry(({ handle, token }) => {
  ops.pdf_close(handle);
  if (token !== null) ops.pdf_cancel_release(token);
});

function settle(result, signal) {
  if (!("error" in result)) return result.ok;
  if (result.error === "aborted" && signal?.aborted) throw signal.reason;
  const err = new Error(result.message);
  const code = ERROR_CODES[result.error];
  if (code !== undefined) err.code = code;
  if (result.limit !== undefined) err.limit = result.limit;
  throw err;
}

function optionsObject(options, what) {
  if (options === undefined) return {};
  if (options === null || typeof options !== "object") {
    throw new TypeError(`${what} options must be an object`);
  }
  return options;
}

function isFile(value) {
  return value !== null && typeof value === "object" && typeof value[FILE_BODY] === "function";
}

// The input, as either `{ bytes }` or `{ path }`. A view is kept, not copied:
// the op call copies it, so the bytes read are the ones there at the call.
function sourceOf(input) {
  if (typeof input === "string" || input instanceof URL) {
    throw new TypeError("PDF.open takes bytes, a Blob or a file(); pass file(path) from runtime:fs to read a file");
  }
  if (isFile(input)) return { path: input[FILE_BODY]() };
  if (input instanceof Blob) return { bytes: input[BLOB_BYTES]() };
  return { bytes: bytesOf(input, "PDF.open") };
}

function bytesOf(input, what) {
  let buffer;
  if (input instanceof ArrayBuffer) buffer = input;
  else if (ArrayBuffer.isView(input)) buffer = input.buffer;
  else throw new TypeError(`${what} takes a Uint8Array, an ArrayBuffer, a view, a Blob or a file()`);
  if (typeof SharedArrayBuffer === "function" && buffer instanceof SharedArrayBuffer) {
    throw new TypeError(`${what} cannot read a SharedArrayBuffer, which may change while it is read`);
  }
  if (input instanceof ArrayBuffer) return new Uint8Array(input);
  return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
}

function signalOf(options) {
  const signal = options.signal;
  if (signal === undefined) return undefined;
  if (!(signal instanceof AbortSignal)) throw new TypeError("signal must be an AbortSignal");
  return signal;
}

// A host cancel token that `signal` sets when it aborts, or null.
function tokenFor(signal) {
  if (signal === undefined) return null;
  const token = ops.pdf_cancel_token();
  signal.addEventListener("abort", () => ops.pdf_cancel(token), { once: true });
  return token;
}

function openOptions(options) {
  const o = optionsObject(options, "PDF.open");
  const open = {};
  if (o.password !== undefined) {
    if (typeof o.password !== "string") throw new TypeError("password must be a string");
    open.password = o.password;
  }
  if (o.strict !== undefined) open.strict = Boolean(o.strict);
  if (o.maxDecodedSize !== undefined) {
    if (!Number.isSafeInteger(o.maxDecodedSize) || o.maxDecodedSize < 1) {
      throw new RangeError("maxDecodedSize must be a positive integer");
    }
    open.maxDecodedSize = o.maxDecodedSize;
  }
  return open;
}

// PDF dates are `D:YYYYMMDDHHmmSSOHH'mm'`, every part after the year optional.
const PDF_DATE = /^(?:D:)?(\d{4})(\d{2})?(\d{2})?(\d{2})?(\d{2})?(\d{2})?(?:([Zz+-])(?:(\d{2})'?(\d{2})?'?)?)?$/;

function parseDate(text) {
  const m = PDF_DATE.exec(text.trim());
  if (m === null) return undefined;
  const [, y, mo = "01", d = "01", h = "00", mi = "00", s = "00", sign, oh = "00", om = "00"] = m;
  const utc = Date.UTC(+y, +mo - 1, +d, +h, +mi, +s);
  const offset = sign === "+" || sign === "-" ? (sign === "+" ? 1 : -1) * (+oh * 60 + +om) : 0;
  const date = new Date(utc - offset * 60_000);
  return Number.isNaN(date.getTime()) ? undefined : date;
}

function formatDate(date) {
  if (!(date instanceof Date) || Number.isNaN(date.getTime())) {
    throw new TypeError("a PDF date must be a valid Date");
  }
  const p = (n, w = 2) => String(n).padStart(w, "0");
  return `D:${p(date.getUTCFullYear(), 4)}${p(date.getUTCMonth() + 1)}${p(date.getUTCDate())}${p(date.getUTCHours())}${p(date.getUTCMinutes())}${p(date.getUTCSeconds())}Z`;
}

function infoOf(raw) {
  const info = {};
  for (const key of INFO_TEXT) if (raw[key] !== undefined) info[key] = raw[key];
  for (const key of INFO_DATES) {
    if (raw[key] === undefined) continue;
    const date = parseDate(raw[key]);
    if (date !== undefined) info[key] = date;
  }
  return Object.freeze(info);
}

function pageList(pages, count, what) {
  const list = typeof pages === "number" ? [pages] : pages;
  if (!Array.isArray(list)) throw new TypeError(`${what} takes a page index or an array of them`);
  for (const index of list) checkIndex(index, count);
  return [...list];
}

function checkIndex(index, count) {
  if (!Number.isInteger(index) || index < 0 || index >= count) {
    throw new RangeError(`page index ${index} is out of range (the document has ${count} pages)`);
  }
}

function sanitizeOptions(options) {
  const o = optionsObject(options, "sanitize");
  const chosen = {};
  for (const [key, value] of Object.entries(o)) {
    if (!SANITIZE.includes(key)) throw new TypeError(`sanitize does not know ${JSON.stringify(key)}`);
    chosen[key] = Boolean(value);
  }
  return chosen;
}

function boxOf(box, what) {
  if (!Array.isArray(box) || box.length !== 4 || !box.every(Number.isFinite)) {
    throw new TypeError(`${what} must be [left, bottom, right, top]`);
  }
  return [...box];
}

// Matches the host's nesting bound.
const MAX_BOOKMARK_DEPTH = 256;

function bookmarkOf(item, count, depth) {
  if (depth >= MAX_BOOKMARK_DEPTH) throw new RangeError("bookmarks nest too deeply");
  if (item === null || typeof item !== "object") throw new TypeError("a bookmark must be an object");
  if (typeof item.title !== "string") throw new TypeError("a bookmark needs a title string");
  const bookmark = { title: item.title, open: item.open === true, children: [] };
  if (item.page !== undefined && item.page !== null) {
    checkIndex(item.page, count);
    bookmark.page = item.page;
  }
  if (item.children !== undefined) {
    if (!Array.isArray(item.children)) throw new TypeError("bookmark children must be an array");
    bookmark.children = item.children.map((child) => bookmarkOf(child, count, depth + 1));
  }
  return bookmark;
}

/**
 * An open PDF. Reads reuse what opening parsed; edits go through `edit()`,
 * which never changes this document.
 */
export class PDF {
  #handle;
  #token;
  #signal;
  #closed = false;

  constructor(key, opened, signal, token) {
    if (key !== INTERNAL) throw new TypeError("a PDF is made by PDF.open()");
    this.#handle = opened.handle;
    this.#signal = signal;
    this.#token = token;
    /** The number of pages. */
    this.pageCount = opened.pageCount;
    /** The version the file declares, such as `"1.7"`, or null. */
    this.version = opened.version;
    /** Whether the file is encrypted (it opened, so a password matched or none was needed). */
    this.encrypted = opened.encrypted;
    /** What an encrypted file allows a reader without the owner password, or null. */
    this.permissions = opened.permissions === null ? null : Object.freeze(opened.permissions);
    /** The document information: title, author, … and dates as `Date`s. */
    this.info = infoOf(opened.info);
    /** Every repair made while opening a damaged file: `{ kind, message, offset? }`. */
    this.diagnostics = Object.freeze(opened.diagnostics.map(Object.freeze));
    Object.freeze(this);
    handles.register(this, { handle: this.#handle, token }, this);
  }

  /**
   * Opens `input`: a Uint8Array, an ArrayBuffer, any view, a Blob, or a
   * runtime:fs `file()`. Options: `password`, `strict` (refuse a file that
   * needs repair; default false), `maxDecodedSize` (bytes one stream may
   * decode to; default 64 MiB), and `signal`, which stops work on this
   * document when it aborts — the open and every later read.
   */
  static async open(input, options) {
    const source = sourceOf(input);
    const o = optionsObject(options, "PDF.open");
    const open = openOptions(o);
    const signal = signalOf(o);
    signal?.throwIfAborted();
    const token = tokenFor(signal);
    let result;
    try {
      result =
        source.path !== undefined
          ? await ops.pdf_open_file(source.path, open, token)
          : await ops.pdf_open(source.bytes, open, token);
      return new PDF(INTERNAL, settle(result, signal), signal, token);
    } catch (err) {
      if (token !== null) ops.pdf_cancel_release(token);
      throw err;
    }
  }

  // The handle, or a TypeError once closed.
  get [INTERNAL]() {
    if (this.#closed) throw new TypeError("this PDF has been closed");
    return this.#handle;
  }

  async #read(op, ...args) {
    const handle = this[INTERNAL];
    this.#signal?.throwIfAborted();
    return settle(await ops[op](handle, ...args), this.#signal);
  }

  /** Each page's displayed `width` and `height` (points, rotation applied), `rotate`, `mediaBox` and `cropBox`. */
  async pages() {
    return this.#read("pdf_pages");
  }

  /** The text of every page, in content order, pages separated by form feeds (`\f`). */
  async text() {
    return this.#read("pdf_text");
  }

  /**
   * One page's `text` and its `lines`, each with a `bbox` and `words`
   * (`{ text, bbox }`). With `{ chars: true }` each line also has `chars`:
   * `{ text, origin, end, size, font, invisible }`.
   */
  async pageText(index, options) {
    checkIndex(index, this.pageCount);
    const o = optionsObject(options, "pageText");
    return this.#read("pdf_page_text", index, o.chars === true);
  }

  /** The bookmarks: `{ title, page?, open, children }`; `page` when the destination is a page here. */
  async outlines() {
    return this.#read("pdf_outlines");
  }

  /** Named destinations: `{ name, page? }`, `page` when it is a page here. */
  async destinations() {
    return this.#read("pdf_destinations");
  }

  /** The XMP metadata packet as text, or undefined when there is none. */
  async xmp() {
    return this.#read("pdf_xmp");
  }

  /**
   * What `edit().sanitize(options)` would remove, counted:
   * `{ javascript, externalActions, links, attachments, xfa, multimedia, metadata }`.
   * Nothing is changed.
   */
  async sanitizeReport(options) {
    return this.#read("pdf_sanitize_report", sanitizeOptions(options));
  }

  /** Embedded files: `{ name, filename?, description?, type?, size? }`. */
  async attachments() {
    return this.#read("pdf_attachments");
  }

  /** An embedded file's bytes, or undefined when there is none of that name. */
  async attachment(name) {
    if (typeof name !== "string") throw new TypeError("an attachment name must be a string");
    return this.#read("pdf_attachment", name);
  }

  /** An edit of this document. Nothing changes until its `bytes`, `blob` or `write`. */
  edit() {
    this[INTERNAL];
    return new PDFEdit(INTERNAL, this, [], this.pageCount, this.encrypted);
  }

  /** Releases the document. Later reads, and edits not yet saved, reject. */
  close() {
    if (this.#closed) return;
    this.#closed = true;
    handles.unregister(this);
    ops.pdf_close(this.#handle);
    if (this.#token !== null) ops.pdf_cancel_release(this.#token);
  }

  [Symbol.dispose]() {
    this.close();
  }

  get [Symbol.toStringTag]() {
    return "PDF";
  }
}

/**
 * Edits to a PDF, recorded and applied on the host at the terminal. Every
 * method returns a new `PDFEdit`.
 */
export class PDFEdit {
  #doc;
  #steps;
  #count;
  #encrypted;

  constructor(key, doc, steps, count, encrypted) {
    if (key !== INTERNAL) throw new TypeError("a PDFEdit is made by pdf.edit()");
    this.#doc = doc;
    this.#steps = steps;
    this.#count = count;
    this.#encrypted = encrypted;
    /** The number of pages after these edits. */
    this.pageCount = count;
    Object.freeze(this);
  }

  #with(step, count = this.#count, encrypted = this.#encrypted) {
    return new PDFEdit(INTERNAL, this.#doc, [...this.#steps, step], count, encrypted);
  }

  /** Keeps these pages, in this order; a page listed twice is duplicated. */
  select(pages) {
    const list = pageList(pages, this.#count, "select");
    return this.#with(["select", list], list.length);
  }

  /** Removes these pages, with the bookmarks and named destinations that point to them. */
  delete(pages) {
    const list = pageList(pages, this.#count, "delete");
    return this.#with(["delete", list], this.#count - new Set(list).size);
  }

  /** Turns these pages clockwise by `degrees`, a multiple of 90, on top of their rotation. */
  rotate(pages, degrees) {
    const list = pageList(pages, this.#count, "rotate");
    if (!Number.isInteger(degrees) || degrees % 90 !== 0) {
      throw new RangeError("rotation must be a multiple of 90");
    }
    return this.#with(["rotate", list, degrees]);
  }

  /** Sets these pages' visible region, `[left, bottom, right, top]` in points. */
  crop(pages, box) {
    const list = pageList(pages, this.#count, "crop");
    return this.#with(["crop", list, boxOf(box, "a crop box")]);
  }

  /** Sets these pages' physical size, `[left, bottom, right, top]` in points. */
  setMediaBox(pages, box) {
    const list = pageList(pages, this.#count, "setMediaBox");
    return this.#with(["mediaBox", list, boxOf(box, "a media box")]);
  }

  /**
   * Replaces the bookmarks: `[{ title, page?, open?, children? }]`, where
   * `page` is an index into the pages this edit has at this point. An empty
   * array removes them.
   */
  setOutlines(items) {
    if (!Array.isArray(items)) throw new TypeError("setOutlines takes an array of bookmarks");
    return this.#with(["setOutlines", items.map((item) => bookmarkOf(item, this.#count, 0))]);
  }

  /** Appends pages of another PDF or PDFEdit — all of them, or the indices given. */
  append(other, pages) {
    let nested;
    let count;
    let encrypted;
    if (other instanceof PDF) {
      nested = { doc: other, steps: [] };
      count = other.pageCount;
      encrypted = other.encrypted;
    } else if (other instanceof PDFEdit) {
      nested = other.#nested();
      count = other.#count;
      encrypted = other.#encrypted;
    } else {
      throw new TypeError("append takes a PDF or a PDFEdit");
    }
    const list = pages === undefined ? null : pageList(pages, count, "append");
    const added = list === null ? count : list.length;
    return this.#with(["append", nested, list], this.#count + added, this.#encrypted || encrypted);
  }

  /** Sets document information. Text fields take strings, dates take `Date`s; `null` removes one. */
  setInfo(info) {
    if (info === null || typeof info !== "object") throw new TypeError("setInfo takes an object");
    const fields = {};
    for (const [key, value] of Object.entries(info)) {
      if (INFO_TEXT.includes(key)) {
        if (value !== null && typeof value !== "string") throw new TypeError(`${key} must be a string or null`);
        fields[key] = value;
      } else if (INFO_DATES.includes(key)) {
        fields[key] = value === null ? null : formatDate(value);
      } else {
        throw new TypeError(`setInfo does not know ${JSON.stringify(key)}`);
      }
    }
    return this.#with(["setInfo", fields]);
  }

  /** Embeds a file, replacing one of the same name. Options: `type` (a media type) and `description`. */
  addAttachment(name, data, options) {
    if (typeof name !== "string" || name === "") throw new TypeError("an attachment name must be a non-empty string");
    const o = optionsObject(options, "addAttachment");
    for (const key of ["type", "description"]) {
      if (o[key] !== undefined && typeof o[key] !== "string") throw new TypeError(`${key} must be a string`);
    }
    // Copied now, so changing the buffer afterwards cannot change the file.
    const bytes = bytesOf(data, "addAttachment").slice();
    return this.#with(["addAttachment", name, bytes, { type: o.type, description: o.description }]);
  }

  /** Removes an embedded file. */
  removeAttachment(name) {
    if (typeof name !== "string") throw new TypeError("an attachment name must be a string");
    return this.#with(["removeAttachment", name]);
  }

  /**
   * Removes active and hidden content. By default JavaScript, external
   * actions, attachments, XFA forms and multimedia; `links` and `metadata`
   * are kept unless set. Each option is a boolean.
   */
  sanitize(options) {
    return this.#with(["sanitize", sanitizeOptions(options)]);
  }

  // `{ handle, steps }` for the host, with every nested document's handle.
  #nested() {
    return { doc: this.#doc, steps: this.#steps };
  }

  static #wire({ doc, steps }) {
    return {
      handle: doc[INTERNAL],
      steps: steps.map((step) =>
        step[0] === "append" ? ["append", PDFEdit.#wire(step[1]), step[2]] : step,
      ),
    };
  }

  #save(options) {
    const o = optionsObject(options, "save");
    const save = {};
    if (o.incremental !== undefined) save.incremental = Boolean(o.incremental);
    if (o.objectStreams !== undefined) save.objectStreams = Boolean(o.objectStreams);
    if (o.compress !== undefined) save.compress = Boolean(o.compress);
    if (o.version !== undefined) {
      if (o.version !== "1.7" && o.version !== "2.0") {
        throw new RangeError('version must be "1.7" or "2.0"');
      }
      save.version = o.version;
    }
    if (o.encrypt !== undefined && o.encrypt !== false) {
      const e = optionsObject(o.encrypt, "encrypt");
      if (typeof e.ownerPassword !== "string" || e.ownerPassword === "") {
        throw new TypeError("encrypt needs a non-empty ownerPassword");
      }
      if (e.userPassword !== undefined && typeof e.userPassword !== "string") {
        throw new TypeError("userPassword must be a string");
      }
      const permissions = {};
      for (const [key, value] of Object.entries(optionsObject(e.permissions, "permissions"))) {
        if (!PERMISSIONS.includes(key)) throw new TypeError(`permissions does not know ${JSON.stringify(key)}`);
        permissions[key] = Boolean(value);
      }
      save.encrypt = {
        userPassword: e.userPassword ?? "",
        ownerPassword: e.ownerPassword,
        permissions,
        encryptMetadata: e.encryptMetadata !== false,
      };
    }
    // A full save writes plaintext unless told to encrypt; an encrypted
    // source must say which it wants (D190).
    if (this.#encrypted && !save.incremental && o.encrypt === undefined) {
      throw new TypeError(
        "this PDF was encrypted: pass encrypt: { ownerPassword, … } to keep it encrypted, or encrypt: false to write it decrypted",
      );
    }
    return { save, signal: signalOf(o) };
  }

  async #run(options, destination) {
    const { save, signal } = this.#save(options);
    const wire = PDFEdit.#wire(this.#nested());
    signal?.throwIfAborted();
    const token = tokenFor(signal);
    try {
      const result =
        destination === undefined
          ? await ops.pdf_save(wire.handle, wire.steps, save, token)
          : await ops.pdf_save_file(wire.handle, wire.steps, save, token, destination);
      return settle(result, signal);
    } finally {
      if (token !== null) ops.pdf_cancel_release(token);
    }
  }

  /**
   * The edited PDF's bytes. Options: `incremental` (append the edits to the
   * original bytes), `version` (`"1.7"` or `"2.0"`), `objectStreams`,
   * `compress`, `encrypt` (`{ ownerPassword, userPassword?, permissions?,
   * encryptMetadata? }` for AES-256, or `false`) and `signal`.
   */
  async bytes(options) {
    return this.#run(options);
  }

  /** The edited PDF as a Blob of type `application/pdf`. */
  async blob(options) {
    return new Blob([await this.#run(options)], { type: "application/pdf" });
  }

  /** Writes the edited PDF to `destination`, a runtime:fs `file()`; resolves to the bytes written. */
  async write(destination, options) {
    if (!isFile(destination)) throw new TypeError("write takes a file() from runtime:fs");
    return this.#run(options, destination[FILE_BODY]());
  }

  get [Symbol.toStringTag]() {
    return "PDFEdit";
  }
}

export default { PDF, PDFEdit };
