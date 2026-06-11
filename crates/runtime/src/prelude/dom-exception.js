// DOMException (WebIDL). Pure-JS class so prelude APIs (atob/btoa,
// structuredClone, AbortSignal, …) can throw the right type. This closes the
// JS-class half of the D3a "DOMException not real" note; engine-thrown
// capability errors still arrive as Error until a later phase reconciles them.
(() => {
  "use strict";
  if (typeof globalThis.DOMException === "function") return;

  // Legacy `.code` values for the historical names (0 for the rest).
  const CODES = {
    IndexSizeError: 1,
    HierarchyRequestError: 3,
    WrongDocumentError: 4,
    InvalidCharacterError: 5,
    NoModificationAllowedError: 7,
    NotFoundError: 8,
    NotSupportedError: 9,
    InUseAttributeError: 10,
    InvalidStateError: 11,
    SyntaxError: 12,
    InvalidModificationError: 13,
    NamespaceError: 14,
    InvalidAccessError: 15,
    SecurityError: 18,
    NetworkError: 19,
    AbortError: 20,
    URLMismatchError: 21,
    QuotaExceededError: 22,
    TimeoutError: 23,
    InvalidNodeTypeError: 24,
    DataCloneError: 25,
  };

  class DOMException extends Error {
    constructor(message = "", name = "Error") {
      super(String(message));
      Object.defineProperty(this, "name", {
        value: String(name),
        writable: true,
        configurable: true,
      });
    }

    get code() {
      return CODES[this.name] ?? 0;
    }

    get [Symbol.toStringTag]() {
      return "DOMException";
    }
  }

  // Expose the legacy code constants on the constructor (e.g. DOMException.ABORT_ERR).
  for (const [name, code] of Object.entries(CODES)) {
    const konst = name
      .replace(/Error$/, "_ERR")
      .replace(/([a-z])([A-Z])/g, "$1_$2")
      .toUpperCase();
    Object.defineProperty(DOMException, konst, { value: code, enumerable: true });
  }

  globalThis.DOMException = DOMException;
})();
