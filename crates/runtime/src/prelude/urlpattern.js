// URLPattern (SPEC §2.4). The pattern syntax is the path-to-regexp dialect the
// URLPattern standard adopts, compiled per component to a RegExp:
//
//   :name            a named group, matching one segment
//   :name(\d+)       a named group with a custom regex
//   (\d+)            an anonymous group with a custom regex, named by index
//   *                a full wildcard, named by index
//   ?  +  *          modifiers on the preceding group: optional, one-or-more,
//                    zero-or-more
//   {…}              a group, so a modifier can cover literal text too
//   \x               an escaped literal
//
// A group directly preceded by the component's prefix character (`/` for
// pathname, `.` for hostname) absorbs it, so `/a/:b?` matches `/a` as well as
// `/a/x` — the separator disappears with the segment rather than being left
// dangling.
(function () {
  "use strict";

  // Per-component matching rules. `delimiter` bounds a plain `:name` group;
  // `prefix` is the character a group absorbs when modified.
  const COMPONENTS = {
    protocol: { delimiter: "", prefix: "" },
    username: { delimiter: "", prefix: "" },
    password: { delimiter: "", prefix: "" },
    hostname: { delimiter: ".", prefix: "." },
    port: { delimiter: "", prefix: "" },
    pathname: { delimiter: "/", prefix: "/" },
    search: { delimiter: "", prefix: "" },
    hash: { delimiter: "", prefix: "" },
  };

  // Only the characters that are actually special. Escaping others (`:`, `=`,
  // `!`) would be an invalid escape under the `u` flag, not a harmless one.
  const escapeRegex = (s) => s.replace(/([.+*?^${}()|[\]/\\])/g, "\\$1");

  // ---- Lexer ---------------------------------------------------------------

  function lex(str) {
    const tokens = [];
    let i = 0;
    while (i < str.length) {
      const c = str[i];
      if (c === "\\") {
        if (i + 1 >= str.length) throw new TypeError("Pattern ends with a trailing backslash");
        tokens.push({ type: "CHAR", value: str[i + 1] });
        i += 2;
        continue;
      }
      if (c === "*" || c === "+" || c === "?") {
        tokens.push({ type: "MODIFIER", value: c });
        i++;
        continue;
      }
      if (c === "{") {
        tokens.push({ type: "OPEN" });
        i++;
        continue;
      }
      if (c === "}") {
        tokens.push({ type: "CLOSE" });
        i++;
        continue;
      }
      if (c === ":") {
        let j = i + 1;
        let name = "";
        while (j < str.length && /[A-Za-z0-9_$]/.test(str[j])) name += str[j++];
        if (name === "") throw new TypeError("Pattern has a ':' with no group name");
        tokens.push({ type: "NAME", value: name });
        i = j;
        continue;
      }
      if (c === "(") {
        // Balanced scan, so a custom regex may itself contain groups.
        let depth = 1;
        let j = i + 1;
        let pattern = "";
        while (j < str.length) {
          if (str[j] === "\\") {
            pattern += str[j] + (str[j + 1] ?? "");
            j += 2;
            continue;
          }
          if (str[j] === "(") depth++;
          else if (str[j] === ")") {
            depth--;
            if (depth === 0) break;
          }
          pattern += str[j];
          j++;
        }
        if (depth !== 0) throw new TypeError("Pattern has an unbalanced '('");
        if (pattern === "") throw new TypeError("Pattern has an empty group");
        tokens.push({ type: "PATTERN", value: pattern });
        i = j + 1;
        continue;
      }
      tokens.push({ type: "CHAR", value: c });
      i++;
    }
    tokens.push({ type: "END" });
    return tokens;
  }

  // ---- Parser --------------------------------------------------------------

  // Produces a flat list of `{ type: "literal" }` and `{ type: "group" }` parts.
  function parse(str, component) {
    const { delimiter, prefix: prefixChar } = component;
    const defaultPattern = delimiter ? `[^${escapeRegex(delimiter)}]+?` : "[^]+?";
    const tokens = lex(str);
    const parts = [];
    let literal = "";
    let index = 0;
    let i = 0;

    const flush = () => {
      if (literal !== "") {
        parts.push({ type: "literal", value: literal });
        literal = "";
      }
    };
    const takeModifier = () => {
      if (tokens[i].type === "MODIFIER") return tokens[i++].value;
      return "";
    };
    const takeText = () => {
      let out = "";
      while (tokens[i].type === "CHAR") out += tokens[i++].value;
      return out;
    };

    while (tokens[i].type !== "END") {
      const token = tokens[i];

      if (token.type === "CHAR") {
        literal += token.value;
        i++;
        continue;
      }

      if (token.type === "NAME" || token.type === "PATTERN") {
        let name;
        let pattern;
        if (token.type === "NAME") {
          name = token.value;
          i++;
          pattern = tokens[i].type === "PATTERN" ? tokens[i++].value : defaultPattern;
        } else {
          name = index++;
          pattern = token.value;
          i++;
        }
        // A trailing prefix character belongs to the group, not the literal
        // before it, so a modifier can take it away with the group.
        let prefix = "";
        if (prefixChar && literal.endsWith(prefixChar)) {
          prefix = prefixChar;
          literal = literal.slice(0, -prefixChar.length);
        }
        flush();
        parts.push({ type: "group", name, pattern, prefix, suffix: "", modifier: takeModifier() });
        continue;
      }

      // A bare `*` is a full wildcard; one directly after a group was already
      // consumed above as that group's modifier.
      if (token.type === "MODIFIER" && token.value === "*") {
        i++;
        let prefix = "";
        if (prefixChar && literal.endsWith(prefixChar)) {
          prefix = prefixChar;
          literal = literal.slice(0, -prefixChar.length);
        }
        flush();
        parts.push({
          type: "group",
          name: index++,
          pattern: "[^]*?",
          prefix,
          suffix: "",
          modifier: takeModifier(),
          wildcard: true,
        });
        continue;
      }

      if (token.type === "OPEN") {
        i++;
        const prefix = takeText();
        let name;
        let pattern;
        if (tokens[i].type === "NAME") {
          name = tokens[i++].value;
          pattern = tokens[i].type === "PATTERN" ? tokens[i++].value : defaultPattern;
        } else if (tokens[i].type === "PATTERN") {
          name = index++;
          pattern = tokens[i++].value;
        }
        const suffix = takeText();
        if (tokens[i].type !== "CLOSE") throw new TypeError("Pattern has an unbalanced '{'");
        i++;
        flush();
        parts.push({
          type: "group",
          name: name === undefined ? null : name,
          pattern: pattern ?? null,
          prefix,
          suffix,
          modifier: takeModifier(),
        });
        continue;
      }

      if (token.type === "CLOSE") throw new TypeError("Pattern has an unbalanced '}'");

      // `?` or `+` with nothing before it is just a literal character.
      literal += token.value;
      i++;
    }
    flush();
    return parts;
  }

  // ---- Compiler ------------------------------------------------------------

  function toRegExp(parts, ignoreCase) {
    let source = "^";
    const names = [];
    for (const part of parts) {
      if (part.type === "literal") {
        source += escapeRegex(part.value);
        continue;
      }
      const prefix = escapeRegex(part.prefix);
      const suffix = escapeRegex(part.suffix);
      if (part.name === null) {
        // A text-only `{…}` group: the modifier applies to the literal text.
        source += `(?:${prefix}${suffix})${part.modifier}`;
        continue;
      }
      names.push(part.name);
      if (part.modifier === "+" || part.modifier === "*") {
        // The capture spans the whole repeated run, with the separator between
        // repetitions rather than around them.
        const optional = part.modifier === "*" ? "?" : "";
        source +=
          `(?:${prefix}((?:${part.pattern})(?:${suffix}${prefix}(?:${part.pattern}))*)${suffix})` +
          optional;
      } else {
        source += `(?:${prefix}(${part.pattern})${suffix})${part.modifier}`;
      }
    }
    source += "$";
    return { re: new RegExp(source, ignoreCase ? "iu" : "u"), names };
  }

  const cache = new Map();

  function compile(str, componentName, ignoreCase) {
    const key = `${componentName}|${ignoreCase}|${str}`;
    let compiled = cache.get(key);
    if (compiled === undefined) {
      const component = COMPONENTS[componentName];
      // "*" on its own is the everything-matches default for a component; it
      // still yields one indexed group, as the spec's wildcard does.
      compiled = toRegExp(parse(str, component), ignoreCase);
      cache.set(key, compiled);
    }
    return compiled;
  }

  // ---- Pattern input resolution -------------------------------------------

  const COMPONENT_NAMES = [
    "protocol",
    "username",
    "password",
    "hostname",
    "port",
    "pathname",
    "search",
    "hash",
  ];

  function resolvePattern(input, base) {
    const res = {};
    for (const name of COMPONENT_NAMES) res[name] = "*";

    if (typeof input === "object" && input !== null) {
      for (const name of COMPONENT_NAMES) {
        if (input[name] !== undefined) res[name] = String(input[name]);
      }
      if (base !== undefined) {
        const b = new URL(String(base));
        if (input.protocol === undefined) res.protocol = b.protocol.replace(":", "");
        if (input.hostname === undefined) res.hostname = b.hostname;
        if (input.port === undefined) res.port = b.port;
      }
      return res;
    }

    const str = String(input);
    const b = base !== undefined ? new URL(String(base)) : null;

    // Split off ?search and #hash first — they are the same wherever the rest
    // of the pattern came from.
    const splitTail = (s) => {
      const hashAt = s.indexOf("#");
      let hash;
      if (hashAt !== -1) {
        hash = s.slice(hashAt + 1);
        s = s.slice(0, hashAt);
      }
      const qAt = s.indexOf("?");
      let search;
      if (qAt !== -1) {
        search = s.slice(qAt + 1);
        s = s.slice(0, qAt);
      }
      return { head: s, search, hash };
    };

    const protoMatch = str.match(/^([a-zA-Z0-9+.*-]+):\/\//);
    if (protoMatch) {
      res.protocol = protoMatch[1];
      let rest = str.slice(protoMatch[0].length);
      const pathAt = rest.indexOf("/");
      let authority = pathAt === -1 ? rest : rest.slice(0, pathAt);
      const path = pathAt === -1 ? "" : rest.slice(pathAt);

      const atAt = authority.indexOf("@");
      if (atAt !== -1) {
        const credentials = authority.slice(0, atAt);
        authority = authority.slice(atAt + 1);
        const colon = credentials.indexOf(":");
        if (colon !== -1) {
          res.username = credentials.slice(0, colon);
          res.password = credentials.slice(colon + 1);
        } else {
          res.username = credentials;
        }
      }
      // A port is only the part after the *last* colon, and never inside an
      // IPv6 literal.
      const portColon = authority.lastIndexOf(":");
      if (portColon !== -1 && !authority.endsWith("]")) {
        res.hostname = authority.slice(0, portColon);
        res.port = authority.slice(portColon + 1);
      } else {
        res.hostname = authority;
      }

      const tail = splitTail(path);
      res.pathname = tail.head === "" ? "*" : tail.head;
      if (tail.search !== undefined) res.search = tail.search;
      if (tail.hash !== undefined) res.hash = tail.hash;
      return res;
    }

    if (b) {
      res.protocol = b.protocol.replace(":", "");
      res.hostname = b.hostname;
      res.port = b.port;
    }

    const tail = splitTail(str);
    if (tail.head.startsWith("/") || !b) {
      res.pathname = tail.head;
    } else {
      // Relative to the base's directory.
      const dir = b.pathname.slice(0, b.pathname.lastIndexOf("/") + 1);
      res.pathname = dir + tail.head;
    }
    if (tail.search !== undefined) res.search = tail.search;
    if (tail.hash !== undefined) res.hash = tail.hash;
    return res;
  }

  // The value each component of a real URL contributes to matching.
  function componentValues(url) {
    return {
      protocol: url.protocol.replace(":", ""),
      username: url.username,
      password: url.password,
      hostname: url.hostname,
      port: url.port,
      pathname: url.pathname,
      search: url.search.replace("?", ""),
      hash: url.hash.replace("#", ""),
    };
  }

  class URLPattern {
    #patterns = {};
    #compiled = {};
    #hasRegExpGroups = false;

    constructor(input = {}, baseURL, options) {
      // Both `new URLPattern(input, base, options)` and
      // `new URLPattern(input, options)` are valid.
      let base = baseURL;
      let opts = options;
      if (typeof baseURL === "object" && baseURL !== null) {
        opts = baseURL;
        base = undefined;
      }
      const ignoreCase = Boolean(opts && opts.ignoreCase);

      this.#patterns = resolvePattern(input, base);
      for (const name of COMPONENT_NAMES) {
        this.#compiled[name] = compile(this.#patterns[name], name, ignoreCase);
      }
      // True when any component uses a custom regex rather than only the
      // segment/wildcard shorthands.
      this.#hasRegExpGroups = COMPONENT_NAMES.some((name) =>
        /\((?!\?)/.test(this.#patterns[name]),
      );
    }

    get protocol() {
      return this.#patterns.protocol;
    }
    get username() {
      return this.#patterns.username;
    }
    get password() {
      return this.#patterns.password;
    }
    get hostname() {
      return this.#patterns.hostname;
    }
    get port() {
      return this.#patterns.port;
    }
    get pathname() {
      return this.#patterns.pathname;
    }
    get search() {
      return this.#patterns.search;
    }
    get hash() {
      return this.#patterns.hash;
    }
    get hasRegExpGroups() {
      return this.#hasRegExpGroups;
    }

    test(input, baseURL) {
      let url;
      try {
        url = new URL(input, baseURL);
      } catch {
        return false;
      }
      const values = componentValues(url);
      for (const name of COMPONENT_NAMES) {
        if (!this.#compiled[name].re.test(values[name])) return false;
      }
      return true;
    }

    exec(input, baseURL) {
      let url;
      try {
        url = new URL(input, baseURL);
      } catch {
        return null;
      }
      const values = componentValues(url);
      const result = { inputs: baseURL !== undefined ? [input, baseURL] : [input] };
      for (const name of COMPONENT_NAMES) {
        const compiled = this.#compiled[name];
        const match = compiled.re.exec(values[name]);
        if (match === null) return null;
        const groups = {};
        for (let i = 0; i < compiled.names.length; i++) {
          // An unmatched optional group is `undefined`, not "".
          groups[compiled.names[i]] = match[i + 1];
        }
        result[name] = { input: values[name], groups };
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
