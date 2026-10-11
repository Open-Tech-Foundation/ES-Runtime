// Technology tiles for the landing page "Why" cloud: official brand artwork
// with a label under each, no paragraphs.
//
// Sources, all vendored under public/img/brands/ so the site makes no
// external requests: SQLite, PostgreSQL, MySQL, Redis, Rust, TypeScript,
// YAML, JSON, V8 and XML from devicons/devicon (MIT); TOML, WebAssembly and
// JavaScript from Simple Icons (CC0). MessagePack, Protobuf, HTTP,
// WebSocket, images, durable state, the toolchain, the sandbox, mail,
// hashing, test running, watching, browser runs, plugins, HMR, OXC builds,
// CSS minifying and capabilities have no official artwork, so they keep
// original monochrome glyphs — the same deal as RuntimeIcon's `oj` tile.
//
// Compiler quirks to respect (see components/StatusIcon.jsx): branch on
// `name` via JSX conditionals, NOT a body lookup — a body read leaves
// `name` as the raw signal, which prerenders fine and wipes the icon on
// hydration. Glyph markup is written out in full in every branch: passing
// paths through a wrapper component's `children` hits the same signal
// boundary and goes blank on the dev server. Stroke attributes in kebab
// case.
const CHIP =
  "stack-icon size-14 shrink-0 rounded-2xl bg-white object-contain p-2 ring-1 ring-zinc-900/10 dark:ring-white/25";
const TILE =
  "stack-icon flex size-14 shrink-0 items-center justify-center rounded-2xl bg-white text-zinc-700 ring-1 ring-zinc-900/10 dark:bg-zinc-900 dark:text-zinc-100 dark:ring-white/25";

