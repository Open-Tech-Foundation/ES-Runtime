// URL + URLSearchParams (SPEC §2.4). Parsing/serialization is delegated to the
// host `url_parse`/`url_set` ops (the `url` crate); these wrappers provide the
// JS surface and keep URL.search and URL.searchParams in sync. URLPattern is
// deferred (SPEC §7).
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
    #c;
    #params;

    constructor(url, base) {
      const json =
        base !== undefined
          ? ops.url_parse(String(url), String(base))
          : ops.url_parse(String(url), null);
      if (json === null) throw new TypeError(`Invalid URL: "${url}"`);
      this.#c = JSON.parse(json);
      this.#params = new URLSearchParams(this.#c.search);
      this.#params._attach(this);
    }

    static canParse(url, base) {
      const json =
        base !== undefined
          ? ops.url_parse(String(url), String(base))
          : ops.url_parse(String(url), null);
      return json !== null;
    }

    #apply(component, value) {
      const json = ops.url_set(this.#c.href, component, String(value));
      if (json === null) {
        if (component === "href") throw new TypeError("Invalid URL");
        return; // an invalid component setter is a no-op
      }
      this.#c = JSON.parse(json);
    }

    get href() {
      return this.#c.href;
    }
    set href(v) {
      this.#apply("href", v);
      this.#params._reload(this.#c.search);
    }
    get origin() {
      return this.#c.origin;
    }
    get protocol() {
      return this.#c.protocol;
    }
    set protocol(v) {
      this.#apply("protocol", v);
    }
    get username() {
      return this.#c.username;
    }
    set username(v) {
      this.#apply("username", v);
    }
    get password() {
      return this.#c.password;
    }
    set password(v) {
      this.#apply("password", v);
    }
    get host() {
      return this.#c.host;
    }
    set host(v) {
      this.#apply("host", v);
    }
    get hostname() {
      return this.#c.hostname;
    }
    set hostname(v) {
      this.#apply("hostname", v);
    }
    get port() {
      return this.#c.port;
    }
    set port(v) {
      this.#apply("port", v);
    }
    get pathname() {
      return this.#c.pathname;
    }
    set pathname(v) {
      this.#apply("pathname", v);
    }
    get hash() {
      return this.#c.hash;
    }
    set hash(v) {
      this.#apply("hash", v);
    }
    get search() {
      return this.#c.search;
    }
    set search(v) {
      this.#apply("search", v);
      this.#params._reload(this.#c.search);
    }
    get searchParams() {
      return this.#params;
    }

    // Called by the attached URLSearchParams when it mutates.
    _setSearchFromParams(serialized) {
      this.#apply("search", serialized ? "?" + serialized : "");
    }

    toString() {
      return this.#c.href;
    }
    toJSON() {
      return this.#c.href;
    }
  }

  globalThis.URL = URL;
  globalThis.URLSearchParams = URLSearchParams;
})();
