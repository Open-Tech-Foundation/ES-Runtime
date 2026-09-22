// CSS colours: what a declaration keeps, and what a computed value says.
//
// Every rule here was taken from Chrome rather than from the specification's
// prose, and the surprise is how much of the work a *declaration* already does:
//
//   * A **name** survives the declaration and resolves when computed:
//     `style.color = "red"` reads `red`, `getComputedStyle(el).color` reads
//     `rgb(255, 0, 0)`.
//   * **Hex, `rgb()`, `hsl()` and `hwb()`** are already `rgb()`/`rgba()` in the
//     declaration, clamped and rounded — `rgb(300 0 0)` is `rgb(255, 0, 0)`,
//     `hsl(400 150% 50%)` wraps its hue to `rgb(255, 170, 0)`.
//   * **`lab()`, `lch()`, `oklab()`, `oklch()`** keep their space but lose their
//     sugar: a percentage lightness becomes a number and a hue angle loses its
//     unit, so `lab(50% 40 30)` is `lab(50 40 30)`.
//   * **`color()`** is left exactly as written.
//   * `transparent` computes to `rgba(0, 0, 0, 0)`, `currentcolor` to the
//     element's own computed `color`.
//   * **`color-mix()`** drops `in oklab` — the default space — from the
//     declaration, and resolves when computed. Only an sRGB mix resolves here;
//     see the note at `mix()`.
//
// The 148 colour names come from `@opentf/std`; everything below is CSS's own
// arithmetic, because the library throws on `hwb()` and `lab()` and converts
// `oklch()`, which would be the wrong answer rather than a better one.

// A number as CSS serializes one: no trailing zeros, no exponent.
function print(value, places = 6) {
  const rounded = Number(value.toFixed(places));
  return String(Object.is(rounded, -0) ? 0 : rounded);
}

const clamp = (value, low, high) => Math.min(high, Math.max(low, value));

// An alpha is printed as short as it can be while still naming the same 8-bit
// value: `#ff000080` is `0.5` rather than `0.502`, because 0.5 of 255 rounds
// back to 0x80 — while `#f008` needs all three places, 0.533, since 0.53 would
// round back to a different byte. That is the rule a browser serializes by.
function printAlpha(alpha) {
  const byte = Math.round(clamp(alpha, 0, 1) * 255);
  for (const places of [1, 2, 3]) {
    const short = Number(alpha.toFixed(places));
    if (Math.round(short * 255) === byte) return String(short);
  }
  return print(alpha, 3);
}

function serializeRgb({ red, green, blue, alpha }) {
  const parts = [red, green, blue].map((channel) => Math.round(clamp(channel, 0, 255)));
  return alpha >= 1
    ? `rgb(${parts.join(", ")})`
    : `rgba(${parts.join(", ")}, ${printAlpha(clamp(alpha, 0, 1))})`;
}

// The pieces inside a function, split on top-level commas and then whitespace,
// with an alpha after a slash. Modern and legacy syntax both arrive here.
function args(inner) {
  const commas = [];
  let depth = 0;
  let start = 0;
  for (let at = 0; at < inner.length; at += 1) {
    if (inner[at] === "(") depth += 1;
    else if (inner[at] === ")") depth -= 1;
    else if (inner[at] === "," && depth === 0) { commas.push(inner.slice(start, at)); start = at + 1; }
  }
  commas.push(inner.slice(start));
  const parts = commas.length > 1
    ? commas.map((part) => part.trim())
    : commas[0].trim().split(/\s+/).filter(Boolean);
  // A slash separates the alpha in the modern form, attached or spaced.
  const flat = [];
  for (const part of parts) {
    if (part === "/") { flat.push("/"); continue; }
    if (part.includes("/")) { flat.push(...part.split("/").map((piece) => piece.trim()).filter(Boolean).flatMap((piece, index) => (index ? ["/", piece] : [piece]))); continue; }
    flat.push(part);
  }
  const slash = flat.indexOf("/");
  return slash === -1
    ? { values: flat, alpha: null }
    : { values: flat.slice(0, slash), alpha: flat[slash + 1] ?? null };
}

