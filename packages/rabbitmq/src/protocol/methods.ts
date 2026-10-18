/**
 * AMQP 0-9-1 methods: which one a payload is, and what its arguments are.
 *
 * A method-frame payload opens with two 16-bit numbers — a class and a method
 * within it — and everything after them is that method's arguments, in the order
 * the class declares them. There is no field name or tag on the wire, so the
 * only thing that can say what `\x00\x3c\x00\x28` means is a table transcribed
 * from the specification. That table is [`METHODS`](#METHODS).
 *
 * Two things about the arguments are easy to get wrong and are why this is a
 * table with one encoder rather than a hundred hand-written pairs:
 *
 * - **Adjacent `bit` fields share an octet.** They are packed least-significant
 *   bit first and an octet is emitted even when every bit in the run is false,
 *   so the run's length is part of the method's shape, not of its values.
 * - **A `reserved` field is still transmitted.** The specification's reserved
 *   fields carry default values and a peer will reject the frame if a field's
 *   count or size is wrong, which is why the table gives each one a default and
 *   [`encodeMethod`](#encodeMethod) fills it in.
 *
 * The table covers the classes this client speaks. RabbitMQ's two extensions to
 * the class set — `confirm` and `basic.nack`, plus `connection.blocked` and
 * `exchange.bind`/`unbind` — are included because the broker sends them. The
 * `access` class is not, because 0-9-1 no longer defines it, and
 * `basic.recover-async` is not, because the specification marks it deprecated in
 * favour of `basic.recover`.
 */
import { ProtocolError } from "./errors.js";
import { type FieldTable, readFieldTable, writeFieldTable } from "./fields.js";
import { ByteReader, ByteWriter } from "./wire.js";

// -- the identity of a method -------------------------------------------------

/** Class identifiers, by the class they name. */
export const ClassId = {
  connection: 10,
  channel: 20,
  exchange: 40,
  queue: 50,
  basic: 60,
  confirm: 85,
  tx: 90,
} as const;

/** Method identifiers, by class. */
export const MethodId = {
  connection: {
    start: 10,
    startOk: 11,
    secure: 20,
    secureOk: 21,
    tune: 30,
    tuneOk: 31,
    open: 40,
    openOk: 41,
    close: 50,
    closeOk: 51,
    blocked: 60,
    unblocked: 61,
  },
  channel: {
    open: 10,
    openOk: 11,
    flow: 20,
    flowOk: 21,
    close: 40,
    closeOk: 41,
  },
  exchange: {
    declare: 10,
    declareOk: 11,
    delete: 20,
    deleteOk: 21,
    bind: 30,
    bindOk: 31,
    unbind: 40,
    unbindOk: 51,
  },
  queue: {
    declare: 10,
    declareOk: 11,
    bind: 20,
    bindOk: 21,
    purge: 30,
    purgeOk: 31,
    delete: 40,
    deleteOk: 41,
    unbind: 50,
    unbindOk: 51,
  },
  basic: {
    qos: 10,
    qosOk: 11,
    consume: 20,
    consumeOk: 21,
    cancel: 30,
    cancelOk: 31,
    publish: 40,
    return: 50,
    deliver: 60,
    get: 70,
    getOk: 71,
    getEmpty: 72,
    ack: 80,
    reject: 90,
    recover: 110,
    recoverOk: 111,
    nack: 120,
  },
  confirm: {
    select: 10,
    selectOk: 11,
  },
  tx: {
    select: 10,
    selectOk: 11,
    commit: 20,
    commitOk: 21,
    rollback: 30,
    rollbackOk: 31,
  },
} as const;

/** A method's identity: the class and the method within it. */
export interface MethodKey {
  classId: number;
  methodId: number;
}

// Built from the constants above so a name cannot drift from its number.
const NAMES = new Map<string, string>();
for (const [className, classId] of Object.entries(ClassId) as [keyof typeof ClassId, number][]) {
  for (const [methodName, methodId] of Object.entries(MethodId[className])) {
    NAMES.set(`${classId}:${methodId}`, `${className}.${methodName}`);
  }
}

/** The dotted name of a method — `channel.close` — or `10:60` if it has none. */
export function methodName(classId: number, methodId: number): string {
  return NAMES.get(`${classId}:${methodId}`) ?? `${classId}:${methodId}`;
}

/** Reads the class and method a payload opens with. */
export function readMethodKey(r: ByteReader): MethodKey {
  return { classId: r.u16(), methodId: r.u16() };
}

