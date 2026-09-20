// WPT files that are not tests of esdev's strict, layout-free DOM.
export function excluded(path) {
  if (path.includes(".sub.")) return "requires WPT server substitution";
  if (path.includes("idlharness")) return "requires complete Web IDL exposure";
  if (path.includes("/observable/")) return "Observable remains a tentative API";
  if (path.includes("iframe") || path.includes("cross-document")) return "requires nested browsing contexts or multiple realms";
  if (path.includes("legacy-")) return "legacy behavior is outside the modern-only scope";
  if (path.includes("Document-createEvent-")) return "legacy createEvent API is outside the modern-only scope";
  if (path.includes("insertion-removing-steps/script")) return "script execution is not part of the test DOM";
  return null;
}
