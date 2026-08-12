/**
 * Global Route Guard
 * 
 * Use this to protect routes, redirect users, or handle global navigation logic.
 */
// Pages that have moved. A docs URL that has been published is a promise: the
// page can move, the link cannot break. Keep an entry here for as long as the
// old address is likely to be in someone's bookmarks or another site's markup.
const MOVED = {
  // TypeScript setup joined the esdev section, where the tool that writes the
  // tsconfig lives (2026-08-12).
  "/docs/typescript": "/docs/esdev/typescript",
};

export default async function routeGuard(to, { next, redirect }) {
  const path = (to?.path ?? to?.pathname ?? String(to ?? "")).replace(/\/+$/, "");
  if (MOVED[path]) return redirect(MOVED[path]);

  next();
}