export default function StackIcon({ name }) {
  return (
    <>
      {name === "sqlite" && (
        <img src="/img/brands/sqlite.svg" alt="" aria-hidden="true" draggable="false" className={CHIP} />
      )}
      {name === "postgres" && (
        <img src="/img/brands/postgres.svg" alt="" aria-hidden="true" draggable="false" className={CHIP} />
      )}
      {name === "mysql" && (
        <img src="/img/brands/mysql.svg" alt="" aria-hidden="true" draggable="false" className={CHIP} />
      )}
      {name === "redis" && (
        <img src="/img/brands/redis.svg" alt="" aria-hidden="true" draggable="false" className={CHIP} />
      )}
      {name === "xml" && (
        <img src="/img/brands/xml.svg" alt="" aria-hidden="true" draggable="false" className={CHIP} />
      )}
      {name === "yaml" && (
        <img src="/img/brands/yaml.svg" alt="" aria-hidden="true" draggable="false" className={CHIP} />
      )}
      {name === "json" && (
        <img src="/img/brands/json.svg" alt="" aria-hidden="true" draggable="false" className={CHIP} />
      )}
      {name === "toml" && (
        <img src="/img/brands/toml.svg" alt="" aria-hidden="true" draggable="false" className={CHIP} />
      )}
      {name === "messagepack" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16Z" />
            <path d="m3.3 7 8.7 5 8.7-5" />
            <path d="M12 22V12" />
          </svg>
        </span>
      )}
      {name === "protobuf" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" />
            <path d="M14 2v4a2 2 0 0 0 2 2h4" />
            <path d="M10 9H8" />
            <path d="M16 13H8" />
            <path d="M16 17H8" />
          </svg>
        </span>
      )}
      {name === "wasm" && (
        <span className="stack-icon flex size-14 shrink-0 items-center justify-center rounded-2xl bg-white ring-1 ring-zinc-900/10 dark:ring-white/25">
          <svg width="28" height="28" viewBox="0 0 24 24" fill="#654ff0" aria-hidden="true">
            <path d="M14.745,0c0,0.042,0,0.085,0,0.129c0,1.52-1.232,2.752-2.752,2.752c-1.52,0-2.752-1.232-2.752-2.752 c0-0.045,0-0.087,0-0.129H0v24h24V0H14.745z M11.454,21.431l-1.169-5.783h-0.02l-1.264,5.783H7.39l-1.824-8.497h1.59l1.088,5.783 h0.02l1.311-5.783h1.487l1.177,5.854h0.02l1.242-5.854h1.561l-2.027,8.497H11.454z M20.209,21.431l-0.542-1.891h-2.861l-0.417,1.891 h-1.59l2.056-8.497h2.509l2.5,8.497H20.209z M17.812,15.028l-0.694,3.118h2.159l-0.796-3.118H17.812z" />
          </svg>
        </span>
      )}
      {name === "v8" && (
        <img src="/img/brands/v8.svg" alt="" aria-hidden="true" draggable="false" className={CHIP} />
      )}
      {name === "rust" && (
        <img src="/img/brands/rust.svg" alt="" aria-hidden="true" draggable="false" className={CHIP} />
      )}
      {name === "javascript" && (
        <img src="/img/brands/javascript.svg" alt="" aria-hidden="true" draggable="false" className={CHIP} />
      )}
      {name === "typescript" && (
        <img src="/img/brands/typescript.svg" alt="" aria-hidden="true" draggable="false" className={CHIP} />
      )}
      {name === "http" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <circle cx="12" cy="12" r="10" />
            <path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" />
            <path d="M2 12h20" />
          </svg>
        </span>
      )}
      {name === "websocket" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M22 12h-4l-3 9L9 3l-3 9H2" />
          </svg>
        </span>
      )}
      {name === "images" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <rect width="18" height="18" x="3" y="3" rx="2" ry="2" />
            <circle cx="9" cy="9" r="2" />
            <path d="m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21" />
          </svg>
        </span>
      )}
      {name === "durable" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <ellipse cx="12" cy="5" rx="9" ry="3" />
            <path d="M3 5V19A9 3 0 0 0 21 19V5" />
            <path d="M3 12A9 3 0 0 0 21 12" />
          </svg>
        </span>
      )}
      {name === "toolchain" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="m4 17 6-6-6-6" />
            <path d="M12 19h8" />
          </svg>
        </span>
      )}
      {name === "sandbox" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z" />
            <path d="m9 12 2 2 4-4" />
          </svg>
        </span>
      )}
      {name === "email" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <rect width="20" height="16" x="2" y="4" rx="2" />
            <path d="m22 7-8.97 5.7a1.94 1.94 0 0 1-2.06 0L2 7" />
          </svg>
        </span>
      )}
      {name === "hashing" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M4 9h16" />
            <path d="M4 15h16" />
            <path d="M10 3 8 21" />
            <path d="M16 3l-2 18" />
          </svg>
        </span>
      )}
      {name === "test" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M10 2v7.527a2 2 0 0 1-.211.896L4.72 20.55a1 1 0 0 0 .9 1.45h12.76a1 1 0 0 0 .9-1.45l-5.069-10.127A2 2 0 0 1 14 9.527V2" />
            <path d="M8.5 2h7" />
            <path d="M7 16h10" />
          </svg>
        </span>
      )}
      {name === "watch" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M2 12s3-7 10-7 10 7 10 7-3 7-10 7-10-7-10-7Z" />
            <circle cx="12" cy="12" r="3" />
          </svg>
        </span>
      )}
      {name === "browsers" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <rect width="20" height="14" x="2" y="3" rx="2" />
            <path d="M8 21h8" />
            <path d="M12 17v4" />
          </svg>
        </span>
      )}
      {name === "plugins" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M12 22v-5" />
            <path d="M9 8V2" />
            <path d="M15 8V2" />
            <path d="M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8Z" />
          </svg>
        </span>
      )}
      {name === "hmr" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" />
            <path d="M21 3v5h-5" />
            <path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" />
            <path d="M8 16H3v5" />
          </svg>
        </span>
      )}
      {name === "oxc" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="m12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83Z" />
            <path d="m22 17.65-9.17 4.16a2 2 0 0 1-1.66 0L2 17.65" />
            <path d="m22 12.65-9.17 4.16a2 2 0 0 1-1.66 0L2 12.65" />
          </svg>
        </span>
      )}
      {name === "css" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <circle cx="6" cy="6" r="3" />
            <path d="M8.12 8.12 12 12" />
            <path d="M20 4 8.12 15.88" />
            <circle cx="6" cy="18" r="3" />
            <path d="M14.8 14.8 20 20" />
          </svg>
        </span>
      )}
      {name === "tailwind" && (
        <img src="/img/brands/tailwind.svg" alt="" aria-hidden="true" draggable="false" className={CHIP} />
      )}
      {name === "capabilities" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <circle cx="7.5" cy="15.5" r="5.5" />
            <path d="m21 2-9.6 9.6" />
            <path d="m15.5 7.5 3 3L22 7l-3-3" />
          </svg>
        </span>
      )}
      {name === "cookies" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="M12 2a10 10 0 1 0 10 10 4 4 0 0 1-5-5 4 4 0 0 1-5-5" />
            <path d="M8.5 8.5v.01" />
            <path d="M16 15.5v.01" />
            <path d="M12 12v.01" />
            <path d="M11 17v.01" />
            <path d="M7 14v.01" />
          </svg>
        </span>
      )}
      {name === "otel" && (
        <span className={TILE}>
          <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            <path d="m22 2-7 20-4-9-9-4Z" />
            <path d="M22 2 11 13" />
          </svg>
        </span>
      )}
    </>
  );
}