// A number, a percentage of `full`, or `none` — the three shapes a colour
// component takes.
function component(text, full = 1) {
  if (text === undefined || text === null) return null;
  const value = String(text).trim().toLowerCase();
  if (value === "none") return 0;
  const percent = /^([+-]?(?:\d+\.?\d*|\.\d+))%$/.exec(value);
  if (percent) return (Number(percent[1]) / 100) * full;
  const number = /^[+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?$/i.exec(value);
  return number === null ? null : Number(value);
}

function alphaOf(text) {
  if (text === undefined || text === null) return 1;
  const value = component(text, 1);
  return value === null ? 1 : clamp(value, 0, 1);
}

// An angle in degrees, whatever unit it was written in.
function angle(text) {
  const value = String(text ?? "").trim().toLowerCase();
  const match = /^([+-]?(?:\d+\.?\d*|\.\d+))(deg|grad|rad|turn)?$/.exec(value);
  if (!match) return null;
  const number = Number(match[1]);
  switch (match[2]) {
    case "grad": return (number * 360) / 400;
    case "rad": return (number * 180) / Math.PI;
    case "turn": return number * 360;
    default: return number;
  }
}

function hueToRgb(hue, saturation, lightness) {
  const h = ((hue % 360) + 360) % 360;
  const s = clamp(saturation, 0, 1);
  const l = clamp(lightness, 0, 1);
  const chroma = (1 - Math.abs(2 * l - 1)) * s;
  const secondary = chroma * (1 - Math.abs(((h / 60) % 2) - 1));
  const match = l - chroma / 2;
  const [red, green, blue] = h < 60 ? [chroma, secondary, 0]
    : h < 120 ? [secondary, chroma, 0]
    : h < 180 ? [0, chroma, secondary]
    : h < 240 ? [0, secondary, chroma]
    : h < 300 ? [secondary, 0, chroma]
    : [chroma, 0, secondary];
  return [(red + match) * 255, (green + match) * 255, (blue + match) * 255];
}

// `hwb(H W B)`: the hue at full saturation, then whitened and blackened. When
// the two add to 1 or more the colour is the grey between them.
function hwbToRgb(hue, white, black) {
  const w = clamp(white, 0, 1);
  const b = clamp(black, 0, 1);
  if (w + b >= 1) {
    const grey = (w / (w + b)) * 255;
    return [grey, grey, grey];
  }
  return hueToRgb(hue, 1, 0.5).map((channel) => (channel / 255) * (1 - w - b) * 255 + w * 255);
}

function hexToRgb(text) {
  const hex = text.slice(1);
  if (![3, 4, 6, 8].includes(hex.length) || /[^0-9a-f]/i.test(hex)) return null;
  const wide = hex.length > 4;
  const size = wide ? 2 : 1;
  const channels = [];
  for (let at = 0; at < hex.length; at += size) {
    const piece = hex.slice(at, at + size);
    channels.push(Number.parseInt(wide ? piece : piece + piece, 16));
  }
  const [red, green, blue, alpha] = channels;
  return { red, green, blue, alpha: alpha === undefined ? 1 : alpha / 255 };
}

export function createColors(convert) {
  // The properties whose value is a colour. Only these are resolved, so a colour
  // name appearing in `font-family` or `content` is left alone.
  const COLOR_PROPERTIES = new Set([
    "accent-color", "background-color", "border-block-end-color", "border-block-start-color",
    "border-bottom-color", "border-inline-end-color", "border-inline-start-color", "border-left-color",
    "border-right-color", "border-top-color", "caret-color", "color", "column-rule-color", "fill",
    "flood-color", "lighting-color", "outline-color", "stop-color", "stroke", "text-decoration-color",
    "text-emphasis-color",
  ]);

  const KEYWORDS = new Set(["currentcolor", "transparent", "inherit", "initial", "unset", "revert",
    "revert-layer", "none", "auto"]);
  const TRANSPARENT = "rgba(0, 0, 0, 0)";

  // A name is whatever the bundled table knows, which is the 148 CSS ones.
  function nameToRgb(name) {
    try {
      const parts = /^rgba?\(\s*([\d.]+)\D+([\d.]+)\D+([\d.]+)(?:\D+([\d.]+))?/.exec(convert({ value: name, to: "rgba" }));
      if (!parts) return null;
      return {
        red: Number(parts[1]), green: Number(parts[2]), blue: Number(parts[3]),
        alpha: parts[4] === undefined ? 1 : Number(parts[4]),
      };
    } catch {
      return null;
    }
  }

  // The declaration form of a colour, or null for something this DOM leaves
  // alone.
  function canonical(text) {
    const value = text.trim();
    const lower = value.toLowerCase();
    if (lower.startsWith("#")) {
      const rgb = hexToRgb(lower);
      return rgb === null ? null : serializeRgb(rgb);
    }
    const call = /^([a-z-]+)\(([\s\S]*)\)$/i.exec(value);
    if (call === null) {
      // A bare word: a name if the table knows it, and nothing otherwise.
      return /^[a-z]+$/.test(lower) && nameToRgb(lower) !== null ? lower : null;
    }
    const name = call[1].toLowerCase();
    const { values, alpha } = args(call[2]);
    switch (name) {
      case "rgb": case "rgba": {
        const channels = values.map((part) => component(part, 255));
        if (channels.length !== 3 || channels.some((channel) => channel === null)) return null;
        return serializeRgb({ red: channels[0], green: channels[1], blue: channels[2], alpha: alphaOf(alpha) });
      }
      case "hsl": case "hsla": {
        const hue = angle(values[0]);
        const saturation = component(values[1], 1);
        const lightness = component(values[2], 1);
        if (hue === null || saturation === null || lightness === null) return null;
        const [red, green, blue] = hueToRgb(hue, saturation, lightness);
        return serializeRgb({ red, green, blue, alpha: alphaOf(alpha) });
      }
      case "hwb": {
        const hue = angle(values[0]);
        const white = component(values[1], 1);
        const black = component(values[2], 1);
        if (hue === null || white === null || black === null) return null;
        const [red, green, blue] = hwbToRgb(hue, white, black);
        return serializeRgb({ red, green, blue, alpha: alphaOf(alpha) });
      }
      // The four that keep their space and lose their sugar: a percentage
      // lightness becomes a number — of 100 for `lab`/`lch`, of 1 for the `ok`
      // pair — and a hue angle loses its unit.
      case "lab": case "oklab": case "lch": case "oklch": {
        const full = name.startsWith("ok") ? 1 : 100;
        const lightness = component(values[0], full);
        const second = component(values[1], name.startsWith("ok") ? 0.4 : 150);
        const third = name.endsWith("ch") ? angle(values[2]) : component(values[2], name.startsWith("ok") ? 0.4 : 150);
        if (lightness === null || second === null || third === null) return null;
        const printed = [print(lightness), print(second), print(third)].join(" ");
        const opacity = alphaOf(alpha);
        return `${name}(${printed}${opacity >= 1 ? "" : ` / ${printAlpha(opacity)}`})`;
      }
      case "color-mix": {
        // `in oklab` is the default, and a browser does not keep what it can
        // assume.
        const parts = call[2].split(",");
        if (/^\s*in\s+oklab\s*$/i.test(parts[0])) return `color-mix(${parts.slice(1).map((part) => part.trim()).join(", ")})`;
        return value;
      }
      default: return null;
    }
  }

  /// The value a declaration holds for a colour property.
  function specifiedColor(value) {
    const text = String(value).trim();
    if (text === "") return value;
    if (KEYWORDS.has(text.toLowerCase())) return text.toLowerCase();
    return canonical(text) ?? value;
  }

  /// The value a computed style reports: as above, and then the keywords and the
  /// name that only a computed value resolves.
  function computedColor(value, currentColor) {
    const text = String(value).trim();
    const lower = text.toLowerCase();
    if (lower === "transparent") return TRANSPARENT;
    if (lower === "currentcolor") return currentColor;
    if (KEYWORDS.has(lower)) return text;
    if (/^[a-z]+$/.test(lower)) {
      const rgb = nameToRgb(lower);
      return rgb === null ? text : serializeRgb(rgb);
    }
    const mixed = mix(text);
    if (mixed !== null) return mixed;
    return canonical(text) ?? text;
  }

  // `color-mix()`, resolved. An sRGB mix is plain interpolation between the two
  // colours' channels, which is what a browser reports as `color(srgb …)`.
  //
  // A mix in another space is left as written: matching Chrome's
  // `oklab(0.539974 …)` means reproducing its conversion to the sixth decimal,
  // and a number that is close is worse than a value that says it was not
  // resolved. Recorded as a difference rather than approximated.
  function mix(text) {
    const call = /^color-mix\(([\s\S]*)\)$/i.exec(text.trim());
    if (call === null) return null;
    const parts = [];
    let depth = 0;
    let start = 0;
    for (let at = 0; at < call[1].length; at += 1) {
      if (call[1][at] === "(") depth += 1;
      else if (call[1][at] === ")") depth -= 1;
      else if (call[1][at] === "," && depth === 0) { parts.push(call[1].slice(start, at).trim()); start = at + 1; }
    }
    parts.push(call[1].slice(start).trim());
    const space = /^in\s+(\S+)$/i.exec(parts[0]);
    if (space === null || space[1].toLowerCase() !== "srgb" || parts.length !== 3) return null;
    const read = (part) => {
      const percent = /\s([+-]?[\d.]+)%$/.exec(part);
      const colour = percent === null ? part : part.slice(0, percent.index).trim();
      const serialized = computedColor(colour, "rgb(0, 0, 0)");
      const channels = /^rgba?\((\d+), (\d+), (\d+)(?:, ([\d.]+))?\)$/.exec(serialized);
      if (channels === null) return null;
      return {
        weight: percent === null ? null : Number(percent[1]) / 100,
        rgb: [Number(channels[1]) / 255, Number(channels[2]) / 255, Number(channels[3]) / 255],
        alpha: channels[4] === undefined ? 1 : Number(channels[4]),
      };
    };
    const first = read(parts[1]);
    const second = read(parts[2]);
    if (first === null || second === null) return null;
    let left = first.weight;
    let right = second.weight;
    if (left === null && right === null) { left = 0.5; right = 0.5; }
    else if (left === null) left = 1 - right;
    else if (right === null) right = 1 - left;
    const total = left + right;
    if (!(total > 0)) return null;
    const mixed = first.rgb.map((channel, at) => (channel * left + second.rgb[at] * right) / total);
    const alpha = (first.alpha * left + second.alpha * right) / total;
    const printed = mixed.map((channel) => print(clamp(channel, 0, 1))).join(" ");
    return alpha >= 1 ? `color(srgb ${printed})` : `color(srgb ${printed} / ${printAlpha(alpha)})`;
  }

  return { COLOR_PROPERTIES, specifiedColor, computedColor };
}