/** Writes the class and method a payload opens with. */
export function writeMethodKey(w: ByteWriter, key: MethodKey): void {
  if (key.classId > 0xffff || key.methodId > 0xffff) {
    throw new ProtocolError(
      `a method key must fit in two 16-bit numbers: ${methodName(key.classId, key.methodId)}`,
    );
  }
  w.u16(key.classId).u16(key.methodId);
}

// -- a method's arguments -----------------------------------------------------

/** A value an argument can hold: what the domains below decode to. */
export type MethodArg = number | bigint | string | Uint8Array | boolean | FieldTable;

/**
 * A field's type. `longlong` is the specification's `longlong` and its
 * `timestamp` (both 64-bit unsigned); `long` is 32-bit and `short` 16-bit.
 */
export type Domain =
  | "octet"
  | "short"
  | "long"
  | "longlong"
  | "shortstr"
  | "longstr"
  | "table"
  | "bit";

/** One argument, in wire order. `default` makes it optional to supply. */
export interface FieldDefinition {
  name: string;
  domain: Domain;
  default?: MethodArg;
}

/** One method: its identity, its arguments, and whether content follows it. */
export interface MethodDefinition {
  name: string;
  classId: number;
  methodId: number;
  fields: readonly FieldDefinition[];
  /**
   * Whether the method announces content, so a header frame and body frames
   * follow it. `basic.publish` sends content; `basic.deliver` receives it.
   */
  hasContent?: boolean;
}

// A field with a default is one a caller may leave out — the specification's
// reserved fields, which are still on the wire with a fixed value.
function f(name: string, domain: Domain, value?: MethodArg): FieldDefinition {
  return value === undefined ? { name, domain } : { name, domain, default: value };
}

/**
 * Every method this client speaks, in the classes it speaks them in.
 *
 * Names are camelCased off the specification's hyphenated ones (`consumer-tag`
 * is `consumerTag`), which is the only liberty taken here.
 */
