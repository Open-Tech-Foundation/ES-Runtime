// A deliberately strict subset of Selectors: unsupported syntax throws rather
// than turning into a selector that happens to match nothing.  It is kept
// realm-local beside the JS tree because matching must walk JS nodes.

function syntax(source, at, message) {
  throw new SyntaxError(`Invalid selector at ${at}: ${message} (${source})`);
}

function splitList(source) {
  const out = [];
  let start = 0;
  let quote = null;
  let brackets = 0;
  for (let at = 0; at < source.length; at += 1) {
    const char = source[at];
    if (quote) {
      if (char === quote) quote = null;
      continue;
    }
    if (char === "'" || char === '"') quote = char;
    else if (char === "[") brackets += 1;
    else if (char === "]") {
      if (brackets === 0) syntax(source, at, "unexpected ]");
      brackets -= 1;
    } else if (char === "," && brackets === 0) {
      const part = source.slice(start, at).trim();
      if (!part) syntax(source, at, "empty selector in list");
      out.push(part);
      start = at + 1;
    }
  }
  if (quote) syntax(source, source.length, "unterminated string");
  if (brackets) syntax(source, source.length, "unterminated attribute selector");
  const part = source.slice(start).trim();
  if (!part) syntax(source, source.length, "empty selector in list");
  out.push(part);
  return out;
}

