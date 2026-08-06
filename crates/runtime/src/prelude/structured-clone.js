// structuredClone (SPEC §2.1) — HTML's StructuredSerialize followed by
// StructuredDeserialize, both performed by the engine over V8's ValueSerializer.
//
// This was a hand-written JS deep clone until workers arrived. Keeping it would
// have meant two implementations of one algorithm — this one and the engine
// primitive `postMessage` needs — and they would drift, so that a value cloning
// fine here failed there. The engine's is also the one already correct: it fixes
// two divergences the JS version had, both covered by conformance/structured-clone.js:
//
//   * an ordinary object with a class prototype threw DataCloneError, where the
//     spec serializes its own enumerable properties and rebuilds a plain object;
//   * enumerable symbol-keyed properties were copied, where the spec walks
//     String keys only.
//
// This file now owns three things: the transfer list, the host-object codec
// dispatch (V8 knows JS types, not Blob or MessagePort), and the entry point.
(() => {
  "use strict";
  const HOST_CLONE = __internal.hostClone;
  const CODECS = __internal.hostCodecs;

  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  function cannotClone(what) {
    return new DOMException(
      `${what} could not be cloned.`,
      "DataCloneError",
    );
  }

  // ---- host-object framing -------------------------------------------------
  //
  // The engine hands us one object and expects one Uint8Array back, so the tag
  // has to travel in the bytes for the read side to dispatch on:
  //
  //   [tag length: u8][tag: utf-8][payload: the codec's own bytes]

  globalThis.__structuredWriteHostObject = (object) => {
    const tag = object[HOST_CLONE];
    const codec = CODECS.get(tag);
    // Reachable only if something carries the tag with no codec registered —
    // a forged tag, or a codec fragment that failed to load.
    if (!codec) throw cannotClone(tag ? `A ${tag}` : "The object");

    const payload = codec.write(object);
    const tagBytes = encoder.encode(tag);
    if (tagBytes.length > 255) throw cannotClone(`A ${tag}`);

    const framed = new Uint8Array(1 + tagBytes.length + payload.length);
    framed[0] = tagBytes.length;
    framed.set(tagBytes, 1);
    framed.set(payload, 1 + tagBytes.length);
    return framed;
  };

  globalThis.__structuredReadHostObject = (bytes) => {
    const tagLength = bytes[0];
    const tag = decoder.decode(bytes.subarray(1, 1 + tagLength));
    const codec = CODECS.get(tag);
    // The blob named a type this agent does not have. Only reachable across a
    // version skew, since both ends run the same prelude.
    if (!codec) throw cannotClone(`A ${tag}`);
    return codec.read(bytes.subarray(1 + tagLength));
  };

  // ---- a codec helper the registering fragments share -----------------------
  //
  // Every host type so far is "some JSON metadata, optionally followed by raw
  // bytes", so the framing lives here rather than being repeated per type.

  Object.assign(__internal.hostCodec, {
    // [header length: u32 LE][header: JSON utf-8][payload: raw bytes]
    pack(header, payload = new Uint8Array(0)) {
      const headerBytes = encoder.encode(JSON.stringify(header));
      const out = new Uint8Array(4 + headerBytes.length + payload.length);
      new DataView(out.buffer).setUint32(0, headerBytes.length, true);
      out.set(headerBytes, 4);
      out.set(payload, 4 + headerBytes.length);
      return out;
    },
    unpack(bytes) {
      const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
      const headerLength = view.getUint32(0, true);
      return {
        header: JSON.parse(decoder.decode(bytes.subarray(4, 4 + headerLength))),
        payload: bytes.subarray(4 + headerLength),
      };
    },
  });

  // ---- transfer -------------------------------------------------------------

  // The one place that knows what a transfer list may hold, shared by
  // `structuredClone`, `Worker.postMessage` and `MessagePort.postMessage` — the
  // three call sites the spec gives the same transfer semantics, which should
  // not be three readings of them.
  //
  // Order matters and is the spec's: validate the whole list first, so a bad
  // entry throws before anything is half-detached; serialize while the listed
  // objects are still live; detach only once that has succeeded.
  function serializeWithTransfer(message, list) {
    const ports = [];
    for (const item of list) {
      if (item instanceof ArrayBuffer && typeof item.transfer === "function") {
        if (item.detached) {
          throw new DOMException(
            "An already detached ArrayBuffer could not be transferred.",
            "DataCloneError",
          );
        }
        continue;
      }
      if (globalThis.MessagePort && item instanceof globalThis.MessagePort) {
        ports.push(item);
        continue;
      }
      throw new DOMException(
        "Only ArrayBuffer and MessagePort objects can be transferred.",
        "DataCloneError",
      );
    }

    const transferring = __internal.transferringPorts;
    for (const port of ports) transferring.add(port);
    let bytes;
    try {
      bytes = __structuredSerialize(message);
    } finally {
      // Cleared even on failure: leaving a port marked as "being transferred"
      // would let a *later* clone of it succeed where it should not.
      transferring.clear();
    }

    // `transfer()` is what actually detaches an ArrayBuffer; there is no other
    // way from JS. A port's detach hands its queue over to whoever received it.
    for (const item of list) {
      if (item instanceof ArrayBuffer) item.transfer();
    }
    for (const port of ports) port[__internal.portDetach]();
    return bytes;
  }

  __internal.transfer.serialize = serializeWithTransfer;

  // ---- the entry point ------------------------------------------------------

  globalThis.structuredClone = (value, options) =>
    __structuredDeserialize(
      serializeWithTransfer(
        value,
        options && options.transfer ? [...options.transfer] : [],
      ),
    );
})();
