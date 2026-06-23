// Recursive-descent parser for proto3 + edition 2023 .proto source → an AST.
// Linking/resolution into the descriptor model lives in link.ts. proto2-only
// constructs (required, group, extend/extensions) are rejected with a clear error.
import { Lexer, type Token } from "./lexer.js";
import { type FeatureSet, featureFromOption } from "./features.js";

export interface AstField {
  label: "singular" | "optional" | "repeated";
  typeName: string;
  name: string;
  number: number;
  jsonName?: string;
  packedOption?: boolean;
  features: FeatureSet;
  map?: { key: string; value: string };
}

export interface AstOneof {
  name: string;
  fields: AstField[];
}

export interface AstEnumValue {
  name: string;
  number: number;
}

export interface AstEnum {
  name: string;
  values: AstEnumValue[];
  features: FeatureSet;
}

export interface AstMessage {
  name: string;
  fields: AstField[];
  oneofs: AstOneof[];
  messages: AstMessage[];
  enums: AstEnum[];
  features: FeatureSet;
}

export interface ParsedFile {
  syntax: "proto3" | "2023";
  package: string;
  imports: string[];
  features: FeatureSet;
  messages: AstMessage[];
  enums: AstEnum[];
}

const SCALARS = new Set([
  "double", "float", "int32", "int64", "uint32", "uint64", "sint32", "sint64",
  "fixed32", "fixed64", "sfixed32", "sfixed64", "bool", "string", "bytes",
]);

export function parseProto(source: string): ParsedFile {
  return new Parser(source).parseFile();
}

class Parser {
  private lx: Lexer;
  constructor(source: string) {
    this.lx = new Lexer(source);
  }

  private err(msg: string, line?: number): never {
    throw new Error(`protobuf parse error${line ? ` (line ${line})` : ""}: ${msg}`);
  }

  private expectSym(s: string): void {
    const t = this.lx.next();
    if (t.kind !== "sym" || t.value !== s) this.err(`expected '${s}', got '${t.value}'`, t.line);
  }

  private acceptSym(s: string): boolean {
    const t = this.lx.peek();
    if (t.kind === "sym" && t.value === s) {
      this.lx.next();
      return true;
    }
    return false;
  }

  private expectIdent(): string {
    const t = this.lx.next();
    if (t.kind !== "ident") this.err(`expected identifier, got '${t.value}'`, t.line);
    return t.value;
  }

  private isKeyword(t: Token, kw: string): boolean {
    return t.kind === "ident" && t.value === kw;
  }

  parseFile(): ParsedFile {
    const file: ParsedFile = {
      syntax: "proto3",
      package: "",
      imports: [],
      features: {},
      messages: [],
      enums: [],
    };
    let sawSyntax = false;

    for (;;) {
      const t = this.lx.peek();
      if (t.kind === "eof") break;
      if (t.kind === "sym" && t.value === ";") {
        this.lx.next();
        continue;
      }
      if (t.kind !== "ident") this.err(`unexpected '${t.value}'`, t.line);

      switch (t.value) {
        case "syntax": {
          this.lx.next();
          this.expectSym("=");
          const v = this.expectStr();
          if (v !== "proto3" && v !== "proto2") this.err(`unknown syntax "${v}"`, t.line);
          if (v === "proto2") this.err("proto2 syntax is unsupported (use proto3 or edition 2023)", t.line);
          file.syntax = "proto3";
          this.expectSym(";");
          sawSyntax = true;
          break;
        }
        case "edition": {
          this.lx.next();
          this.expectSym("=");
          const v = this.expectStr();
          if (v !== "2023") this.err(`unsupported edition "${v}" (only 2023)`, t.line);
          file.syntax = "2023";
          this.expectSym(";");
          sawSyntax = true;
          break;
        }
        case "package":
          this.lx.next();
          file.package = this.parseQualifiedName();
          this.expectSym(";");
          break;
        case "import": {
          this.lx.next();
          // optional public/weak
          const p = this.lx.peek();
          if (this.isKeyword(p, "public") || this.isKeyword(p, "weak")) this.lx.next();
          file.imports.push(this.expectStr());
          this.expectSym(";");
          break;
        }
        case "option": {
          this.lx.next();
          const { key, value } = this.parseOption();
          const f = featureFromOption(key, value);
          if (f) Object.assign(file.features, f);
          this.expectSym(";");
          break;
        }
        case "message":
          this.lx.next();
          file.messages.push(this.parseMessage());
          break;
        case "enum":
          this.lx.next();
          file.enums.push(this.parseEnum());
          break;
        case "service":
          this.lx.next();
          this.skipBlock();
          break;
        case "extend":
          this.lx.next();
          this.skipBlock();
          break;
        default:
          this.err(`unexpected '${t.value}' at top level`, t.line);
      }
    }

    void sawSyntax; // proto3 is the implicit default if omitted
    return file;
  }

