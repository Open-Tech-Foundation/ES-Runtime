// URLPattern (SPEC §2.4). Parsing and canonicalization are delegated to the
// host `urlpattern_*` ops (the `urlpattern` crate), the same way URL delegates
// to the `url` crate. The ops hand back each component's regular expression as
// *source*; V8 compiles it and the matching happens here.
//
// That split is deliberate (see urlpattern_ops.rs): compiling the components
// Rust-side costs ~600µs per pattern against ~6µs for emitting source, and the
// crate's Rust regex backend silently drops the `ignoreCase` flag. Matching in
// V8 also means `test()` against a URL string makes no host call at all — the
// components come off `URL`, which a router's hot path already needs.
(() => {
  "use strict";
  const ops = globalThis.__ops;

  // Component order, matching the ops' wire shape.
  const COMPONENTS = [
    "protocol",
    "username",
    "password",
    "hostname",
    "port",
    "pathname",
    "search",
    "hash",
  ];

  const KIND_STRING = 0;
  const KIND_INIT = 1;

  // Encodes a URLPatternInit as the flat nine-element array the ops take, so no
  // JS object crosses the boundary. `baseURL` is the ninth slot.
  function encodeInit(init) {
    const fields = COMPONENTS.map((name) =>
      init[name] === undefined || init[name] === null ? null : String(init[name]),
    );
    fields.push(
      init.baseURL === undefined || init.baseURL === null ? null : String(init.baseURL),
    );
    return fields;
  }

  // The component values a URL contributes to matching. Mirrors the crate's
  // `parse_match_input` for the URL case, so a string input needs no host call.
  function valuesFromURL(url) {
    return [
      url.protocol.slice(0, -1), // strip the trailing ":"
      url.username,
      url.password,
      url.hostname,
      url.port,
      url.pathname,
      url.search.slice(1), // strip the leading "?"
      url.hash.slice(1), // strip the leading "#"
    ];
  }

  class URLPattern {
    #patterns = [];
    #regexps = [];
    #names = [];
    #hasRegExpGroups;

    constructor(input = {}, baseURL, options) {
      // Both `new URLPattern(input, base, options)` and
      // `new URLPattern(input, options)` are valid.
      let base = baseURL;
      let opts = options;
      if (typeof baseURL === "object" && baseURL !== null) {
        opts = baseURL;
        base = undefined;
      }
      const isString = typeof input === "string";
      if (!isString && (input === null || typeof input !== "object")) {
        throw new TypeError("URLPattern input must be a string or a URLPatternInit");
      }
      if (!isString && base !== undefined && base !== null) {
        throw new TypeError(
          "specifying both an init object and a separate base URL is not valid",
        );
      }

      const ignoreCase = Boolean(opts && opts.ignoreCase);
      const parsed = ops.urlpattern_parse(
        isString ? KIND_STRING : KIND_INIT,
        isString ? input : encodeInit(input),
        base === undefined || base === null ? null : String(base),
        ignoreCase,
      );

      // The crate emits ECMAScript source; the flags belong here, where the
      // regex is actually compiled. Compiling eagerly also means an invalid
      // custom regex is a construction-time TypeError, as the spec requires,
      // rather than a surprise on first match.
      //
      // `v` (unicodeSets), not `u`: the standard compiles component regexes
      // with it, which is what makes set notation like `[\d&&[0-1]]` inside a
      // custom group work.
      const flags = ignoreCase ? "vi" : "v";
      for (let i = 0; i < COMPONENTS.length; i++) {
        this.#patterns.push(parsed[i * 3]);
        try {
          this.#regexps.push(new RegExp(parsed[i * 3 + 1], flags));
        } catch (e) {
          throw new TypeError(
            `Invalid ${COMPONENTS[i]} pattern: ${(e && e.message) || e}`,
          );
        }
        this.#names.push(parsed[i * 3 + 2]);
      }
      this.#hasRegExpGroups = parsed[COMPONENTS.length * 3];
    }

    get protocol() {
      return this.#patterns[0];
    }
    get username() {
      return this.#patterns[1];
    }
    get password() {
      return this.#patterns[2];
    }
    get hostname() {
      return this.#patterns[3];
    }
    get port() {
      return this.#patterns[4];
    }
    get pathname() {
      return this.#patterns[5];
    }
    get search() {
      return this.#patterns[6];
    }
    get hash() {
      return this.#patterns[7];
    }
    get hasRegExpGroups() {
      return this.#hasRegExpGroups;
    }

    // The eight canonicalized values to match against, or null when the input
    // cannot be resolved to a URL — which is a non-match, not an error.
    #values(input, baseURL) {
      if (typeof input === "string") {
        try {
          return valuesFromURL(new URL(input, baseURL));
        } catch {
          return null;
        }
      }
      if (input === null || typeof input !== "object") {
        throw new TypeError("URLPattern input must be a string or a URLPatternInit");
      }
      if (baseURL !== undefined && baseURL !== null) {
        throw new TypeError(
          "specifying both an init object and a separate base URL is not valid",
        );
      }
      // Canonicalizing a dictionary needs the spec's per-component rules, so
      // this one goes to the host.
      return ops.urlpattern_canonicalize(encodeInit(input));
    }

    test(input = {}, baseURL) {
      const values = this.#values(input, baseURL);
      if (values === null) return false;
      for (let i = 0; i < COMPONENTS.length; i++) {
        if (!this.#regexps[i].test(values[i])) return false;
      }
      return true;
    }

    exec(input = {}, baseURL) {
      const values = this.#values(input, baseURL);
      if (values === null) return null;
      const matches = [];
      for (let i = 0; i < COMPONENTS.length; i++) {
        const match = this.#regexps[i].exec(values[i]);
        if (match === null) return null;
        matches.push(match);
      }
      // `inputs` echoes the arguments as given.
      const result = { inputs: baseURL !== undefined ? [input, baseURL] : [input] };
      for (let i = 0; i < COMPONENTS.length; i++) {
        const names = this.#names[i];
        const match = matches[i];
        const groups = {};
        for (let g = 0; g < names.length; g++) {
          // A group that did not participate stays `undefined`, not "".
          groups[names[g]] = match[g + 1];
        }
        result[COMPONENTS[i]] = { input: values[i], groups };
      }
      return result;
    }
  }

  Object.defineProperty(URLPattern.prototype, Symbol.toStringTag, {
    value: "URLPattern",
    configurable: true,
  });
  globalThis.URLPattern = URLPattern;
})();
