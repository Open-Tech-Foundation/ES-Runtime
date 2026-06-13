// URL + URLSearchParams (SPEC §2.4). Parsing/serialization is delegated to the
// host `url_parse`/`url_set` ops (the `url` crate); these wrappers provide the
// JS surface and keep URL.search and URL.searchParams in sync. URLPattern is
// deferred (SPEC §7).
//
// Wire shape (see url_ops.rs): the ops return `[href, o0..o14]` — the canonical
// href plus fifteen component offsets (UTF-16 code-unit indices). Every getter
// below is a lazy `href.slice(...)`; nothing is materialized for components the
// script never reads. Offset map (`u[0]` is href, offsets shifted +1):
//   protocol  u[0].slice(0, u[1]+1)      hostname  u[0].slice(u[6], u[7])
//   username  u[0].slice(u[2], u[3])     port      u[0].slice(u[8], u[9])
//   password  u[0].slice(u[4], u[5])     pathname  u[0].slice(u[10], u[11])
//   host      u[0].slice(u[6], u[9])     search    "?"-prefixed u[12]..u[13]
//                                        hash      "#"-prefixed u[14]..u[15]
// u[12]/u[14] sit *after* the "?"/"#", so a present-but-empty query/fragment
// reads back as "" (WHATWG) and the delimiter is at index u[..]-1 when present.
(() => {
  "use strict";
  const ops = globalThis.__ops;

  // application/x-www-form-urlencoded encode/decode.
  function encode(s) {
    return encodeURIComponent(s).replace(/%20/g, "+").replace(/[!'()*~]/g, (c) =>
      "%" + c.charCodeAt(0).toString(16).toUpperCase(),
    );
  }
  function decode(s) {
    try {
      return decodeURIComponent(s.replace(/\+/g, " "));
    } catch {
      return s.replace(/\+/g, " ");
    }
  }

  class URLSearchParams {
    #list = [];
    #url = null;

    constructor(init) {
      if (init === undefined || init === null || init === "") return;
      if (init instanceof URLSearchParams) {
        this.#list = init.#list.map((p) => [p[0], p[1]]);
      } else if (typeof init === "string") {
        this.#parse(init);
      } else if (Array.isArray(init)) {
        for (const pair of init) {
          if (pair.length !== 2) {
            throw new TypeError("URLSearchParams pair must have two elements");
          }
          this.#list.push([String(pair[0]), String(pair[1])]);
        }
      } else if (typeof init === "object") {
        for (const key of Object.keys(init)) {
          this.#list.push([key, String(init[key])]);
        }
      }
    }

    #parse(str) {
      const s = str.replace(/^\?/, "");
      if (!s) return;
      for (const part of s.split("&")) {
        if (part === "") continue;
        const eq = part.indexOf("=");
        const name = eq === -1 ? part : part.slice(0, eq);
        const value = eq === -1 ? "" : part.slice(eq + 1);
        this.#list.push([decode(name), decode(value)]);
      }
    }

    #serialize() {
      return this.#list.map(([n, v]) => `${encode(n)}=${encode(v)}`).join("&");
    }

    #notify() {
      if (this.#url) this.#url._setSearchFromParams(this.#serialize());
    }

    // Internal hooks used by URL.
    _attach(url) {
      this.#url = url;
    }
    _reload(searchString) {
      this.#list = [];
      this.#parse(searchString);
    }

    get size() {
      return this.#list.length;
    }
    append(name, value) {
      this.#list.push([String(name), String(value)]);
      this.#notify();
    }
    delete(name, value) {
      const n = String(name);
      const matchValue = value !== undefined ? String(value) : undefined;
      this.#list = this.#list.filter(
        ([k, v]) => !(k === n && (matchValue === undefined || v === matchValue)),
      );
      this.#notify();
    }
    get(name) {
      const n = String(name);
      const hit = this.#list.find(([k]) => k === n);
      return hit ? hit[1] : null;
    }
    getAll(name) {
      const n = String(name);
      return this.#list.filter(([k]) => k === n).map(([, v]) => v);
    }
    has(name, value) {
      const n = String(name);
      const matchValue = value !== undefined ? String(value) : undefined;
      return this.#list.some(
        ([k, v]) => k === n && (matchValue === undefined || v === matchValue),
      );
    }
    set(name, value) {
      const n = String(name);
      const val = String(value);
      let found = false;
      this.#list = this.#list.filter(([k]) => {
        if (k !== n) return true;
        if (!found) {
          found = true;
          return true;
        }
        return false;
      });
      const hit = this.#list.find(([k]) => k === n);
      if (hit) hit[1] = val;
      else this.#list.push([n, val]);
      this.#notify();
    }
    sort() {
      this.#list.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
      this.#notify();
    }
    forEach(callback, thisArg) {
      for (const [k, v] of this.#list) callback.call(thisArg, v, k, this);
    }
    *entries() {
      for (const [k, v] of this.#list) yield [k, v];
    }
    *keys() {
      for (const [k] of this.#list) yield k;
    }
    *values() {
      for (const [, v] of this.#list) yield v;
    }
    [Symbol.iterator]() {
      return this.entries();
    }
    toString() {
      return this.#serialize();
    }
  }

  class URL {
    #u; // [href, o0..o14] — see the header comment for the offset map.
    #params; // lazily-built URLSearchParams (first `.searchParams` access)
    #origin; // lazily-fetched origin (first `.origin` access)

    constructor(url, base) {
      const u =
        base !== undefined
          ? ops.url_parse(String(url), String(base))
          : ops.url_parse(String(url), null);
      if (u === null) throw new TypeError(`Invalid URL: "${url}"`);
      this.#u = u;
    }

    static canParse(url, base) {
      const u =
        base !== undefined
          ? ops.url_parse(String(url), String(base))
          : ops.url_parse(String(url), null);
      return u !== null;
    }

    #apply(component, value) {
      const u = ops.url_set(this.#u[0], component, String(value));
      if (u === null) {
        if (component === "href") throw new TypeError("Invalid URL");
        return; // an invalid component setter is a no-op
      }
      this.#u = u;
      this.#origin = undefined;
    }

    get href() {
      return this.#u[0];
    }
    set href(v) {
      this.#apply("href", v);
      // Only resync if a URLSearchParams has actually been materialized;
      // otherwise it will be built from the current search on first access.
      if (this.#params !== undefined) this.#params._reload(this.search);
    }
    get origin() {
      if (this.#origin === undefined) this.#origin = ops.url_origin(this.#u[0]);
      return this.#origin;
    }
    get protocol() {
      const u = this.#u;
      return u[0].slice(0, u[1] + 1);
    }
    set protocol(v) {
      this.#apply("protocol", v);
    }
    get username() {
      const u = this.#u;
      return u[0].slice(u[2], u[3]);
    }
    set username(v) {
      this.#apply("username", v);
    }
    get password() {
      const u = this.#u;
      return u[0].slice(u[4], u[5]);
    }
    set password(v) {
      this.#apply("password", v);
    }
    get host() {
      const u = this.#u;
      return u[0].slice(u[6], u[9]);
    }
    set host(v) {
      this.#apply("host", v);
    }
    get hostname() {
      const u = this.#u;
      return u[0].slice(u[6], u[7]);
    }
    set hostname(v) {
      this.#apply("hostname", v);
    }
    get port() {
      const u = this.#u;
      return u[0].slice(u[8], u[9]);
    }
    set port(v) {
      this.#apply("port", v);
    }
    get pathname() {
      const u = this.#u;
      return u[0].slice(u[10], u[11]);
    }
    set pathname(v) {
      this.#apply("pathname", v);
    }
    get hash() {
      const u = this.#u;
      return u[14] < u[15] ? u[0].slice(u[14] - 1, u[15]) : "";
    }
    set hash(v) {
      this.#apply("hash", v);
    }
    get search() {
      const u = this.#u;
      return u[12] < u[13] ? u[0].slice(u[12] - 1, u[13]) : "";
    }
    set search(v) {
      this.#apply("search", v);
      if (this.#params !== undefined) this.#params._reload(this.search);
    }
    get searchParams() {
      // Lazily materialize + attach on first access.
      if (this.#params === undefined) {
        this.#params = new URLSearchParams(this.search);
        this.#params._attach(this);
      }
      return this.#params;
    }

    // Called by the attached URLSearchParams when it mutates.
    _setSearchFromParams(serialized) {
      this.#apply("search", serialized ? "?" + serialized : "");
    }

    toString() {
      return this.#u[0];
    }
    toJSON() {
      return this.#u[0];
    }
  }

  globalThis.URL = URL;
  globalThis.URLSearchParams = URLSearchParams;
})();