export const METHODS: readonly MethodDefinition[] = [
  // -- connection (10) ------------------------------------------------------
  {
    name: "connection.start",
    classId: ClassId.connection,
    methodId: MethodId.connection.start,
    fields: [
      f("versionMajor", "octet"),
      f("versionMinor", "octet"),
      f("serverProperties", "table"),
      f("mechanisms", "longstr"),
      f("locales", "longstr"),
    ],
  },
  {
    name: "connection.start-ok",
    classId: ClassId.connection,
    methodId: MethodId.connection.startOk,
    fields: [
      f("clientProperties", "table"),
      f("mechanism", "shortstr"),
      f("response", "longstr"),
      f("locale", "shortstr"),
    ],
  },
  {
    name: "connection.secure",
    classId: ClassId.connection,
    methodId: MethodId.connection.secure,
    fields: [f("challenge", "longstr")],
  },
  {
    name: "connection.secure-ok",
    classId: ClassId.connection,
    methodId: MethodId.connection.secureOk,
    fields: [f("response", "longstr")],
  },
  {
    name: "connection.tune",
    classId: ClassId.connection,
    methodId: MethodId.connection.tune,
    fields: [f("channelMax", "short"), f("frameMax", "long"), f("heartbeat", "short")],
  },
  {
    name: "connection.tune-ok",
    classId: ClassId.connection,
    methodId: MethodId.connection.tuneOk,
    fields: [f("channelMax", "short"), f("frameMax", "long"), f("heartbeat", "short")],
  },
  {
    name: "connection.open",
    classId: ClassId.connection,
    methodId: MethodId.connection.open,
    fields: [
      f("virtualHost", "shortstr"),
      f("reserved1", "shortstr", ""),
      f("reserved2", "bit", false),
    ],
  },
  {
    name: "connection.open-ok",
    classId: ClassId.connection,
    methodId: MethodId.connection.openOk,
    fields: [f("reserved1", "shortstr", "")],
  },
  {
    name: "connection.close",
    classId: ClassId.connection,
    methodId: MethodId.connection.close,
    fields: [
      f("replyCode", "short"),
      f("replyText", "shortstr"),
      f("classId", "short"),
      f("methodId", "short"),
    ],
  },
  {
    name: "connection.close-ok",
    classId: ClassId.connection,
    methodId: MethodId.connection.closeOk,
    fields: [],
  },
  // RabbitMQ extension: the broker is low on a resource and tells the client.
  {
    name: "connection.blocked",
    classId: ClassId.connection,
    methodId: MethodId.connection.blocked,
    fields: [f("reason", "shortstr")],
  },
  {
    name: "connection.unblocked",
    classId: ClassId.connection,
    methodId: MethodId.connection.unblocked,
    fields: [],
  },

  // -- channel (20) ---------------------------------------------------------
  {
    name: "channel.open",
    classId: ClassId.channel,
    methodId: MethodId.channel.open,
    fields: [f("reserved1", "shortstr", "")],
  },
  {
    name: "channel.open-ok",
    classId: ClassId.channel,
    methodId: MethodId.channel.openOk,
    fields: [f("reserved1", "longstr", "")],
  },
  {
    name: "channel.flow",
    classId: ClassId.channel,
    methodId: MethodId.channel.flow,
    fields: [f("active", "bit")],
  },
  {
    name: "channel.flow-ok",
    classId: ClassId.channel,
    methodId: MethodId.channel.flowOk,
    fields: [f("active", "bit")],
  },
  {
    name: "channel.close",
    classId: ClassId.channel,
    methodId: MethodId.channel.close,
    fields: [
      f("replyCode", "short"),
      f("replyText", "shortstr"),
      f("classId", "short"),
      f("methodId", "short"),
    ],
  },
  {
    name: "channel.close-ok",
    classId: ClassId.channel,
    methodId: MethodId.channel.closeOk,
    fields: [],
  },

  // -- exchange (40) --------------------------------------------------------
  // The base specification reserved the two bit positions after `durable`; the
  // extended one un-deprecates them as `auto-delete` and `internal`, which is
  // what RabbitMQ reads there.
  {
    name: "exchange.declare",
    classId: ClassId.exchange,
    methodId: MethodId.exchange.declare,
    fields: [
      f("reserved1", "short", 0),
      f("exchange", "shortstr"),
      f("type", "shortstr"),
      f("passive", "bit"),
      f("durable", "bit"),
      f("autoDelete", "bit"),
      f("internal", "bit"),
      f("noWait", "bit"),
      f("arguments", "table"),
    ],
  },
  {
    name: "exchange.declare-ok",
    classId: ClassId.exchange,
    methodId: MethodId.exchange.declareOk,
    fields: [],
  },
  {
    name: "exchange.delete",
    classId: ClassId.exchange,
    methodId: MethodId.exchange.delete,
    fields: [
      f("reserved1", "short", 0),
      f("exchange", "shortstr"),
      f("ifUnused", "bit"),
      f("noWait", "bit"),
    ],
  },
  {
    name: "exchange.delete-ok",
    classId: ClassId.exchange,
    methodId: MethodId.exchange.deleteOk,
    fields: [],
  },
  {
    name: "exchange.bind",
    classId: ClassId.exchange,
    methodId: MethodId.exchange.bind,
    fields: [
      f("reserved1", "short", 0),
      f("destination", "shortstr"),
      f("source", "shortstr"),
      f("routingKey", "shortstr"),
      f("noWait", "bit"),
      f("arguments", "table"),
    ],
  },
  {
    name: "exchange.bind-ok",
    classId: ClassId.exchange,
    methodId: MethodId.exchange.bindOk,
    fields: [],
  },
  {
    name: "exchange.unbind",
    classId: ClassId.exchange,
    methodId: MethodId.exchange.unbind,
    fields: [
      f("reserved1", "short", 0),
      f("destination", "shortstr"),
      f("source", "shortstr"),
      f("routingKey", "shortstr"),
      f("noWait", "bit"),
      f("arguments", "table"),
    ],
  },
  {
    name: "exchange.unbind-ok",
    classId: ClassId.exchange,
    methodId: MethodId.exchange.unbindOk,
    fields: [],
  },

  // -- queue (50) -----------------------------------------------------------
  {
    name: "queue.declare",
    classId: ClassId.queue,
    methodId: MethodId.queue.declare,
    fields: [
      f("reserved1", "short", 0),
      f("queue", "shortstr", ""),
      f("passive", "bit"),
      f("durable", "bit"),
      f("exclusive", "bit"),
      f("autoDelete", "bit"),
      f("noWait", "bit"),
      f("arguments", "table"),
    ],
  },
  {
    name: "queue.declare-ok",
    classId: ClassId.queue,
    methodId: MethodId.queue.declareOk,
    fields: [f("queue", "shortstr"), f("messageCount", "long"), f("consumerCount", "long")],
  },
  {
    name: "queue.bind",
    classId: ClassId.queue,
    methodId: MethodId.queue.bind,
    fields: [
      f("reserved1", "short", 0),
      f("queue", "shortstr"),
      f("exchange", "shortstr"),
      f("routingKey", "shortstr", ""),
      f("noWait", "bit"),
      f("arguments", "table"),
    ],
  },
  {
    name: "queue.bind-ok",
    classId: ClassId.queue,
    methodId: MethodId.queue.bindOk,
    fields: [],
  },
  {
    name: "queue.purge",
    classId: ClassId.queue,
    methodId: MethodId.queue.purge,
    fields: [f("reserved1", "short", 0), f("queue", "shortstr"), f("noWait", "bit")],
  },
  {
    name: "queue.purge-ok",
    classId: ClassId.queue,
    methodId: MethodId.queue.purgeOk,
    fields: [f("messageCount", "long")],
  },
  {
    name: "queue.delete",
    classId: ClassId.queue,
    methodId: MethodId.queue.delete,
    fields: [
      f("reserved1", "short", 0),
      f("queue", "shortstr"),
      f("ifUnused", "bit"),
      f("ifEmpty", "bit"),
      f("noWait", "bit"),
    ],
  },
  {
    name: "queue.delete-ok",
    classId: ClassId.queue,
    methodId: MethodId.queue.deleteOk,
    fields: [f("messageCount", "long")],
  },
  {
    name: "queue.unbind",
    classId: ClassId.queue,
    methodId: MethodId.queue.unbind,
    fields: [
      f("reserved1", "short", 0),
      f("queue", "shortstr"),
      f("exchange", "shortstr"),
      f("routingKey", "shortstr", ""),
      f("arguments", "table"),
    ],
  },
  {
    name: "queue.unbind-ok",
    classId: ClassId.queue,
    methodId: MethodId.queue.unbindOk,
    fields: [],
  },

  // -- basic (60) -----------------------------------------------------------
  {
    name: "basic.qos",
    classId: ClassId.basic,
    methodId: MethodId.basic.qos,
    fields: [f("prefetchSize", "long"), f("prefetchCount", "short"), f("global", "bit")],
  },
  {
    name: "basic.qos-ok",
    classId: ClassId.basic,
    methodId: MethodId.basic.qosOk,
    fields: [],
  },
  {
    name: "basic.consume",
    classId: ClassId.basic,
    methodId: MethodId.basic.consume,
    fields: [
      f("reserved1", "short", 0),
      f("queue", "shortstr"),
      f("consumerTag", "shortstr", ""),
      f("noLocal", "bit"),
      f("noAck", "bit"),
      f("exclusive", "bit"),
      f("noWait", "bit"),
      f("arguments", "table"),
    ],
  },
  {
    name: "basic.consume-ok",
    classId: ClassId.basic,
    methodId: MethodId.basic.consumeOk,
    fields: [f("consumerTag", "shortstr")],
  },
  {
    name: "basic.cancel",
    classId: ClassId.basic,
    methodId: MethodId.basic.cancel,
    fields: [f("consumerTag", "shortstr"), f("noWait", "bit")],
  },
  {
    name: "basic.cancel-ok",
    classId: ClassId.basic,
    methodId: MethodId.basic.cancelOk,
    fields: [f("consumerTag", "shortstr")],
  },
  {
    name: "basic.publish",
    classId: ClassId.basic,
    methodId: MethodId.basic.publish,
    hasContent: true,
    fields: [
      f("reserved1", "short", 0),
      f("exchange", "shortstr"),
      f("routingKey", "shortstr"),
      f("mandatory", "bit"),
      f("immediate", "bit"),
    ],
  },
  {
    name: "basic.return",
    classId: ClassId.basic,
    methodId: MethodId.basic.return,
    hasContent: true,
    fields: [
      f("replyCode", "short"),
      f("replyText", "shortstr"),
      f("exchange", "shortstr"),
      f("routingKey", "shortstr"),
    ],
  },
  {
    name: "basic.deliver",
    classId: ClassId.basic,
    methodId: MethodId.basic.deliver,
    hasContent: true,
    fields: [
      f("consumerTag", "shortstr"),
      f("deliveryTag", "longlong"),
      f("redelivered", "bit"),
      f("exchange", "shortstr"),
      f("routingKey", "shortstr"),
    ],
  },
  {
    name: "basic.get",
    classId: ClassId.basic,
    methodId: MethodId.basic.get,
    fields: [f("reserved1", "short", 0), f("queue", "shortstr"), f("noAck", "bit")],
  },
  {
    name: "basic.get-ok",
    classId: ClassId.basic,
    methodId: MethodId.basic.getOk,
    hasContent: true,
    fields: [
      f("deliveryTag", "longlong"),
      f("redelivered", "bit"),
      f("exchange", "shortstr"),
      f("routingKey", "shortstr"),
      f("messageCount", "long"),
    ],
  },
  {
    name: "basic.get-empty",
    classId: ClassId.basic,
    methodId: MethodId.basic.getEmpty,
    fields: [f("reserved1", "shortstr", "")],
  },
  {
    name: "basic.ack",
    classId: ClassId.basic,
    methodId: MethodId.basic.ack,
    fields: [f("deliveryTag", "longlong"), f("multiple", "bit")],
  },
  {
    name: "basic.reject",
    classId: ClassId.basic,
    methodId: MethodId.basic.reject,
    fields: [f("deliveryTag", "longlong"), f("requeue", "bit")],
  },
  {
    name: "basic.recover",
    classId: ClassId.basic,
    methodId: MethodId.basic.recover,
    fields: [f("requeue", "bit")],
  },
  {
    name: "basic.recover-ok",
    classId: ClassId.basic,
    methodId: MethodId.basic.recoverOk,
    fields: [],
  },
  // RabbitMQ extension: reject or requeue a whole range in one frame.
  {
    name: "basic.nack",
    classId: ClassId.basic,
    methodId: MethodId.basic.nack,
    fields: [f("deliveryTag", "longlong"), f("multiple", "bit"), f("requeue", "bit")],
  },

  // -- confirm (85), a RabbitMQ extension -----------------------------------
  {
    name: "confirm.select",
    classId: ClassId.confirm,
    methodId: MethodId.confirm.select,
    fields: [f("noWait", "bit")],
  },
  {
    name: "confirm.select-ok",
    classId: ClassId.confirm,
    methodId: MethodId.confirm.selectOk,
    fields: [],
  },

  // -- tx (90) --------------------------------------------------------------
  {
    name: "tx.select",
    classId: ClassId.tx,
    methodId: MethodId.tx.select,
    fields: [],
  },
  {
    name: "tx.select-ok",
    classId: ClassId.tx,
    methodId: MethodId.tx.selectOk,
    fields: [],
  },
  {
    name: "tx.commit",
    classId: ClassId.tx,
    methodId: MethodId.tx.commit,
    fields: [],
  },
  {
    name: "tx.commit-ok",
    classId: ClassId.tx,
    methodId: MethodId.tx.commitOk,
    fields: [],
  },
  {
    name: "tx.rollback",
    classId: ClassId.tx,
    methodId: MethodId.tx.rollback,
    fields: [],
  },
  {
    name: "tx.rollback-ok",
    classId: ClassId.tx,
    methodId: MethodId.tx.rollbackOk,
    fields: [],
  },
];

