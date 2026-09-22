// The one thing the DOM needs from @opentf/std: colour names and conversion.
// Re-exported rather than imported directly by `runtime:dom/colors` so the
// bundle has a single entry, and so the CSS rules live in hand-written code
// while the table and the arithmetic stay upstream.
export { color } from "@opentf/std";
