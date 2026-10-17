// Colored brand icons for the landing page Benchmarks rows (the hero roller
// and the FrameworkTabs charts), each on a small white chip so dark artwork
// stays readable in both themes.
//
// Sources, all vendored under public/img/brands/ so the site makes no
// external requests: Node.js and Deno from devicons/devicon (MIT),
// Vite's light-background mark from tandpfun/skill-icons (MIT), esbuild's
// own favicon, the AWS smile for LLRT (Simple Icons path, CC0, tinted AWS
// orange), Bun from the artwork already in public/bun.svg, and ours
// (esrun, esdev) from public/img/otf-logo.svg. oj has no official artwork,
// so it keeps an original monochrome tile.
//
// Compiler quirks to respect (see components/StatusIcon.jsx): branch on
// `name` via JSX conditionals, NOT body `if`s — a body read leaves `name` as
// the raw signal, so the lookup misses and no icon renders.
//
// Pass `size` to change the box (default `size-4`); the standings table uses
// `size-6` so its single-column logos read at a glance.
const CHIP =
  "shrink-0 rounded-[4px] bg-white object-contain p-px ring-1 ring-zinc-900/10 dark:ring-white/25";

export default function RuntimeIcon({ name, size = "size-4" }) {
  const chip = size + " " + CHIP;
  return (
    <>
      {(name === "esrun" || name === "esdev") && (
        <img src="/img/otf-logo.svg" alt="" aria-hidden="true" draggable="false" className={chip} />
      )}
      {name === "bun" && (
        <img src="/bun.svg" alt="" aria-hidden="true" draggable="false" className={chip} />
      )}
      {name === "deno" && (
        <img src="/img/brands/deno.svg" alt="" aria-hidden="true" draggable="false" className={chip} />
      )}
      {name === "node" && (
        <img src="/img/brands/node.svg" alt="" aria-hidden="true" draggable="false" className={chip} />
      )}
      {name === "vite" && (
        <img src="/img/brands/vite.svg" alt="" aria-hidden="true" draggable="false" className={chip} />
      )}
      {name === "esbuild" && (
        <img src="/img/brands/esbuild.svg" alt="" aria-hidden="true" draggable="false" className={chip} />
      )}
      {name === "llrt" && (
        <img src="/img/brands/aws.svg" alt="" aria-hidden="true" draggable="false" className={chip} />
      )}
      {name === "oj" && (
        <svg viewBox="0 0 24 24" fill="none" aria-hidden="true" className={size + " shrink-0 opacity-80"}>
          <rect x="1.5" y="1.5" width="21" height="21" rx="5" fill="currentColor" opacity="0.16" />
          <rect x="1.5" y="1.5" width="21" height="21" rx="5" stroke="currentColor" stroke-width="1.6" />
          <circle cx="9" cy="12" r="3.2" stroke="currentColor" stroke-width="1.8" />
          <path d="M14.5 7.5v9" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
        </svg>
      )}
    </>
  );
}