const BY_NAME = new Map<string, MethodDefinition>();
const BY_KEY = new Map<string, MethodDefinition>();
for (const definition of METHODS) {
  BY_NAME.set(definition.name, definition);
  BY_KEY.set(`${definition.classId}:${definition.methodId}`, definition);
}

/** The definition of a method by name, or a `ProtocolError` if there is none. */
export function methodDefinition(name: string): MethodDefinition {
  const definition = BY_NAME.get(name);
  if (definition === undefined) {
    throw new ProtocolError(`no such method: ${name}`);
  }
  return definition;
}

/** A decoded method: which one it is and what it carried. */
export interface DecodedMethod {
  classId: number;
  methodId: number;
  name: string;
  hasContent: boolean;
  args: Record<string, MethodArg>;
}

/** Decodes a method-frame payload. */
export function decodeMethod(payload: Uint8Array): DecodedMethod {
  const r = new ByteReader(payload);
  const { classId, methodId } = readMethodKey(r);
  const definition = BY_KEY.get(`${classId}:${methodId}`);
  if (definition === undefined) {
    throw new ProtocolError(`unknown method ${methodName(classId, methodId)}`);
  }
  const args = readArgs(r, definition);
  if (!r.done) {
    throw new ProtocolError(`${definition.name} left ${r.remaining} bytes unread`);
  }
  return {
    classId,
    methodId,
    name: definition.name,
    hasContent: definition.hasContent === true,
    args,
  };
}

