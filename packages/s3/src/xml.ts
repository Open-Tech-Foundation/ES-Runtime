/**
 * Reading S3's XML through `runtime:serialization`.
 *
 * `XML.parse` gives an element with text as `{ $text: "…" }`, an empty element
 * as `""`, a repeated element as an array, and a single one as itself. These
 * helpers flatten that back into what S3 meant.
 */
import { XML } from "runtime:serialization";
import { S3Error, S3ErrorCode } from "./errors.js";

export type Node = unknown;

export function parse(text: string, what: string): Record<string, Node> {
  try {
    const value = XML.parse(text) as Record<string, Node>;
    if (value === null || typeof value !== "object") throw new Error("not an element");
    return value;
  } catch (cause) {
    throw new S3Error(`${what}: the response is not XML`, { code: S3ErrorCode.Response, cause });
  }
}

/** The element's text, or `undefined` when it is absent. An empty element is `""`. */
export function text(node: Node): string | undefined {
  if (node === undefined || node === null) return undefined;
  if (typeof node === "string") return node;
  if (typeof node === "object" && "$text" in (node as object)) {
    return String((node as { $text: unknown }).$text);
  }
  return undefined;
}

export function child(node: Node, name: string): Node {
  return node !== null && typeof node === "object"
    ? (node as Record<string, Node>)[name]
    : undefined;
}

/** A child that may repeat, as an array whether it appeared zero, one or many times. */
export function children(node: Node, name: string): Node[] {
  const value = child(node, name);
  if (value === undefined) return [];
  return Array.isArray(value) ? value : [value];
}

export function childText(node: Node, name: string): string | undefined {
  return text(child(node, name));
}

export function childNumber(node: Node, name: string): number | undefined {
  const value = childText(node, name);
  return value === undefined || value === "" ? undefined : Number(value);
}

export function childBoolean(node: Node, name: string): boolean | undefined {
  const value = childText(node, name);
  return value === undefined ? undefined : value === "true";
}

export function childDate(node: Node, name: string): Date | undefined {
  const value = childText(node, name);
  return value === undefined ? undefined : new Date(value);
}

export function build(value: Record<string, unknown>): string {
  try {
    return XML.build(value);
  } catch (cause) {
    throw new S3Error(`cannot be sent as XML: ${(cause as Error)?.message ?? cause}`, {
      code: S3ErrorCode.InvalidArgument,
      cause,
    });
  }
}

/** S3's namespace, which request bodies carry. */
export const NAMESPACE = "http://s3.amazonaws.com/doc/2006-03-01/";
