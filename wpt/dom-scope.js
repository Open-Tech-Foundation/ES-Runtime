// WPT files that are not tests of esdev's strict, layout-free DOM. `source` is
// the page and every script it loads, for what a path alone does not say.
export function excluded(path, source = "") {
  if (path.includes(".sub.")) return "requires WPT server substitution";
  if (path.includes("idlharness")) return "requires complete Web IDL exposure";
  if (path.includes("/observable/")) return "Observable remains a tentative API";
  if (path.includes("iframe")) return "requires nested browsing contexts or multiple realms";
  // Not named for it, but its first line is
  // `createElement("iframe").contentWindow`: every case in the file is about
  // `window.event` across two globals.
  if (path.endsWith("dom/events/event-global-extra.window.js")) {
    return "requires nested browsing contexts or multiple realms";
  }
  if (path.includes("legacy-")) return "legacy behavior is outside the modern-only scope";
  // `createEvent` exists for the modern interface names; the HTML4 aliases are
  // refused by name, and the upstream file tests all of them together.
  if (path.includes("Document-createEvent")) return "the HTML4 event-interface aliases are outside the modern-only scope";
  if (path.includes("xpath")) return "XPath is legacy, superseded by the selector APIs";
  // Geometry: a layout-free DOM has no boxes to hit-test or measure.
  if (/elementFromPoint|highlightsFromPoint|offsetParent-across|offsetTop-offsetLeft|offsetX-offsetY/.test(path)) {
    return "requires layout: there are no boxes to measure or hit-test";
  }
  if (/currentScript|insertion-removing-steps\/.*script/i.test(path)) return "script execution is not part of the test DOM";
  if (path.includes("tentative")) return "a tentative API, not yet in the specifications";
  // What a page reaches for that a layout-free, single-document DOM has none of.
  if (/\/resources\/testdriver\.js/.test(source)) return "requires WebDriver automation (testdriver.js)";
  if (/<iframe\b|createElement\(\s*["'`]iframe["'`]|\.contentWindow\b|\.contentDocument\b|window\.open\(|create_window_in_test/.test(source)) {
    return "requires nested browsing contexts or multiple realms";
  }
  if (/getAnimations\(|\.animate\(|transitionend|animationend|document\.timeline/.test(source)) return "requires CSS animations or transitions, which need rendering";
  if (/scrollTo\(|scrollBy\(|scrollIntoView|scrollTop\s*=|onscroll|"scroll"|'scroll'/.test(source)) return "requires scrolling, which needs layout";
  return null;
}