/** Encodes a method-frame payload, filling in reserved fields the caller omitted. */
export function encodeMethod(name: string, args: Record<string, MethodArg> = {}): Uint8Array {
  const definition = methodDefinition(name);
  const w = new ByteWriter(64);
  writeMethodKey(w, definition);
  writeArgs(w, definition, args);
  return w.finish();
}

// Adjacent `bit` fields share an octet, least-significant bit first, and the
// octet is written even when every bit is false — so the reader and writer must
// agree on where a run begins, which they do by both resetting the counter at
// the first non-bit field.
function readArgs(r: ByteReader, definition: MethodDefinition): Record<string, MethodArg> {
  const args: Record<string, MethodArg> = {};
  let octet = 0;
  let bit = 0;
  for (const field of definition.fields) {
    if (field.domain === "bit") {
      if (bit === 0) octet = r.u8();
      args[field.name] = (octet & (1 << bit)) !== 0;
      bit = (bit + 1) % 8;
      continue;
    }
    bit = 0;
    args[field.name] = readArg(r, field.domain, `${definition.name}.${field.name}`);
  }
  return args;
}

function writeArgs(
  w: ByteWriter,
  definition: MethodDefinition,
  args: Record<string, MethodArg>,
): void {
  let octet = 0;
  let bit = 0;
  const flush = () => {
    if (bit > 0) {
      w.u8(octet);
      octet = 0;
      bit = 0;
    }
  };
  for (const field of definition.fields) {
    const what = `${definition.name}.${field.name}`;
    // A bit and a field table each have a default the specification spells out:
    // off, and empty. Neither costs an octet of its own, so leaving one out is
    // the same as sending the default — which is why the client need not say
    // `mandatory: false` on every publish.
    if (field.domain === "bit") {
      const on = args[field.name] ?? field.default ?? false;
      if (typeof on !== "boolean") {
        throw new ProtocolError(`${what} must be a boolean`);
      }
      if (on) octet |= 1 << bit;
      bit += 1;
      if (bit === 8) flush();
      continue;
    }
    const value = args[field.name] ?? field.default;
    if (value === undefined && field.domain === "table") {
      flush();
      writeFieldTable(w, new Map());
      continue;
    }
    if (value === undefined) {
      throw new ProtocolError(`${what} is required`);
    }
    flush();
    writeArg(w, field.domain, value, what);
  }
  flush();
}

