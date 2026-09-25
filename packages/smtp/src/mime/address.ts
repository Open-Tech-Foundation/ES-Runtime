/**
 * Mail addresses: what a caller writes, what the envelope needs, and what a
 * header shows.
 *
 * Three spellings are accepted — `"ada@example.com"`, `"Ada <ada@example.com>"`
 * and `{ name, address }` — and each is split into a display name and an
 * address. The address goes on the envelope (`MAIL FROM`, `RCPT TO`), which
 * carries no names; both go into the header.
 */

import { SmtpError, SmtpErrorCode } from "../errors.js";
import { encodeWord, needsEncoding } from "./encode.js";

export type AddressInput = string | { name?: string; address: string };

export interface Address {
  name: string;
  address: string;
}

export function parseAddress(input: AddressInput): Address {
  let name = "";
  let address: string;
  if (typeof input === "string") {
    const angle = /^\s*(.*?)\s*<([^<>]+)>\s*$/.exec(input);
    if (angle !== null) {
      name = (angle[1] ?? "").replace(/^"(.*)"$/, "$1");
      address = angle[2] ?? "";
    } else {
      address = input.trim();
    }
  } else {
    name = input.name ?? "";
    address = input.address.trim();
  }
  checkAddress(address);
  if (/[\r\n]/.test(name))
    invalid(`the display name ${JSON.stringify(name)} contains a line break`);
  return { name, address: normalizeDomain(address) };
}

export function parseAddresses(input: AddressInput | AddressInput[] | undefined): Address[] {
  if (input === undefined) return [];
  return (Array.isArray(input) ? input : [input]).map(parseAddress);
}

/**
 * Refuses what cannot be an address. Deliberately not a full RFC 5321 grammar:
 * a server is the authority on which addresses exist, and a client rejecting
 * a valid-but-unusual one is worse than letting the server say no. What is
 * refused here is only what would break the protocol line it is written into —
 * a line break, an angle bracket, whitespace — or has no `@` at all.
 */
function checkAddress(address: string): void {
  if (/[\r\n]/.test(address))
    invalid(`the address ${JSON.stringify(address)} contains a line break`);
  if (/[<>\s]/.test(address)) invalid(`${JSON.stringify(address)} is not an address`);
  const at = address.lastIndexOf("@");
  if (at <= 0 || at === address.length - 1) invalid(`${JSON.stringify(address)} is not an address`);
}

/**
 * An internationalised domain in its ASCII form (`bücher.example` →
 * `xn--bcher-kva.example`), which every server accepts. The WHATWG URL parser
 * already implements IDNA, so it is the converter.
 */
function normalizeDomain(address: string): string {
  const at = address.lastIndexOf("@");
  const local = address.slice(0, at);
  const domain = address.slice(at + 1);
  if (!needsEncoding(domain)) return address;
  try {
    return `${local}@${new URL(`http://${domain}`).hostname}`;
  } catch {
    invalid(`the domain ${JSON.stringify(domain)} is not a valid internationalised domain name`);
  }
}

/** Whether an address needs `SMTPUTF8`: a local part outside ASCII. */
export function needsSmtpUtf8(address: string): boolean {
  return needsEncoding(address.slice(0, address.lastIndexOf("@")));
}

/** The header form: `Ada <ada@example.com>`, quoted or encoded as needed. */
export function formatAddress({ name, address }: Address): string {
  if (name === "") return address;
  if (needsEncoding(name)) return `${encodeWord(name)} <${address}>`;
  // A name with specials must be quoted (RFC 5322 §3.2.3), escaping `"` and `\`.
  if (/[()<>[\]:;@\\,."]/.test(name)) {
    return `"${name.replace(/(["\\])/g, "\\$1")}" <${address}>`;
  }
  return `${name} <${address}>`;
}

function invalid(message: string): never {
  throw new SmtpError(message, { code: SmtpErrorCode.InvalidMessage });
}
