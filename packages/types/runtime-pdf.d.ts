declare module "runtime:pdf" {
  import type { FsFile } from "runtime:fs";

  /** What a PDF is read from. A path string is refused: pass `file(path)`. */
  export type PDFInput = ArrayBuffer | ArrayBufferView | Blob | FsFile;

  /** How a PDF is opened. */
  export interface PDFOpenOptions {
    /** Tried as the user password, then the owner password. */
    password?: string;
    /** Refuse a file that needs repair instead of repairing it. Default false. */
    strict?: boolean;
    /** Bytes one stream may decode to. Default 67108864 (64 MiB). */
    maxDecodedSize?: number;
    /** Stops work on this document when it aborts: the open and every later read. */
    signal?: AbortSignal;
  }

  /** What an encrypted file allows a reader without the owner password. */
  export interface PDFPermissions {
    print: boolean;
    modify: boolean;
    copy: boolean;
    annotate: boolean;
    fillForms: boolean;
    accessibility: boolean;
    assemble: boolean;
    printHighQuality: boolean;
  }

  /** The document information dictionary. A field the file does not state is absent. */
  export interface PDFInfo {
    title?: string;
    author?: string;
    subject?: string;
    keywords?: string;
    creator?: string;
    producer?: string;
    creationDate?: Date;
    modDate?: Date;
  }

  /** A repair made while opening a damaged file. */
  export interface PDFDiagnostic {
    kind: string;
    message: string;
    offset?: number;
  }

  /** `[left, bottom, right, top]` in points. */
  export type PDFBox = [number, number, number, number];

  export interface PDFPage {
    /** The displayed width in points: the crop box, rotation applied. */
    width: number;
    height: number;
    rotate: number;
    mediaBox: PDFBox;
    cropBox: PDFBox;
  }

  export interface PDFWord {
    text: string;
    bbox: PDFBox;
  }

  export interface PDFChar {
    text: string;
    /** Baseline start, `[x, y]` in points. */
    origin: [number, number];
    /** Baseline end: where the next character would start. */
    end: [number, number];
    /** Font size in points. */
    size: number;
    font: string;
    /** Drawn invisibly, as the text layer of a scanned page is. */
    invisible: boolean;
  }

  export interface PDFTextLine {
    text: string;
    bbox: PDFBox;
    words: PDFWord[];
    /** Present when `pageText` was asked for `{ chars: true }`. */
    chars?: PDFChar[];
  }

  export interface PDFPageText {
    text: string;
    lines: PDFTextLine[];
  }

  export interface PDFOutline {
    title: string;
    /** The page index, when the destination is a page in this document. */
    page?: number;
    open: boolean;
    children: PDFOutline[];
  }

  export interface PDFDestination {
    name: string;
    page?: number;
  }

  /** A bookmark for `setOutlines`. */
  export interface PDFBookmark {
    title: string;
    /** An index into the edit's pages at that point. */
    page?: number;
    /** Shown expanded. Default false. */
    open?: boolean;
    children?: PDFBookmark[];
  }

  /** What `sanitize` removes, counted. */
  export interface PDFSanitizeReport {
    javascript: number;
    externalActions: number;
    links: number;
    attachments: number;
    xfa: number;
    multimedia: number;
    metadata: number;
  }

  export interface PDFAttachment {
    name: string;
    filename?: string;
    description?: string;
    type?: string;
    size?: number;
  }

  /** AES-256 encryption for a saved PDF. */
  export interface PDFEncryptOptions {
    ownerPassword: string;
    /** Default `""`: opens without a password, with `permissions` applied. */
    userPassword?: string;
    /** Every permission not given is allowed. */
    permissions?: Partial<PDFPermissions>;
    /** Default true. */
    encryptMetadata?: boolean;
  }

  export interface PDFSaveOptions {
    /** Append the edits to the original bytes. Default false. */
    incremental?: boolean;
    /** Default `"1.7"`. */
    version?: "1.7" | "2.0";
    /** Default true. */
    objectStreams?: boolean;
    /** Default true. */
    compress?: boolean;
    /**
     * Required on a full save of an encrypted source: encryption options to
     * keep it encrypted, or `false` to write it decrypted.
     */
    encrypt?: PDFEncryptOptions | false;
    signal?: AbortSignal;
  }

  export interface PDFSanitizeOptions {
    /** Default true. */
    javascript?: boolean;
    /** Launch, submit-form, import and similar actions. Default true. */
    externalActions?: boolean;
    /** Default false. */
    links?: boolean;
    /** Default true. */
    attachments?: boolean;
    /** Default true. */
    xfa?: boolean;
    /** Default true. */
    multimedia?: boolean;
    /** Default false. */
    metadata?: boolean;
  }

  /** A text field, a date, or `null` to remove it. */
  export type PDFInfoUpdate = {
    [K in keyof PDFInfo]?: PDFInfo[K] | null;
  };

  /** An open PDF. Reads reuse what opening parsed; edits go through `edit()`. */
  export class PDF implements Disposable {
    private constructor();
    /**
     * Fails with `ERR_PDF_INVALID_PASSWORD`, `ERR_PDF_ENCRYPTION_UNSUPPORTED`,
     * `ERR_PDF_LIMIT` (with `limit`) or `ERR_PDF_MALFORMED`.
     */
    static open(input: PDFInput, options?: PDFOpenOptions): Promise<PDF>;
    readonly pageCount: number;
    readonly version: string | null;
    readonly encrypted: boolean;
    readonly permissions: Readonly<PDFPermissions> | null;
    readonly info: Readonly<PDFInfo>;
    readonly diagnostics: readonly PDFDiagnostic[];
    pages(): Promise<PDFPage[]>;
    /** Every page's text, pages separated by form feeds (`\f`). */
    text(): Promise<string>;
    pageText(index: number, options?: { chars?: boolean }): Promise<PDFPageText>;
    outlines(): Promise<PDFOutline[]>;
    destinations(): Promise<PDFDestination[]>;
    /** The XMP metadata packet, or undefined. */
    xmp(): Promise<string | undefined>;
    /** What `edit().sanitize(options)` would remove. Changes nothing. */
    sanitizeReport(options?: PDFSanitizeOptions): Promise<PDFSanitizeReport>;
    attachments(): Promise<PDFAttachment[]>;
    attachment(name: string): Promise<Uint8Array | undefined>;
    edit(): PDFEdit;
    close(): void;
    [Symbol.dispose](): void;
  }

  /** Edits to a PDF, applied at `bytes`, `blob` or `write`. Every method returns a new edit. */
  export class PDFEdit {
    private constructor();
    /** The number of pages after these edits. */
    readonly pageCount: number;
    select(pages: number | number[]): PDFEdit;
    delete(pages: number | number[]): PDFEdit;
    /** Clockwise, a multiple of 90, on top of the page's rotation. */
    rotate(pages: number | number[], degrees: number): PDFEdit;
    crop(pages: number | number[], box: PDFBox): PDFEdit;
    setMediaBox(pages: number | number[], box: PDFBox): PDFEdit;
    /** Replaces the bookmarks; an empty array removes them. */
    setOutlines(items: PDFBookmark[]): PDFEdit;
    append(other: PDF | PDFEdit, pages?: number | number[]): PDFEdit;
    setInfo(info: PDFInfoUpdate): PDFEdit;
    addAttachment(
      name: string,
      data: ArrayBuffer | ArrayBufferView,
      options?: { type?: string; description?: string },
    ): PDFEdit;
    removeAttachment(name: string): PDFEdit;
    sanitize(options?: PDFSanitizeOptions): PDFEdit;
    /** Fails with `ERR_PDF_INCREMENTAL_REFUSED` when an incremental save would be unsafe. */
    bytes(options?: PDFSaveOptions): Promise<Uint8Array>;
    blob(options?: PDFSaveOptions): Promise<Blob>;
    /** Resolves to the number of bytes written. */
    write(destination: FsFile, options?: PDFSaveOptions): Promise<number>;
  }

  const _default: { PDF: typeof PDF; PDFEdit: typeof PDFEdit };
  export default _default;
}