  private parseMessage(): AstMessage {
    const name = this.expectIdent();
    const msg: AstMessage = { name, fields: [], oneofs: [], messages: [], enums: [], features: {} };
    this.expectSym("{");
    for (;;) {
      const t = this.lx.peek();
      if (t.kind === "sym" && t.value === "}") {
        this.lx.next();
        break;
      }
      if (t.kind === "eof") this.err("unexpected EOF in message");
      if (t.kind === "sym" && t.value === ";") {
        this.lx.next();
        continue;
      }
      if (t.kind === "ident") {
        switch (t.value) {
          case "message":
            this.lx.next();
            msg.messages.push(this.parseMessage());
            continue;
          case "enum":
            this.lx.next();
            msg.enums.push(this.parseEnum());
            continue;
          case "oneof":
            this.lx.next();
            msg.oneofs.push(this.parseOneof());
            continue;
          case "option": {
            this.lx.next();
            const { key, value } = this.parseOption();
            const f = featureFromOption(key, value);
            if (f) Object.assign(msg.features, f);
            this.expectSym(";");
            continue;
          }
          case "reserved":
            this.lx.next();
            this.skipToSemicolon();
            continue;
          case "map":
            msg.fields.push(this.parseMapField());
            continue;
          case "required":
            this.err("proto2 'required' fields are unsupported", t.line);
          // eslint-disable-next-line no-fallthrough
          case "group":
            this.err("proto2 groups are unsupported", t.line);
          // eslint-disable-next-line no-fallthrough
          case "extensions":
            // extension *range* declaration (valid in editions too) — ignore.
            this.lx.next();
            this.skipToSemicolon();
            continue;
          case "extend":
            // extension field definitions — skipped; such fields decode as unknown.
            this.lx.next();
            this.skipBlock();
            continue;
        }
      }
      // otherwise a field
      msg.fields.push(this.parseField());
    }
    return msg;
  }

  private parseOneof(): AstOneof {
    const name = this.expectIdent();
    const oneof: AstOneof = { name, fields: [] };
    this.expectSym("{");
    for (;;) {
      const t = this.lx.peek();
      if (t.kind === "sym" && t.value === "}") {
        this.lx.next();
        break;
      }
      if (t.kind === "sym" && t.value === ";") {
        this.lx.next();
        continue;
      }
      if (this.isKeyword(t, "option")) {
        this.lx.next();
        this.parseOption();
        this.expectSym(";");
        continue;
      }
      oneof.fields.push(this.parseField(true));
    }
    return oneof;
  }

  private parseField(inOneof = false): AstField {
    let label: AstField["label"] = "singular";
    const t = this.lx.peek();
    if (!inOneof && (t.value === "repeated" || t.value === "optional")) {
      label = t.value as AstField["label"];
      this.lx.next();
    } else if (t.value === "repeated" || t.value === "optional") {
      this.err(`'${t.value}' is not allowed inside oneof`, t.line);
    }
    const typeName = this.parseQualifiedName();
    const name = this.expectIdent();
    this.expectSym("=");
    const number = this.parseInt32();
    const field: AstField = { label, typeName, name, number, features: {} };
    this.parseFieldOptions(field);
    this.expectSym(";");
    return field;
  }

  private parseMapField(): AstField {
    this.lx.next(); // 'map'
    this.expectSym("<");
    const key = this.parseQualifiedName();
    this.expectSym(",");
    const value = this.parseQualifiedName();
    this.expectSym(">");
    const name = this.expectIdent();
    this.expectSym("=");
    const number = this.parseInt32();
    const field: AstField = { label: "repeated", typeName: "", name, number, features: {}, map: { key, value } };
    this.parseFieldOptions(field);
    this.expectSym(";");
    return field;
  }

  private parseFieldOptions(field: AstField): void {
    if (!this.acceptSym("[")) return;
    for (;;) {
      const { key, value } = this.parseOption();
      if (key === "json_name") field.jsonName = value;
      else if (key === "packed") field.packedOption = value === "true";
      else {
        const f = featureFromOption(key, value);
        if (f) Object.assign(field.features, f);
      }
      if (this.acceptSym(",")) continue;
      this.expectSym("]");
      break;
    }
  }

