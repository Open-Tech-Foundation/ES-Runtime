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
  let parentheses = 0;
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
    } else if (char === "(") parentheses += 1;
    else if (char === ")") {
      if (parentheses === 0) syntax(source, at, "unexpected )");
      parentheses -= 1;
    } else if (char === "," && brackets === 0 && parentheses === 0) {
      const part = source.slice(start, at).trim();
      if (!part) syntax(source, at, "empty selector in list");
      out.push(part);
      start = at + 1;
    }
  }
  if (quote) syntax(source, source.length, "unterminated string");
  if (brackets) syntax(source, source.length, "unterminated attribute selector");
  if (parentheses) syntax(source, source.length, "unterminated pseudo-class");
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

function parseNth(source, at, argument) {
  const text = argument.replace(/\s+/g, "").toLowerCase();
  if (text === "odd") return { a: 2, b: 1 };
  if (text === "even") return { a: 2, b: 0 };
  if (/^[+-]?\d+$/.test(text)) return { a: 0, b: Number(text) };
  const match = /^([+-]?\d*)n([+-]\d+)?$/.exec(text);
  if (!match) syntax(source, at, "malformed nth expression");
  const coefficient = match[1] === "" || match[1] === "+" ? 1 : match[1] === "-" ? -1 : Number(match[1]);
  return { a: coefficient, b: Number(match[2] ?? 0) };
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
    if (kind === ":") {
      const match = /^[A-Za-z-]+/.exec(text.slice(at + 1));
      const functional = new Set(["is", "where", "not", "has", "nth-child", "nth-last-child", "nth-of-type", "nth-last-of-type"]);
      const bare = new Set(["root", "empty", "first-child", "last-child", "only-child", "first-of-type", "last-of-type", "only-of-type", "focus", "scope", "checked", "disabled", "enabled", "required", "optional", "link"]);
      if (!match || !functional.has(match[0]) && !bare.has(match[0])) syntax(source, offset + at, "unsupported pseudo-class");
      const name = match[0];
      const open = at + match[0].length + 1;
      if (bare.has(name)) {
        if (text[open] === "(") syntax(source, offset + at, `:${name} does not take arguments`);
        simples.push({ type: name });
        at = open;
        continue;
      }
      if (text[open] !== "(") syntax(source, offset + at, `:${name} requires an argument list`);
      let end = open + 1;
      let depth = 1;
      let quote = null;
      let brackets = 0;
      for (; end < text.length; end += 1) {
        const char = text[end];
        if (quote) { if (char === quote) quote = null; continue; }
        if (char === "'" || char === '"') { quote = char; continue; }
        if (char === "[") { brackets += 1; continue; }
        if (char === "]") { brackets -= 1; continue; }
        if (brackets) continue;
        if (char === "(") depth += 1;
        if (char === ")" && --depth === 0) break;
      }
      if (end === text.length || depth !== 0) syntax(source, offset + at, "unterminated pseudo-class");
      const argument = text.slice(open + 1, end).trim();
      if (!argument) syntax(source, offset + at, `:${name} requires a non-empty argument list`);
      simples.push(name.startsWith("nth-")
        ? { type: name, nth: parseNth(source, offset + at, argument) }
        : { type: name, selectors: name === "has" ? parseRelativeList(argument) : splitList(argument).map(parseOne) });
      at = end + 1;
      continue;
    }
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
    let parentheses = 0;
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
      } else if (char === "(") {
        parentheses += 1;
        at += 1;
      } else if (char === ")") {
        parentheses -= 1;
        at += 1;
      } else if (brackets === 0 && parentheses === 0 && (/\s/.test(char) || ">+~".includes(char))) break;
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

