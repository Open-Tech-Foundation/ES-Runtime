(function() {
  "use strict";

  const escapeRe = /([.+*?^${}()|[\]\\])/g;

  // Compiles a string pattern with wildcards (*) and named groups (:id) into a RegExp.
  // E.g. "/api/:id/*" -> /^\/api\/([^/]+)\/(.*)$/
  function compile(str, delimiter, ignoreCase) {
    if (str === '*' || str === undefined) return { re: /.*/, names: [] };
    if (str === '') return { re: /^$/, names: [] };

    const names = [];
    let regexStr = str.replace(escapeRe, '\\$1');

    // Replace :paramName (we didn't escape colon)
    regexStr = regexStr.replace(/:([a-zA-Z0-9_]+)/g, (_, name) => {
      names.push(name);
      return `([^${delimiter}]+)`;
    });

    // Replace \*
    regexStr = regexStr.replace(/\\\*/g, () => {
      names.push(names.length);
      return '(.*)';
    });

    const flags = ignoreCase ? 'iu' : 'u';
    return { re: new RegExp('^' + regexStr + '$', flags), names };
  }

  // Simplified string parser: figures out the parts based on the presence of base and slashes.
  function resolvePattern(input, base) {
    let res = { protocol: '*', username: '*', password: '*', hostname: '*', port: '*', pathname: '*', search: '*', hash: '*' };

    if (typeof input === 'object' && input !== null) {
      Object.assign(res, input);
      if (base) {
        let b = new URL(base);
        if (!input.protocol) res.protocol = b.protocol.replace(':', '');
        if (!input.hostname) res.hostname = b.hostname;
        if (!input.port) res.port = b.port;
      }
      return res;
    }

    // String parsing
    let b = base ? new URL(base) : null;

    if (input.startsWith('/')) {
      if (b) {
        res.protocol = b.protocol.replace(':', '');
        res.hostname = b.hostname;
        res.port = b.port;
      }
      let qIndex = input.indexOf('?');
      let hIndex = input.indexOf('#');
      if (qIndex === -1 && hIndex === -1) {
        res.pathname = input;
      } else {
         let endPath = qIndex !== -1 ? qIndex : hIndex;
         res.pathname = input.slice(0, endPath);
         if (qIndex !== -1) {
           res.search = input.slice(qIndex + 1, hIndex !== -1 ? hIndex : input.length);
         }
         if (hIndex !== -1) {
           res.hash = input.slice(hIndex + 1);
         }
      }
      return res;
    }

    // Attempt to parse as an absolute URL pattern
    // Because input can contain `*` and `:id`, standard `new URL(input)` will fail if it violates URL syntax.
    // Instead, we manually split the absolute parts if there's a protocol.
    const protoMatch = input.match(/^([a-zA-Z0-9+.-]+):\/\//);
    if (protoMatch) {
      res.protocol = protoMatch[1];
      let rest = input.slice(protoMatch[0].length);
      
      let pathStart = rest.indexOf('/');
      let auth = pathStart === -1 ? rest : rest.slice(0, pathStart);
      let path = pathStart === -1 ? '' : rest.slice(pathStart);

      // Auth split
      let atIndex = auth.indexOf('@');
      if (atIndex !== -1) {
        let credentials = auth.slice(0, atIndex);
        auth = auth.slice(atIndex + 1);
        let colon = credentials.indexOf(':');
        if (colon !== -1) {
          res.username = credentials.slice(0, colon);
          res.password = credentials.slice(colon + 1);
        } else {
          res.username = credentials;
        }
      }
      let portColon = auth.lastIndexOf(':');
      if (portColon !== -1 && !auth.endsWith(']')) {
        res.hostname = auth.slice(0, portColon);
        res.port = auth.slice(portColon + 1);
      } else {
        res.hostname = auth;
      }

      // Path split
      let qIndex = path.indexOf('?');
      let hIndex = path.indexOf('#');
      if (qIndex === -1 && hIndex === -1) {
        res.pathname = path;
      } else {
         let endPath = qIndex !== -1 ? qIndex : hIndex;
         res.pathname = path.slice(0, endPath);
         if (qIndex !== -1) {
           res.search = path.slice(qIndex + 1, hIndex !== -1 ? hIndex : path.length);
         }
         if (hIndex !== -1) {
           res.hash = path.slice(hIndex + 1);
         }
      }
      return res;
    }

    // If no protocol and no leading slash, and we have a base, resolve against base
    if (b) {
      res.protocol = b.protocol.replace(':', '');
      res.hostname = b.hostname;
      res.port = b.port;
      
      let qIndex = input.indexOf('?');
      let hIndex = input.indexOf('#');
      let endPath = input.length;
      if (qIndex !== -1) endPath = qIndex;
      else if (hIndex !== -1) endPath = hIndex;

      let p = input.slice(0, endPath);
      let bDir = b.pathname.substring(0, b.pathname.lastIndexOf('/') + 1);
      res.pathname = bDir + p; // very basic resolution
      
      if (qIndex !== -1) res.search = input.slice(qIndex + 1, hIndex !== -1 ? hIndex : input.length);
      if (hIndex !== -1) res.hash = input.slice(hIndex + 1);
      
      return res;
    }

    // Default basic fallback
    res.pathname = input;
    return res;
  }

  // Global cache to avoid recompiling the same patterns.
  const regexCache = new Map();

  function getCompiledComponent(str, delimiter, ignoreCase) {
    const key = `${str}|${delimiter}|${ignoreCase}`;
    let comp = regexCache.get(key);
    if (!comp) {
      comp = compile(str, delimiter, ignoreCase);
      regexCache.set(key, comp);
    }
    return comp;
  }

  class URLPattern {
    #components = {};

    constructor(input, baseURL, options = {}) {
      const parts = resolvePattern(input, baseURL);
      this.protocol = parts.protocol;
      this.username = parts.username;
      this.password = parts.password;
      this.hostname = parts.hostname;
      this.port = parts.port;
      this.pathname = parts.pathname;
      this.search = parts.search;
      this.hash = parts.hash;

      const ignoreCase = options.ignoreCase === true;

      this.#components.protocol = getCompiledComponent(this.protocol, ':', ignoreCase);
      this.#components.username = getCompiledComponent(this.username, '/', ignoreCase);
      this.#components.password = getCompiledComponent(this.password, '/', ignoreCase);
      this.#components.hostname = getCompiledComponent(this.hostname, '/', ignoreCase);
      this.#components.port     = getCompiledComponent(this.port, '/', ignoreCase);
      this.#components.pathname = getCompiledComponent(this.pathname, '/', ignoreCase);
      this.#components.search   = getCompiledComponent(this.search, '#', ignoreCase);
      this.#components.hash     = getCompiledComponent(this.hash, '', ignoreCase);
    }

    test(input, baseURL) {
      let url;
      try { url = new URL(input, baseURL); } catch { return false; }
      if (!this.#components.protocol.re.test(url.protocol.replace(':', ''))) return false;
      if (!this.#components.hostname.re.test(url.hostname)) return false;
      if (!this.#components.port.re.test(url.port)) return false;
      if (!this.#components.pathname.re.test(url.pathname)) return false;
      if (!this.#components.search.re.test(url.search.replace('?', ''))) return false;
      if (!this.#components.hash.re.test(url.hash.replace('#', ''))) return false;
      if (!this.#components.username.re.test(url.username)) return false;
      if (!this.#components.password.re.test(url.password)) return false;
      return true;
    }

    exec(input, baseURL) {
      let url;
      try { url = new URL(input, baseURL); } catch { return null; }
      
      const parts = {
        protocol: url.protocol.replace(':', ''),
        username: url.username,
        password: url.password,
        hostname: url.hostname,
        port: url.port,
        pathname: url.pathname,
        search: url.search.replace('?', ''),
        hash: url.hash.replace('#', '')
      };

      const result = { inputs: baseURL ? [input, baseURL] : [input] };
      for (const [key, val] of Object.entries(parts)) {
        const comp = this.#components[key];
        const match = comp.re.exec(val);
        if (!match) return null;
        
        const groups = {};
        for (let i = 0; i < comp.names.length; i++) {
          groups[comp.names[i]] = match[i + 1] || "";
        }
        result[key] = { input: val, groups };
      }
      return result;
    }
  }

  globalThis.URLPattern = URLPattern;
})();