  /** Parses `name = value`. Custom/extension option names `(x.y)` are consumed
   *  and their value skipped. Returns the dotted key + a stringified value. */
  private parseOption(): { key: string; value: string } {
    let key: string;
    if (this.acceptSym("(")) {
      // (custom.option) possibly with trailing .field — skip, return sentinel
      this.parseQualifiedName();
      this.expectSym(")");
      while (this.acceptSym(".")) this.expectIdent();
      key = "";
    } else {
      key = this.expectIdent();
      while (this.acceptSym(".")) key += "." + this.expectIdent();
    }
    this.expectSym("=");
    const value = this.parseOptionValue();
    return { key, value };
  }

  private parseOptionValue(): string {
    const t = this.lx.next();
    if (t.kind === "str") return t.value;
    if (t.kind === "ident" || t.kind === "num") return t.value;
    if (t.kind === "sym" && (t.value === "-" || t.value === "+")) {
      const num = this.lx.next();
      if (num.kind !== "num") this.err(`bad option value '${t.value}${num.value}'`, t.line);
      return (t.value === "-" ? "-" : "") + num.value;
    }
    if (t.kind === "sym" && t.value === "{") {
      // aggregate option value — skip to matching brace
      let depth = 1;
      while (depth > 0) {
        const n = this.lx.next();
        if (n.kind === "eof") this.err("unexpected EOF in option");
        if (n.kind === "sym" && n.value === "{") depth++;
        else if (n.kind === "sym" && n.value === "}") depth--;
      }
      return "";
    }
    this.err(`bad option value '${t.value}'`, t.line);
  }

  private parseEnum(): AstEnum {
    const name = this.expectIdent();
    const en: AstEnum = { name, values: [], features: {} };
    this.expectSym("{");
    for (;;) {
      const t = this.lx.peek();
      if (t.kind === "sym" && t.value === "}") {
        this.lx.next();
        break;
      }
      if (t.kind === "sym" && t.value === ";") {
        this.lx.next();
        continue;
      }
      if (this.isKeyword(t, "option")) {
        this.lx.next();
        const { key, value } = this.parseOption();
        const f = featureFromOption(key, value);
        if (f) Object.assign(en.features, f);
        this.expectSym(";");
        continue;
      }
      if (this.isKeyword(t, "reserved")) {
        this.lx.next();
        this.skipToSemicolon();
        continue;
      }
      const vname = this.expectIdent();
      this.expectSym("=");
      const num = this.parseInt32();
      // skip value options
      if (this.acceptSym("[")) {
        for (;;) {
          this.parseOption();
          if (this.acceptSym(",")) continue;
          this.expectSym("]");
          break;
        }
      }
      this.expectSym(";");
      en.values.push({ name: vname, number: num });
    }
    return en;
  }

  // --- token helpers ---

  private expectStr(): string {
    const t = this.lx.next();
    if (t.kind !== "str") this.err(`expected string, got '${t.value}'`, t.line);
    return t.value;
  }

  private parseInt32(): number {
    let sign = 1;
    const s = this.lx.peek();
    if (s.kind === "sym" && (s.value === "-" || s.value === "+")) {
      this.lx.next();
      sign = s.value === "-" ? -1 : 1;
    }
    const t = this.lx.next();
    if (t.kind !== "num") this.err(`expected number, got '${t.value}'`, t.line);
    const n = t.value.startsWith("0x") || t.value.startsWith("0X")
      ? parseInt(t.value, 16)
      : parseInt(t.value, 10);
    if (!Number.isFinite(n)) this.err(`bad number '${t.value}'`, t.line);
    return sign * n;
  }

  /** Reads a (possibly dotted, possibly leading-dot) qualified name. */
  private parseQualifiedName(): string {
    let name = "";
    if (this.acceptSym(".")) name = ".";
    name += this.expectIdent();
    while (this.lx.peek().kind === "sym" && this.lx.peek().value === ".") {
      this.lx.next();
      name += "." + this.expectIdent();
    }
    return name;
  }

  private skipToSemicolon(): void {
    for (;;) {
      const t = this.lx.next();
      if (t.kind === "eof") this.err("unexpected EOF");
      if (t.kind === "sym" && t.value === ";") return;
    }
  }

  private skipBlock(): void {
    // skip an identifier name then a {...} block (services)
    while (!(this.lx.peek().kind === "sym" && this.lx.peek().value === "{")) {
      if (this.lx.peek().kind === "eof") this.err("unexpected EOF");
      this.lx.next();
    }
    this.expectSym("{");
    let depth = 1;
    while (depth > 0) {
      const t = this.lx.next();
      if (t.kind === "eof") this.err("unexpected EOF in block");
      if (t.kind === "sym" && t.value === "{") depth++;
      else if (t.kind === "sym" && t.value === "}") depth--;
    }
  }

  static isScalar(name: string): boolean {
    return SCALARS.has(name);
  }
}

export { Parser };
