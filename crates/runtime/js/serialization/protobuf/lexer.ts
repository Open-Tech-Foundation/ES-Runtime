// Tokenizer for .proto source. Emits identifiers, numbers (raw), string
// literals, and single-character symbols; skips whitespace and // and /* */
// comments.

export interface Token {
  kind: "ident" | "num" | "str" | "sym" | "eof";
  value: string;
  line: number;
}

const IDENT_START = /[A-Za-z_]/;
const IDENT_PART = /[A-Za-z0-9_]/;
const DIGIT = /[0-9]/;

export class Lexer {
  private src: string;
  private pos = 0;
  private line = 1;
  private peeked: Token | null = null;

  constructor(src: string) {
    this.src = src;
  }

  peek(): Token {
    return (this.peeked ??= this.scan());
  }

  next(): Token {
    if (this.peeked) {
      const t = this.peeked;
      this.peeked = null;
      return t;
    }
    return this.scan();
  }

  private scan(): Token {
    this.skipTrivia();
    if (this.pos >= this.src.length) return { kind: "eof", value: "", line: this.line };
    const ch = this.src[this.pos]!;
    const line = this.line;

    if (IDENT_START.test(ch)) {
      const start = this.pos;
      while (this.pos < this.src.length && IDENT_PART.test(this.src[this.pos]!)) this.pos++;
      return { kind: "ident", value: this.src.slice(start, this.pos), line };
    }

    if (DIGIT.test(ch) || (ch === "." && DIGIT.test(this.src[this.pos + 1] ?? ""))) {
      return { kind: "num", value: this.scanNumber(), line };
    }

    if (ch === '"' || ch === "'") {
      return { kind: "str", value: this.scanString(ch), line };
    }

    this.pos++;
    return { kind: "sym", value: ch, line };
  }

  private scanNumber(): string {
    const start = this.pos;
    // hex / octal / decimal / float — keep it permissive; parse() interprets.
    if (this.src[this.pos] === "0" && (this.src[this.pos + 1] === "x" || this.src[this.pos + 1] === "X")) {
      this.pos += 2;
      while (this.pos < this.src.length && /[0-9a-fA-F]/.test(this.src[this.pos]!)) this.pos++;
      return this.src.slice(start, this.pos);
    }
    while (this.pos < this.src.length && /[0-9.eE+\-]/.test(this.src[this.pos]!)) {
      // stop a trailing sign that isn't part of an exponent
      const c = this.src[this.pos]!;
      if ((c === "+" || c === "-") && !/[eE]/.test(this.src[this.pos - 1] ?? "")) break;
      this.pos++;
    }
    return this.src.slice(start, this.pos);
  }

  private scanString(quote: string): string {
    this.pos++; // opening quote
    let out = "";
    while (this.pos < this.src.length) {
      const c = this.src[this.pos++]!;
      if (c === quote) return out;
      if (c === "\\") {
        const e = this.src[this.pos++]!;
        switch (e) {
          case "n": out += "\n"; break;
          case "r": out += "\r"; break;
          case "t": out += "\t"; break;
          case "\\": out += "\\"; break;
          case '"': out += '"'; break;
          case "'": out += "'"; break;
          case "0": out += "\0"; break;
          default: out += e; break;
        }
      } else {
        if (c === "\n") this.line++;
        out += c;
      }
    }
    throw new Error(`protobuf: unterminated string at line ${this.line}`);
  }

  private skipTrivia(): void {
    while (this.pos < this.src.length) {
      const c = this.src[this.pos]!;
      if (c === "\n") {
        this.line++;
        this.pos++;
      } else if (c === " " || c === "\t" || c === "\r" || c === "\f" || c === "\v") {
        this.pos++;
      } else if (c === "/" && this.src[this.pos + 1] === "/") {
        while (this.pos < this.src.length && this.src[this.pos] !== "\n") this.pos++;
      } else if (c === "/" && this.src[this.pos + 1] === "*") {
        this.pos += 2;
        while (this.pos < this.src.length && !(this.src[this.pos] === "*" && this.src[this.pos + 1] === "/")) {
          if (this.src[this.pos] === "\n") this.line++;
          this.pos++;
        }
        this.pos += 2;
      } else {
        break;
      }
    }
  }
}
