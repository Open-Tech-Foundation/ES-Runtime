// navigator (SPEC §2.1), the WinterTC Minimum Common API's one required member:
// `navigator.userAgent`, a string identifying the runtime and its version.
//
// This is the whole interface. The browser `Navigator` is a grab-bag of
// document, device and permission APIs that mean nothing in a server runtime —
// `language`, `onLine`, `clipboard`, `geolocation` — and inventing plausible
// answers for them would be worse than their absence: a feature check would
// pass and the behaviour behind it would be a lie.
//
// `hardwareConcurrency` is the exception, and it earned its place when workers
// arrived: it is the number a pool is sized from, and a program that cannot ask
// simply guesses worse. It comes from the `Process` provider — host
// information, not a constant — and is ungated for the same reason `platform`
// is: it describes the machine the guest is already running on.
//
// The version is substituted from the crate's own `CARGO_PKG_VERSION` when the
// prelude is assembled (see `prelude::source`), so the string cannot drift from
// the binary that reports it.
//
// A real class rather than an object literal, so `navigator` is an instance of
// a branded `Navigator` interface with `userAgent` on the prototype — what
// WebIDL describes, and what an inspector or a type check expects.
(() => {
  "use strict";
  // Guards the constructor: `navigator` is exposed as an instance, and
  // `new Navigator()` is not something a script may do.
  const INTERNAL = Symbol("Navigator.construct");
  const USER_AGENT = "ES-Runtime/__ES_RUNTIME_VERSION__";

  class Navigator {
    constructor(key) {
      if (key !== INTERNAL) throw new TypeError("Illegal constructor");
    }
    get userAgent() {
      return USER_AGENT;
    }
    get hardwareConcurrency() {
      // Read on access, never at load: this fragment is baked into the startup
      // snapshot, where there is no host to ask, and the answer would then be
      // frozen into every launch on every machine.
      return __ops.process_concurrency();
    }
  }

  Object.defineProperty(Navigator.prototype, Symbol.toStringTag, {
    value: "Navigator",
    configurable: true,
  });

  globalThis.Navigator = Navigator;
  globalThis.navigator = new Navigator(INTERNAL);
})();