function readArg(r: ByteReader, domain: Domain, what: string): MethodArg {
  switch (domain) {
    case "octet":
      return r.u8();
    case "short":
      return r.u16();
    case "long":
      return r.u32();
    case "longlong":
      return r.u64();
    case "shortstr":
      return r.shortstr();
    case "longstr":
      return r.longstr();
    case "table":
      return readFieldTable(r);
    case "bit":
      throw new ProtocolError(`a bit is packed with its neighbours, not read alone: ${what}`);
  }
}

function writeArg(w: ByteWriter, domain: Domain, value: MethodArg, what: string): void {
  switch (domain) {
    case "octet":
      w.u8(count(value, 0xff, what));
      break;
    case "short":
      w.u16(count(value, 0xffff, what));
      break;
    case "long":
      w.u32(count(value, 0xffffffff, what));
      break;
    case "longlong":
      w.u64(tag(value, what));
      break;
    case "shortstr":
      w.shortstr(text(value, what));
      break;
    case "longstr":
      w.longstr(value instanceof Uint8Array ? value : text(value, what));
      break;
    case "table":
      if (!(value instanceof Map)) {
        throw new ProtocolError(`${what} must be a field table`);
      }
      writeFieldTable(w, value);
      break;
    case "bit":
      throw new ProtocolError(`a bit is packed with its neighbours, not written alone: ${what}`);
  }
}

function count(value: MethodArg, max: number, what: string): number {
  if (typeof value !== "number" || !Number.isInteger(value) || value < 0 || value > max) {
    throw new ProtocolError(`${what} must be an integer in 0..${max}`);
  }
  return value;
}

// Delivery tags are 64-bit and grow past what a `number` holds exactly, so a
// bigint is the honest type; a small one is accepted for convenience.
function tag(value: MethodArg, what: string): bigint {
  if (typeof value === "bigint") return value;
  if (typeof value === "number" && Number.isSafeInteger(value) && value >= 0) return BigInt(value);
  throw new ProtocolError(`${what} must be a non-negative integer or a bigint`);
}

function text(value: MethodArg, what: string): string {
  if (typeof value !== "string") {
    throw new ProtocolError(`${what} must be a string`);
  }
  return value;
}
