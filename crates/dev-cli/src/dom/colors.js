// CSS colours: what a declaration keeps, and what a computed value says.
//
// The table of 148 names and the hex/rgb/hsl arithmetic come from
// `@opentf/std`'s `color()`, bundled beside this file. The rules are here,
// because they are CSS's rather than colour theory's, and every one of them was
// taken from Chrome rather than from the specification's prose:
//
//   * A **keyword** survives in a declaration and resolves when computed:
//     `style.color = "red"` reads `red`, `getComputedStyle(el).color` reads
//     `rgb(255, 0, 0)`.
//   * A **hex or legacy function** is canonical in the declaration already:
//     `#fff` and `hsl(0, 100%, 50%)` both read `rgb(255, 255, 255)` and
//     `rgb(255, 0, 0)` there, not only when computed.
//   * `transparent` computes to `rgba(0, 0, 0, 0)`, and `currentcolor` to the
//     element's own computed `color`.
//   * A **modern** colour — `oklch()`, `lab()`, `color()`, `color-mix()` — is
//     left exactly as written, in both. A browser keeps the colour space it was
//     given, so converting one would be the wrong answer rather than a better
//     one.
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

// Left as written, in the declaration and in the computed value alike.
const MODERN = /^(?:oklch|oklab|lab|lch|hwb|color|color-mix|light-dark|device-cmyk)\(/i;
const KEYWORDS = new Set(["currentcolor", "transparent", "inherit", "initial", "unset", "revert", "revert-layer", "none", "auto"]);

const TRANSPARENT = "rgba(0, 0, 0, 0)";

// CSS Color 4 writes `rgb(1 2 3 / 50%)`; the bundled converter reads the legacy
// comma form, so the space form is rewritten into it rather than reimplemented.
function legacyForm(value) {
  const call = /^(rgba?|hsla?)\(\s*([^)]*)\)$/i.exec(value.trim());
  if (!call) return value;
  const name = call[1].toLowerCase();
  if (call[2].includes(",")) return value;
  const [components, alpha] = call[2].split("/");
  const parts = components.trim().split(/\s+/);
  if (parts.length !== 3) return value;
  const together = alpha === undefined ? parts : [...parts, alpha.trim()];
  const percentAlpha = /^(-?[\d.]+)%$/.exec(together[3] ?? "");
  if (percentAlpha) together[3] = String(Number(percentAlpha[1]) / 100);
  const base = name.startsWith("rgb") ? "rgb" : "hsl";
  return `${together.length === 4 ? `${base}a` : base}(${together.join(", ")})`;
}

// `rgb(…)` while the colour is opaque, `rgba(…)` once it is not — the way a
// browser serializes one.
function serialize(rgba) {
  const parts = /^rgba?\(\s*([\d.]+)\D+([\d.]+)\D+([\d.]+)(?:\D+([\d.]+))?\s*\)$/.exec(rgba);
  if (!parts) return rgba;
  const [red, green, blue] = parts.slice(1, 4).map((part) => Math.round(Number(part)));
  const alpha = parts[4] === undefined ? 1 : Number(parts[4]);
  return alpha >= 1 ? `rgb(${red}, ${green}, ${blue})` : `rgba(${red}, ${green}, ${blue}, ${alpha})`;
}

function toRgb(value) {
  try {
    return serialize(convert({ value, to: "rgba" }));
  } catch {
    // Not a colour this DOM can read. Leaving it alone is the honest answer:
    // the declaration was kept, so the value is something CSS understands and
    // this does not.
    return null;
  }
}

/// The value a declaration holds for a colour property: a keyword stays a
/// keyword, a modern function stays itself, and everything else is canonical.
function specifiedColor(value) {
  const text = String(value).trim();
  if (text === "" || MODERN.test(text)) return value;
  const lower = text.toLowerCase();
  if (KEYWORDS.has(lower)) return lower;
  // A name stays a name until it is computed — `style.color = "RED"` reads
  // `red`, not `rgb(255, 0, 0)`. Only a hex or a function is canonical here, and
  // a bare word that is no colour at all is left for the value check to refuse.
  if (/^[a-z]+$/.test(lower)) return toRgb(lower) === null ? value : lower;
  return toRgb(legacyForm(text)) ?? value;
}

/// The value a computed style reports: as above, and then the two keywords that
/// only a computed value can answer.
function computedColor(value, currentColor) {
  const text = String(value).trim();
  const lower = text.toLowerCase();
  if (lower === "transparent") return TRANSPARENT;
  if (lower === "currentcolor") return currentColor;
  if (text === "" || MODERN.test(text) || KEYWORDS.has(lower)) return text;
  // Unlike a declaration, a computed value resolves the name as well.
  return toRgb(legacyForm(lower)) ?? text;
}

  return { COLOR_PROPERTIES, specifiedColor, computedColor };
}