function parseAttribute(source, at, body) {
  const match = /^([A-Za-z_][A-Za-z0-9_:-]*)(?:\s*(~=|\|=|\^=|\$=|\*=|=)\s*(?:(["'])(.*?)\3|([^\s\]]+))(?:\s+([iIsS]))?)?\s*$/s.exec(body);
  if (!match) syntax(source, at, "malformed attribute selector");
  const [, name, operator, , quoted, bare, flag] = match;
  return { type: "attribute", name, operator, value: quoted ?? bare, insensitive: flag?.toLowerCase() === "i" };
}

function parseCompound(source, offset, text) {
  const simples = [];
  let at = 0;
  const name = /^[A-Za-z_][A-Za-z0-9_-]*/;
  if (text[at] === "*") {
    simples.push({ type: "universal" });
    at += 1;
  } else {
    const match = name.exec(text.slice(at));
    if (match) {
      simples.push({ type: "tag", name: match[0].toLowerCase() });
      at += match[0].length;
    }
  }
  while (at < text.length) {
    const kind = text[at];
    if (kind === "#" || kind === ".") {
      const match = name.exec(text.slice(at + 1));
      if (!match) syntax(source, offset + at, `expected a name after ${kind}`);
      simples.push({ type: kind === "#" ? "id" : "class", name: match[0] });
      at += match[0].length + 1;
      continue;
    }
    if (kind === "[") {
      let end = at + 1;
      let quote = null;
      for (; end < text.length; end += 1) {
        if (quote) {
          if (text[end] === quote) quote = null;
        } else if (text[end] === "'" || text[end] === '"') quote = text[end];
        else if (text[end] === "]") break;
      }
      if (end === text.length) syntax(source, offset + at, "unterminated attribute selector");
      simples.push(parseAttribute(source, offset + at, text.slice(at + 1, end)));
      at = end + 1;
      continue;
    }
    if (kind === ":") syntax(source, offset + at, "pseudo-classes are not implemented yet");
    syntax(source, offset + at, `unexpected ${kind}`);
  }
  if (simples.length === 0) syntax(source, offset, "expected a simple selector");
  return simples;
}

function parseOne(source) {
  const parts = [];
  let at = 0;
  let relation = null;
  const whitespace = () => {
    const start = at;
    while (/\s/.test(source[at] ?? "")) at += 1;
    return at !== start;
  };
  whitespace();
  while (at < source.length) {
    if (">+~".includes(source[at])) syntax(source, at, "combinator has no left selector");
    const start = at;
    let brackets = 0;
    let quote = null;
    while (at < source.length) {
      const char = source[at];
      if (quote) {
        if (char === quote) quote = null;
        at += 1;
      } else if (char === "'" || char === '"') {
        quote = char;
        at += 1;
      } else if (char === "[") {
        brackets += 1;
        at += 1;
      } else if (char === "]") {
        brackets -= 1;
        at += 1;
      } else if (brackets === 0 && (/\s/.test(char) || ">+~".includes(char))) break;
      else at += 1;
    }
    parts.push({ simples: parseCompound(source, start, source.slice(start, at)), relation });
    const hadSpace = whitespace();
    if (at === source.length) break;
    if (">+~".includes(source[at])) {
      relation = source[at++];
      whitespace();
    } else if (hadSpace) {
      relation = " ";
    } else {
      syntax(source, at, "expected a combinator");
    }
    if (at === source.length) syntax(source, at, "combinator has no right selector");
  }
  return parts;
}

function previousElement(element) {
  for (let node = element.previousSibling; node; node = node.previousSibling) if (node.nodeType === 1) return node;
  return null;
}

function matchesAttribute(element, simple) {
  const actual = element.getAttribute(simple.name);
  if (actual === null) return false;
  if (!simple.operator) return true;
  const value = simple.insensitive ? simple.value.toLowerCase() : simple.value;
  const candidate = simple.insensitive ? actual.toLowerCase() : actual;
  switch (simple.operator) {
    case "=": return candidate === value;
    case "~=": return candidate.split(/\s+/).includes(value);
    case "|=": return candidate === value || candidate.startsWith(`${value}-`);
    case "^=": return candidate.startsWith(value);
    case "$=": return candidate.endsWith(value);
    case "*=": return candidate.includes(value);
    default: return false;
  }
}

export function createSelectors({ Element, Document, DocumentFragment }) {
  function matchesCompound(element, simples) {
    return simples.every((simple) => {
      if (simple.type === "universal") return true;
      if (simple.type === "tag") return element.localName === simple.name;
      if (simple.type === "id") return element.id === simple.name;
      if (simple.type === "class") return (element.className || "").split(/\s+/).includes(simple.name);
      return matchesAttribute(element, simple);
    });
  }

  function matchesParts(element, parts, index = parts.length - 1) {
    if (!matchesCompound(element, parts[index].simples)) return false;
    if (index === 0) return true;
    const relation = parts[index].relation;
    if (relation === ">") return element.parentElement !== null && matchesParts(element.parentElement, parts, index - 1);
    if (relation === "+") {
      const sibling = previousElement(element);
      return sibling !== null && matchesParts(sibling, parts, index - 1);
    }
    if (relation === "~") {
      for (let sibling = previousElement(element); sibling; sibling = previousElement(sibling)) if (matchesParts(sibling, parts, index - 1)) return true;
      return false;
    }
    for (let parent = element.parentElement; parent; parent = parent.parentElement) if (matchesParts(parent, parts, index - 1)) return true;
    return false;
  }

  function compile(source) {
    source = String(source);
    return splitList(source).map(parseOne);
  }

  function matches(element, source) {
    if (!(element instanceof Element)) return false;
    return compile(source).some((parts) => matchesParts(element, parts));
  }

  function descendants(root) {
    const found = [];
    for (let node = root.firstChild; node; node = node.nextSibling) {
      if (node instanceof Element) {
        found.push(node, ...descendants(node));
      }
    }
    return found;
  }

  function staticList(values) {
    return new Proxy({
      length: values.length,
      item(index) { return values[index] ?? null; },
      [Symbol.iterator]() { return values[Symbol.iterator](); },
    }, {
      get(target, property, receiver) {
        if (typeof property === "string" && /^(0|[1-9][0-9]*)$/.test(property)) return values[Number(property)];
        return Reflect.get(target, property, receiver);
      },
    });
  }

  function queryAll(root, source) {
    const compiled = compile(source);
    return staticList(descendants(root).filter((element) => compiled.some((parts) => matchesParts(element, parts))));
  }

  function install() {
    for (const Class of [Element, Document, DocumentFragment]) {
      Object.defineProperties(Class.prototype, {
        querySelector: { value(source) { return queryAll(this, source).item(0); } },
        querySelectorAll: { value(source) { return queryAll(this, source); } },
      });
    }
    Object.defineProperties(Element.prototype, {
      matches: { value(source) { return matches(this, source); } },
      closest: { value(source) { for (let node = this; node; node = node.parentElement) if (matches(node, source)) return node; return null; } },
    });
  }

  return { install, compile, matches, queryAll };
}
