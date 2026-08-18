/**
 * The words the page is made of.
 *
 * Separate from `main.ts` because this touches no DOM, so `esdev test` can run
 * it — there is no DOM in the runtime.
 */

/** One entry in the row of links at the foot of the page. */
export type Link = {
  label: string;
  href: string;
};

export const LEDE =
  "Built with ES Runtime — a secure, standards-based JavaScript runtime from the Open Tech Foundation.";

export const LINKS: readonly Link[] = [
  { label: "Docs", href: "https://esrun.opentechf.org/docs" },
  { label: "API", href: "https://esrun.opentechf.org/api" },
  { label: "GitHub", href: "https://github.com/Open-Tech-Foundation/ES-Runtime" },
];

/** The line telling whoever scaffolded this where to start. */
export function editHint(file: string): string {
  return `Edit ${file} and save.`;
}
