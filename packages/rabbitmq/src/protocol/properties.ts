/**
 * The content header: the frame that says how many body octets are coming and
 * carries a message's properties.
 *
 * For `basic` (class 60) the header is: the class, a weight that is always
 * zero, the body size as a 64-bit count, and a bag of properties. The
 * properties are not a field table — they are positional. A 16-bit flag word
 * says which are present, highest bit first in the class's declared order, and
 * only the present ones follow, in that same order. So `correlationId` costs
 * nothing when absent and its position depends on whether `replyTo` and the
 * four above it came first; the flags and the values have to agree.
 *
 * Only the `basic` header is implemented. A header for another class is a
 * `ProtocolError` rather than something to guess at, since no other class in
 * this client sends content.
 */
import { ProtocolError } from "./errors.js";
import { type FieldTable, readFieldTable, writeFieldTable } from "./fields.js";
import { ClassId } from "./methods.js";
import { ByteReader, ByteWriter } from "./wire.js";

/** The `basic` class's properties, in wire order. Absent means not sent. */
export interface BasicProperties {
  contentType?: string;
  contentEncoding?: string;
  headers?: FieldTable;
  /** 1 transient, 2 persistent. */
  deliveryMode?: number;
  priority?: number;
  correlationId?: string;
  replyTo?: string;
  expiration?: string;
  messageId?: string;
  /** Seconds since the epoch. */
  timestamp?: bigint;
  type?: string;
  userId?: string;
  appId?: string;
  clusterId?: string;
}

/** A decoded content-header frame payload. */
export interface ContentHeader {
  classId: number;
  weight: number;
  bodySize: bigint;
  properties: BasicProperties;
}

// Flag bits, highest first. The class's declared order is the array order.
const BITS = {
  contentType: 15,
  contentEncoding: 14,
  headers: 13,
  deliveryMode: 12,
  priority: 11,
  correlationId: 10,
  replyTo: 9,
  expiration: 8,
  messageId: 7,
  timestamp: 6,
  type: 5,
  userId: 4,
  appId: 3,
  clusterId: 2,
} as const;

// Bit 0 is a continuation flag: a second flag word follows. This client never
// sends one and would not know how to read one, so a peer setting it is an
// error rather than something to skip past.
const CONTINUATION = 1;

/** Encodes a content header's frame payload. */
export function encodeContentHeader(header: ContentHeader): Uint8Array {
  if (header.classId !== ClassId.basic) {
    throw new ProtocolError(
      `the content header is only defined for basic (60), not ${header.classId}`,
    );
  }
  if (header.weight !== 0) {
    throw new ProtocolError(`a content header's weight must be 0, not ${header.weight}`);
  }
  const p = header.properties;
  let flags = 0;
  if (p.contentType !== undefined) flags |= 1 << BITS.contentType;
  if (p.contentEncoding !== undefined) flags |= 1 << BITS.contentEncoding;
  if (p.headers !== undefined) flags |= 1 << BITS.headers;
  if (p.deliveryMode !== undefined) flags |= 1 << BITS.deliveryMode;
  if (p.priority !== undefined) flags |= 1 << BITS.priority;
  if (p.correlationId !== undefined) flags |= 1 << BITS.correlationId;
  if (p.replyTo !== undefined) flags |= 1 << BITS.replyTo;
  if (p.expiration !== undefined) flags |= 1 << BITS.expiration;
  if (p.messageId !== undefined) flags |= 1 << BITS.messageId;
  if (p.timestamp !== undefined) flags |= 1 << BITS.timestamp;
  if (p.type !== undefined) flags |= 1 << BITS.type;
  if (p.userId !== undefined) flags |= 1 << BITS.userId;
  if (p.appId !== undefined) flags |= 1 << BITS.appId;
  if (p.clusterId !== undefined) flags |= 1 << BITS.clusterId;

  const w = new ByteWriter(64);
  w.u16(header.classId).u16(header.weight).u64(header.bodySize).u16(flags);
  if (p.contentType !== undefined) w.shortstr(p.contentType);
  if (p.contentEncoding !== undefined) w.shortstr(p.contentEncoding);
  if (p.headers !== undefined) writeFieldTable(w, p.headers);
  if (p.deliveryMode !== undefined) {
    if (p.deliveryMode !== 1 && p.deliveryMode !== 2) {
      throw new ProtocolError(
        `a delivery mode is 1 (transient) or 2 (persistent), not ${p.deliveryMode}`,
      );
    }
    w.u8(p.deliveryMode);
  }
  if (p.priority !== undefined) w.u8(p.priority);
  if (p.correlationId !== undefined) w.shortstr(p.correlationId);
  if (p.replyTo !== undefined) w.shortstr(p.replyTo);
  if (p.expiration !== undefined) w.shortstr(p.expiration);
  if (p.messageId !== undefined) w.shortstr(p.messageId);
  if (p.timestamp !== undefined) w.u64(p.timestamp);
  if (p.type !== undefined) w.shortstr(p.type);
  if (p.userId !== undefined) w.shortstr(p.userId);
  if (p.appId !== undefined) w.shortstr(p.appId);
  if (p.clusterId !== undefined) w.shortstr(p.clusterId);
  return w.finish();
}

/** Decodes a content-header frame payload. */
export function decodeContentHeader(payload: Uint8Array): ContentHeader {
  const r = new ByteReader(payload);
  const classId = r.u16();
  const weight = r.u16();
  const bodySize = r.u64();
  if (classId !== ClassId.basic) {
    throw new ProtocolError(`the content header is only defined for basic (60), not ${classId}`);
  }
  if (weight !== 0) {
    throw new ProtocolError(`a content header's weight must be 0, not ${weight}`);
  }
  const flags = r.u16();
  if (flags & CONTINUATION) {
    throw new ProtocolError(
      "a content header with more than one property-flag word is not supported",
    );
  }

  const p: BasicProperties = {};
  if (flags & (1 << BITS.contentType)) p.contentType = r.shortstr();
  if (flags & (1 << BITS.contentEncoding)) p.contentEncoding = r.shortstr();
  if (flags & (1 << BITS.headers)) p.headers = readFieldTable(r);
  if (flags & (1 << BITS.deliveryMode)) p.deliveryMode = r.u8();
  if (flags & (1 << BITS.priority)) p.priority = r.u8();
  if (flags & (1 << BITS.correlationId)) p.correlationId = r.shortstr();
  if (flags & (1 << BITS.replyTo)) p.replyTo = r.shortstr();
  if (flags & (1 << BITS.expiration)) p.expiration = r.shortstr();
  if (flags & (1 << BITS.messageId)) p.messageId = r.shortstr();
  if (flags & (1 << BITS.timestamp)) p.timestamp = r.u64();
  if (flags & (1 << BITS.type)) p.type = r.shortstr();
  if (flags & (1 << BITS.userId)) p.userId = r.shortstr();
  if (flags & (1 << BITS.appId)) p.appId = r.shortstr();
  if (flags & (1 << BITS.clusterId)) p.clusterId = r.shortstr();

  if (!r.done) {
    throw new ProtocolError(`a content header left ${r.remaining} bytes unread`);
  }
  return { classId, weight, bodySize, properties: p };
}