function parseRelativeList(source) {
  return splitList(source).map((part) => {
    const relation = ">+~".includes(part[0]) ? part[0] : " ";
    const selector = relation === " " ? part : part.slice(1).trim();
    if (!selector) syntax(source, 0, "relative selector has no right selector");
    return { relation, parts: parseOne(selector) };
  });
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

function elementSiblings(element) {
  return Array.from(element.parentElement?._esdevChildren() ?? []).filter((node) => node.nodeType === 1);
}

function nthMatches(position, { a, b }) {
  if (a === 0) return position === b;
  const quotient = (position - b) / a;
  return Number.isInteger(quotient) && quotient >= 0;
}

export function createSelectors({ Element, Document, DocumentFragment }) {
  function matchesCompound(element, simples, scope) {
    return simples.every((simple) => {
      if (simple.type === "universal") return true;
      if (simple.type === "tag") return element.localName === simple.name;
      if (simple.type === "id") return element.id === simple.name;
      if (simple.type === "class") return (element.className || "").split(/\s+/).includes(simple.name);
      if (simple.type === "is" || simple.type === "where") return simple.selectors.some((parts) => matchesParts(element, parts, parts.length - 1, scope));
      if (simple.type === "not") return !simple.selectors.some((parts) => matchesParts(element, parts, parts.length - 1, scope));
      if (simple.type === "has") return simple.selectors.some((relative) => matchesRelative(element, relative, scope));
      if (simple.type === "root") return element === element.ownerDocument.documentElement;
      if (simple.type === "empty") return element.firstChild === null;
      if (simple.type === "focus") return element === element.ownerDocument.activeElement;
      if (simple.type === "scope") return element === scope;
      if (simple.type === "checked") return element.checked === true;
      if (simple.type === "disabled") return element.hasAttribute("disabled");
      if (simple.type === "enabled") return ["button", "input", "select", "textarea", "option", "optgroup", "fieldset"].includes(element.localName) && !element.hasAttribute("disabled");
      if (simple.type === "required") return element.hasAttribute("required");
      if (simple.type === "optional") return ["input", "select", "textarea"].includes(element.localName) && !element.hasAttribute("required");
      if (simple.type === "link") return ["a", "area"].includes(element.localName) && element.hasAttribute("href");
      if (simple.type.endsWith("child")) {
        const siblings = elementSiblings(element);
        const index = siblings.indexOf(element);
        if (simple.type === "first-child") return index === 0;
        if (simple.type === "last-child") return index === siblings.length - 1;
        if (simple.type === "only-child") return siblings.length === 1;
        const position = simple.type === "nth-last-child" ? siblings.length - index : index + 1;
        return nthMatches(position, simple.nth);
      }
      if (simple.type.endsWith("type")) {
        const siblings = elementSiblings(element).filter((sibling) => sibling.localName === element.localName);
        const index = siblings.indexOf(element);
        if (simple.type === "first-of-type") return index === 0;
        if (simple.type === "last-of-type") return index === siblings.length - 1;
        if (simple.type === "only-of-type") return siblings.length === 1;
        const position = simple.type === "nth-last-of-type" ? siblings.length - index : index + 1;
        return nthMatches(position, simple.nth);
      }
      return matchesAttribute(element, simple);
    });
  }

  function nextElement(element) {
    for (let node = element.nextSibling; node; node = node.nextSibling) if (node.nodeType === 1) return node;
    return null;
  }

  function matchesRelative(element, relative, scope) {
    if (relative.relation === ">") {
      for (let child = element.firstChild; child; child = child.nextSibling) if (child instanceof Element && matchesParts(child, relative.parts, relative.parts.length - 1, scope)) return true;
      return false;
    }
    if (relative.relation === "+") {
      const sibling = nextElement(element);
      return sibling !== null && matchesParts(sibling, relative.parts, relative.parts.length - 1, scope);
    }
    if (relative.relation === "~") {
      for (let sibling = nextElement(element); sibling; sibling = nextElement(sibling)) if (matchesParts(sibling, relative.parts, relative.parts.length - 1, scope)) return true;
      return false;
    }
    return descendants(element).some((child) => matchesParts(child, relative.parts, relative.parts.length - 1, scope));
  }

  function matchesParts(element, parts, index = parts.length - 1, scope = element) {
    if (!matchesCompound(element, parts[index].simples, scope)) return false;
    if (index === 0) return true;
    const relation = parts[index].relation;
    if (relation === ">") return element.parentElement !== null && matchesParts(element.parentElement, parts, index - 1, scope);
    if (relation === "+") {
      const sibling = previousElement(element);
      return sibling !== null && matchesParts(sibling, parts, index - 1, scope);
    }
    if (relation === "~") {
      for (let sibling = previousElement(element); sibling; sibling = previousElement(sibling)) if (matchesParts(sibling, parts, index - 1, scope)) return true;
      return false;
    }
    for (let parent = element.parentElement; parent; parent = parent.parentElement) if (matchesParts(parent, parts, index - 1, scope)) return true;
    return false;
  }

  function compile(source) {
    source = String(source);
    return splitList(source).map(parseOne);
  }

  function matches(element, source) {
    if (!(element instanceof Element)) return false;
    return compile(source).some((parts) => matchesParts(element, parts, parts.length - 1, element));
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

  function usesScope(parts) {
    return parts.some(({ simples }) => simples.some((simple) => simple.type === "scope" || simple.selectors?.some((nested) => usesScope(nested))));
  }

  function queryAll(root, source) {
    const compiled = compile(source);
    const candidates = root instanceof Element && compiled.some(usesScope) ? [root, ...descendants(root)] : descendants(root);
    const scope = root instanceof Element ? root : null;
    return staticList(candidates.filter((element) => compiled.some((parts) => matchesParts(element, parts, parts.length - 1, scope))));
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
