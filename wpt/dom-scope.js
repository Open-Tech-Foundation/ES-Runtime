// WPT files that are not tests of esdev's strict, layout-free DOM.
export function excluded(path) {
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
  if (path.includes("Document-createEvent-")) return "the HTML4 event-interface aliases are outside the modern-only scope";
  if (path.includes("insertion-removing-steps/script")) return "script execution is not part of the test DOM";
  return null;
}
