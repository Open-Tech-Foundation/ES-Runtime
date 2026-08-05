// The runtimes, their display names, and the order every chart lists them in.
//
// Alphabetical, deliberately. Each of these lists used to be written out per
// component, and the copies drifted: eight of them opened with esrun — putting
// the subject of the comparison at the top of every chart whether it won that
// row or lost it — while RpsChart had been hand-sorted by req/s at some past
// moment and had since gone stale, so it showed Deno first when Bun was faster,
// and gave no ranking at all on the static-file chart it also draws.
//
// Alphabetical is the one order that asserts nothing. Which runtime won is the
// chart's job to show — the winning bar is drawn in emerald — and a fixed order
// keeps a runtime on the same line down a multi-row chart, so it can be followed
// from one metric to the next.
//
// Components still choose *which* runtimes they draw: LLRT has no HTTP server,
// so the req/s surfaces filter it out by asking whether their own data has a
// value for it, rather than by keeping a second hand-written list.
export const LABELS = {
  bun: "Bun",
  deno: "Deno",
  esrun: "esrun",
  llrt: "LLRT",
  node: "Node.js",
};

export const ORDER = ["bun", "deno", "esrun", "llrt", "node"];

/// The display name for a runtime key, falling back to the key itself.
export function label(rt) {
  return LABELS[rt] || rt;
}

/// The runtimes matching `keep`, in display order.
export function runtimes(keep) {
  return ORDER.filter(keep);
}
