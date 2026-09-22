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
    if (char === "\\") {
      at += escapeLength(source, at) - 1;
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

// `An+B` optionally followed by `of <selector-list>`. The nth expression is
// matched first so a selector containing the word `of` cannot be split wrongly.
const NTH_WITH_OF = /^\s*(odd|even|[+-]?\d+|[+-]?\d*n(?:\s*[+-]\s*\d+)?)\s*(?:of\s+([\s\S]+))?$/i;

function splitNth(source, at, argument) {
  const match = NTH_WITH_OF.exec(argument);
  if (!match) syntax(source, at, "malformed nth expression");
  return { nth: match[1], of: match[2] ?? null };
}

// Pseudo-classes that need a pointer, a session history, an input modality or a
// rendered page. They are parsed and never match.
const NEVER_MATCH = new Set([
  "hover", "active", "visited", "focus-visible", "autofill", "user-valid", "user-invalid",
  "fullscreen", "picture-in-picture",
]);

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

// A CSS identifier, with its escapes resolved: `#id\\.with\\.dots` is one id
// containing dots, and `\\2c ` is a comma. Non-ASCII characters are name
// characters too, so `.café` is a class.
// How many characters the escape at `at` occupies: a hex escape is up to six
// digits and an optional trailing space, anything else is the backslash and one
// character. Every scanner below steps by this, or a `\\31 ` would be cut in two
// at the space that terminates it.
const HEX_ESCAPE = /^([0-9a-fA-F]{1,6})[ \t\n\f\r]?/;

function escapeLength(text, at) {
  const hex = HEX_ESCAPE.exec(text.slice(at + 1));
  if (hex) return 1 + hex[0].length;
  const next = text.codePointAt(at + 1);
  return next === undefined ? 1 : 1 + String.fromCodePoint(next).length;
}

function readName(text, at) {
  let value = "";
  let index = at;
  while (index < text.length) {
    const character = text[index];
    if (character === "\\") {
      const width = escapeLength(text, index);
      const hex = HEX_ESCAPE.exec(text.slice(index + 1));
      if (hex) {
        const code = Number.parseInt(hex[1], 16);
        // A null escape is a replacement character, as the tokenizer says.
        value += code === 0 || code > 0x10ffff ? "\ufffd" : String.fromCodePoint(code);
      } else {
        const escaped = text.slice(index + 1, index + width);
        if (escaped === "" || escaped === "\n") break;
        value += escaped;
      }
      index += width;
      continue;
    }
    const start = value === "";
    const ordinary = character.codePointAt(0) > 0x7f
      || (start ? /[A-Za-z_-]/.test(character) : /[A-Za-z0-9_-]/.test(character));
    if (!ordinary) break;
    value += character;
    index += 1;
  }
  return value === "" ? null : { value, length: index - at };
}

function parseCompound(source, offset, text) {
  const simples = [];
  let at = 0;
  if (text[at] === "*") {
    simples.push({ type: "universal" });
    at += 1;
  } else {
    const match = readName(text, at);
    if (match) {
      simples.push({ type: "tag", name: match.value });
      at += match.length;
    }
  }
  while (at < text.length) {
    const kind = text[at];
    if (kind === "#" || kind === ".") {
      const match = readName(text, at + 1);
      if (!match) syntax(source, offset + at, `expected a name after ${kind}`);
      simples.push({ type: kind === "#" ? "id" : "class", name: match.value });
      at += match.length + 1;
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
      const functional = new Set(["is", "where", "not", "has", "state", "nth-child", "nth-last-child", "nth-of-type", "nth-last-of-type"]);
      const bare = new Set([
        "root", "empty", "first-child", "last-child", "only-child", "first-of-type", "last-of-type", "only-of-type",
        "focus", "scope", "defined", "checked", "disabled", "enabled", "required", "optional", "link",
        // Computed from the tree or from control state.
        "any-link", "target", "focus-within", "valid", "invalid", "indeterminate", "placeholder-shown",
        "read-only", "read-write", "default", "open", "modal", "popover-open",
        // Real selectors with no answer in a DOM with no pointer, no history
        // and no rendering. They parse — a stylesheet is full of them — and
        // they match nothing, which is the same answer a browser gives when
        // nothing is being hovered.
        ...NEVER_MATCH,
      ]);
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
      if (name.startsWith("nth-")) {
        // Only the child forms take `of S`; `nth-of-type` already selects by type.
        const takesOf = name === "nth-child" || name === "nth-last-child";
        const { nth, of } = takesOf ? splitNth(source, offset + at, argument) : { nth: argument, of: null };
        simples.push({
          type: name,
          nth: parseNth(source, offset + at, nth),
          selectors: of === null ? null : splitList(of).map(parseOne),
        });
      } else if (name === "state") {
        // `:state(foo)` takes one identifier, not a selector list.
        if (!/^[A-Za-z_-][\w-]*$/.test(argument)) syntax(source, offset + at, ":state() takes a custom state name");
        simples.push({ type: "state", name: argument });
      } else {
        simples.push({ type: name, selectors: name === "has" ? parseRelativeList(argument) : splitList(argument).map(parseOne) });
      }
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
      } else if (char === "\\") {
        // An escape and what it escapes are one unit: `#a\\>b` is one id, and a
        // hex escape's terminating space is part of the escape rather than a
        // descendant combinator.
        at += escapeLength(source, at);
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

// Web IDL puts an interface's members on the prototype as **configurable**, and
// a test relies on it: `vi.spyOn(input, "checked", "set")` and every other stub
// redefines the property it is replacing, and a descriptor that forgot the flag
// answers `Cannot redefine property`. Enumerable too, as a browser has them.
// A symbol-keyed slot is this DOM's own bookkeeping and stays hidden and fixed.
function defineIdl(target, properties) {
  const described = {};
  for (const name of Reflect.ownKeys(properties)) {
    const descriptor = properties[name];
    if (typeof name === "symbol") {
      described[name] = descriptor;
      continue;
    }
    // An operation is writable as well as configurable — Web IDL says so, and a
    // test that replaces a method (`el.focus = spy`) needs it.
    const writable = typeof descriptor.value === "function" ? { writable: true } : {};
    described[name] = { configurable: true, enumerable: true, ...writable, ...descriptor };
  }
  Object.defineProperties(target, described);
  return target;
}

export function createSelectors({ Element, Document, DocumentFragment, ShadowRoot, HTML_NAMESPACE, isDefined = () => true, customStates = () => null, controlValidity = (element) => element.validity ?? null }) {
  function matchesCompound(element, simples, scope) {
    return simples.every((simple) => {
      if (simple.type === "universal") return true;
      if (simple.type === "tag") return element.localName === (element.namespaceURI === HTML_NAMESPACE ? simple.name.toLowerCase() : simple.name);
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
      if (NEVER_MATCH.has(simple.type)) return false;
      if (simple.type === "defined") return isDefined(element);
      if (simple.type === "any-link") return ["a", "area"].includes(element.localName) && element.hasAttribute("href");
      if (simple.type === "target") {
        const hash = element.ownerDocument?.defaultView?.location?.hash ?? "";
        return hash.length > 1 && element.id === hash.slice(1);
      }
      if (simple.type === "focus-within") {
        const active = element.ownerDocument.activeElement;
        return active === element || (active !== null && element.contains(active));
      }
      if (simple.type === "valid" || simple.type === "invalid") {
        const validity = controlValidity(element);
        if (!validity) return false;
        return simple.type === "valid" ? validity.valid : !validity.valid;
      }
      if (simple.type === "indeterminate") return element.indeterminate === true;
      if (simple.type === "placeholder-shown") {
        return element.hasAttribute?.("placeholder") && (element.value ?? "") === "";
      }
      if (simple.type === "read-only") return !isEditable(element);
      if (simple.type === "read-write") return isEditable(element);
      if (simple.type === "default") {
        if (element.localName === "option") return element.defaultSelected === true;
        if (["input", "button"].includes(element.localName)) {
          return ["submit", "image"].includes(element.type) || element.defaultChecked === true;
        }
        return false;
      }
      if (simple.type === "open") return element.hasAttribute("open");
      // State rather than rendering: a modal dialog and an open popover are
      // knowable without a top layer to put them in.
      if (simple.type === "modal") return element._esdevIsModal?.() === true;
      if (simple.type === "popover-open") return element._esdevPopoverOpen?.() === true;
      if (simple.type === "state") return customStates(element)?.has(simple.name) === true;
      if (simple.type.endsWith("child")) {
        const all = elementSiblings(element);
        if (simple.type === "first-child") return all.indexOf(element) === 0;
        if (simple.type === "last-child") return all.indexOf(element) === all.length - 1;
        if (simple.type === "only-child") return all.length === 1;
        // With `of S` only the matching siblings are counted, and the element
        // itself has to be one of them.
        const siblings = simple.selectors === null
          ? all
          : all.filter((sibling) => simple.selectors.some((parts) => matchesParts(sibling, parts, parts.length - 1, scope)));
        const index = siblings.indexOf(element);
        if (index < 0) return false;
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

  // Editable means a control the user could type into, or explicit
  // contenteditable. Everything else is read-only, as in a browser.
  function isEditable(element) {
    if (element.isContentEditable === true) return true;
    if (!["input", "textarea"].includes(element.localName)) return false;
    return !element.hasAttribute("readonly") && !element.disabled;
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

  // Specificity as `[ids, classes, types]`, for the cascade to sort by. The
  // logical pseudo-classes take the specificity of their most specific
  // argument and add nothing of their own, except `:where()`, which is zero,
  // and `:nth-child(… of S)`, which is a pseudo-class plus the list's.
  function specificityOfSimple(simple) {
    if (simple.type === "id") return [1, 0, 0];
    if (simple.type === "class" || simple.type === "attribute") return [0, 1, 0];
    if (simple.type === "tag") return [0, 0, 1];
    if (simple.type === "universal") return [0, 0, 0];
    if (simple.type === "where") return [0, 0, 0];
    if (simple.type === "is" || simple.type === "not") return mostSpecific(simple.selectors);
    if (simple.type === "has") return mostSpecific(simple.selectors.map((relative) => relative.parts));
    if (simple.selectors) return add([0, 1, 0], mostSpecific(simple.selectors));
    // Every other pseudo-class counts as one class.
    return [0, 1, 0];
  }

  function add(left, right) {
    return [left[0] + right[0], left[1] + right[1], left[2] + right[2]];
  }

  function compare(left, right) {
    for (let index = 0; index < 3; index += 1) {
      if (left[index] !== right[index]) return left[index] - right[index];
    }
    return 0;
  }

  function specificityOfParts(parts) {
    return parts.reduce(
      (total, part) => part.simples.reduce((sum, simple) => add(sum, specificityOfSimple(simple)), total),
      [0, 0, 0],
    );
  }

  function mostSpecific(list) {
    return (list ?? []).reduce((best, parts) => (compare(specificityOfParts(parts), best) > 0 ? specificityOfParts(parts) : best), [0, 0, 0]);
  }

  // The specificity of a whole selector list is its most specific selector,
  // which is what a rule with a comma-separated selector cascades at.
  function specificity(source) {
    return mostSpecific(compile(source));
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
      forEach(callback, thisArg) {
        if (typeof callback !== "function") throw new TypeError("NodeList.forEach expects a function");
        values.forEach((value, index) => callback.call(thisArg, value, index, this));
      },
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
    for (const Class of [Element, Document, DocumentFragment, ShadowRoot]) {
      defineIdl(Class.prototype, {
        querySelector: { value(source) { return queryAll(this, source).item(0); } },
        querySelectorAll: { value(source) { return queryAll(this, source); } },
      });
    }
    defineIdl(Element.prototype, {
      matches: { value(source) { return matches(this, source); } },
      closest: { value(source) { for (let node = this; node; node = node.parentElement) if (matches(node, source)) return node; return null; } },
    });
  }

  return { install, compile, matches, queryAll, specificity, compareSpecificity: compare };
}
