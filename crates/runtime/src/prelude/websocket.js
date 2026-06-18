// WebSocket — the classic WHATWG interface (DECISIONS D29). A prelude global
// (Minimum Common Web surface, like `fetch`), NOT a `runtime:` module. It
// bridges the interface's push-based `message`/`close` events onto our
// pull-based op seam: an internal receive-pump keeps exactly one `ws_recv` op
// outstanding per open socket and dispatches each resolved frame, then re-arms.
// Every step rides the embedder's tick (D4) — no owned loop. The connect is
// gated on `Capability::Net` in the op; with no `WebSocketProvider` installed,
// `ws_connect` rejects and the socket fails with an `error`/`close` (1006).
(() => {
  "use strict";

  const ops = globalThis.__ops;
  const encoder = new TextEncoder();

  const CONNECTING = 0;
  const OPEN = 1;
  const CLOSING = 2;
  const CLOSED = 3;

  // Validate and serialize the URL: ws:/wss: only, no fragment (DECISIONS D29).
  function parseUrl(input) {
    let url;
    try {
      url = new URL(input);
    } catch {
      throw new SyntaxError(
        `Failed to construct 'WebSocket': The URL '${input}' is invalid.`,
      );
    }
    if (url.protocol !== "ws:" && url.protocol !== "wss:") {
      throw new SyntaxError(
        `Failed to construct 'WebSocket': The URL's scheme must be either 'ws' or 'wss'. '${url.protocol.slice(0, -1)}' is not allowed.`,
      );
    }
    if (url.hash !== "") {
      throw new SyntaxError(
        `Failed to construct 'WebSocket': The URL contains a fragment identifier ('${url.hash.slice(1)}'). Fragment identifiers are not allowed in WebSocket URLs.`,
      );
    }
    return url;
  }

  // RFC 6455 subprotocol tokens: non-empty, no separators/controls, no dupes.
  // Reject any space/control/non-ASCII char or an HTTP separator.
  const NON_TOKEN = /[^!-~]|[()<>@,;:\\"/[\]?={}]/;
  function normalizeProtocols(protocols) {
    let list;
    if (protocols === undefined) list = [];
    else if (typeof protocols === "string") list = [protocols];
    else list = Array.from(protocols, String);
    const seen = new Set();
    for (const p of list) {
      if (p === "" || NON_TOKEN.test(p) || seen.has(p.toLowerCase())) {
        throw new SyntaxError(
          `Failed to construct 'WebSocket': The subprotocol '${p}' is invalid or duplicated.`,
        );
      }
      seen.add(p.toLowerCase());
    }
    return list;
  }

  // Copy bytes into a fresh, exactly-sized ArrayBuffer (binaryType: arraybuffer).
  function toArrayBuffer(bytes) {
    return bytes.slice().buffer;
  }

  class WebSocket extends EventTarget {
    #url;
    #origin;
    #readyState = CONNECTING;
    #protocol = "";
    #extensions = "";
    #binaryType = "blob";
    #bufferedAmount = 0;
    #id = null;
    #closeRequested = null; // { code, reason } if close() ran while CONNECTING
    #handlers = { open: null, message: null, error: null, close: null };

    constructor(url, protocols) {
      super();
      const u = parseUrl(String(url));
      this.#url = u.href;
      this.#origin = u.origin && u.origin !== "null" ? u.origin : `${u.protocol}//${u.host}`;
      const protos = normalizeProtocols(protocols);
      // Kick off the async handshake; we stay CONNECTING until it settles. A
      // synchronous op failure (e.g. the Net capability is denied at dispatch)
      // is routed to the same async failure path so listeners attached right
      // after construction still observe `error`/`close` (DECISIONS D29).
      try {
        ops.ws_connect(this.#url, protos).then(
          (info) => this.#onConnected(info),
          () => this.#abnormalClose(),
        );
      } catch {
        queueMicrotask(() => this.#abnormalClose());
      }
    }

    get url() {
      return this.#url;
    }
    get readyState() {
      return this.#readyState;
    }
    get bufferedAmount() {
      return this.#bufferedAmount;
    }
    get protocol() {
      return this.#protocol;
    }
    get extensions() {
      return this.#extensions;
    }
    get binaryType() {
      return this.#binaryType;
    }
    set binaryType(value) {
      // IDL enum: only "blob" / "arraybuffer" are accepted; others are ignored.
      if (value === "blob" || value === "arraybuffer") this.#binaryType = value;
    }

    get onopen() {
      return this.#handlers.open;
    }
    set onopen(fn) {
      this.#setHandler("open", fn);
    }
    get onmessage() {
      return this.#handlers.message;
    }
    set onmessage(fn) {
      this.#setHandler("message", fn);
    }
    get onerror() {
      return this.#handlers.error;
    }
    set onerror(fn) {
      this.#setHandler("error", fn);
    }
    get onclose() {
      return this.#handlers.close;
    }
    set onclose(fn) {
      this.#setHandler("close", fn);
    }

    send(data) {
      if (this.#readyState === CONNECTING) {
        throw new DOMException(
          "Failed to execute 'send' on 'WebSocket': Still in CONNECTING state.",
          "InvalidStateError",
        );
      }
      if (this.#readyState !== OPEN) return; // CLOSING/CLOSED: drop per spec

      // Blob is read asynchronously; bufferedAmount tracks its byte size until
      // the send op resolves (best-effort accounting, DECISIONS D29).
      if (data instanceof Blob) {
        const size = data.size;
        this.#bufferedAmount += size;
        data
          .arrayBuffer()
          .then((buf) => ops.ws_send(this.#id, new Uint8Array(buf)))
          .catch(() => {})
          .finally(() => {
            this.#bufferedAmount -= size;
          });
        return;
      }

      let payload;
      let size;
      if (typeof data === "string") {
        payload = data;
        size = encoder.encode(data).length;
      } else if (ArrayBuffer.isView(data)) {
        payload = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
        size = payload.byteLength;
      } else if (data instanceof ArrayBuffer) {
        payload = new Uint8Array(data);
        size = payload.byteLength;
      } else {
        payload = String(data);
        size = encoder.encode(payload).length;
      }
      this.#bufferedAmount += size;
      Promise.resolve(ops.ws_send(this.#id, payload))
        .catch(() => {})
        .finally(() => {
          this.#bufferedAmount -= size;
        });
    }

    close(code, reason) {
      if (code !== undefined && code !== 1000 && !(code >= 3000 && code <= 4999)) {
        throw new DOMException(
          `Failed to execute 'close' on 'WebSocket': The close code must be either 1000, or between 3000 and 4999. ${code} is neither.`,
          "InvalidAccessError",
        );
      }
      const reasonStr = reason === undefined ? "" : String(reason);
      if (encoder.encode(reasonStr).length > 123) {
        throw new DOMException(
          "Failed to execute 'close' on 'WebSocket': The close reason must not be greater than 123 UTF-8 bytes.",
          "SyntaxError",
        );
      }
      const c = code === undefined ? null : code;

      if (this.#readyState === CLOSING || this.#readyState === CLOSED) return;
      if (this.#readyState === CONNECTING) {
        // Close requested before the handshake finished: fail the connection.
        this.#readyState = CLOSING;
        this.#closeRequested = { code: c, reason: reasonStr };
        return;
      }
      this.#readyState = CLOSING;
      Promise.resolve(ops.ws_close(this.#id, c, reasonStr)).catch(() => {});
    }

    #setHandler(name, value) {
      const current = this.#handlers[name];
      if (current) this.removeEventListener(name, current);
      const fn = typeof value === "function" ? value : null;
      this.#handlers[name] = fn;
      if (fn) this.addEventListener(name, fn);
    }

    #onConnected(info) {
      this.#id = info.id;
      this.#protocol = info.protocol ?? "";
      this.#extensions = info.extensions ?? "";
      if (this.#closeRequested) {
        // close() ran during CONNECTING — start the handshake, fire no `open`.
        const { code, reason } = this.#closeRequested;
        Promise.resolve(ops.ws_close(this.#id, code, reason)).catch(() => {});
        this.#pump();
        return;
      }
      this.#readyState = OPEN;
      this.dispatchEvent(new Event("open"));
      this.#pump();
    }

    // The receive-pump: one outstanding ws_recv at a time, re-armed after each
    // dispatch. Each resolved frame is one pending op drained on the tick (D4).
    async #pump() {
      try {
        for (;;) {
          if (this.#id === null || this.#readyState === CLOSED) return;
          const frame = await ops.ws_recv(this.#id);
          if (frame === null) {
            this.#abnormalClose();
            return;
          }
          if (frame.type === "close") {
            this.#readyState = CLOSED;
            this.dispatchEvent(
              new CloseEvent("close", {
                wasClean: true,
                code: frame.code,
                reason: frame.reason,
              }),
            );
            return;
          }
          if (this.#readyState !== OPEN) continue; // post-close straggler
          let payload;
          if (frame.type === "text") {
            payload = frame.data;
          } else {
            payload =
              this.#binaryType === "arraybuffer"
                ? toArrayBuffer(frame.data)
                : new Blob([frame.data]);
          }
          this.dispatchEvent(
            new MessageEvent("message", { data: payload, origin: this.#origin }),
          );
        }
      } catch {
        this.#abnormalClose();
      }
    }

    // Connection dropped without a clean handshake (or the handshake failed):
    // an `error` followed by a non-clean `close` with code 1006 (DECISIONS D29).
    #abnormalClose() {
      if (this.#readyState === CLOSED) return;
      const wasClosing = this.#readyState === CLOSING;
      this.#readyState = CLOSED;
      if (!wasClosing) this.dispatchEvent(new Event("error"));
      this.dispatchEvent(
        new CloseEvent("close", { wasClean: false, code: 1006, reason: "" }),
      );
    }
  }

  // readyState constants live on both the interface and instances (per IDL).
  for (const [name, value] of [
    ["CONNECTING", CONNECTING],
    ["OPEN", OPEN],
    ["CLOSING", CLOSING],
    ["CLOSED", CLOSED],
  ]) {
    Object.defineProperty(WebSocket, name, { value, enumerable: true });
    Object.defineProperty(WebSocket.prototype, name, { value, enumerable: true });
  }

  globalThis.WebSocket = WebSocket;
})();
