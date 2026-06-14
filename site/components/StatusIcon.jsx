// SVG status badges for support tables and scope lists: "yes" (green circle with
// a white check), "no" (red circle with a white cross), "partial" (amber warning
// triangle with a white "!").
//
// Compiler quirks to respect (@opentf/web emits SVG attributes verbatim and only
// auto-unwraps prop signals when they're read inside JSX, not in body `if`s):
//   - branch on `status` via JSX conditionals, NOT a body `if (status === ...)`
//     — a body read leaves `status` as the raw signal, so `=== "yes"` is always
//     false and every badge falls through to the last branch;
//   - write stroke attributes in KEBAB case (`stroke-width`, not `strokeWidth`)
//     or they're dropped and strokes fall back to width 1 (near-invisible);
//   - avoid fill-rule/clip-rule cut-outs (kebab names aren't emitted).
// Pass `className` to override size/color.

const SIZE = "inline-block h-6 w-6 align-middle ";

export default function StatusIcon({ status, title, className }) {
  return (
    <>
      {status === "yes" && (
        <svg
          viewBox="0 0 24 24"
          className={className || SIZE + "text-emerald-500"}
          aria-label={title || "Supported"}
          role="img"
        >
          <circle cx="12" cy="12" r="11" fill="currentColor" stroke="none" />
          <path
            d="M7 12.5 L10.5 16 L17 8.5"
            fill="none"
            stroke="white"
            stroke-width="2.5"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </svg>
      )}

      {status === "partial" && (
        <svg
          viewBox="0 0 24 24"
          className={className || SIZE + "text-amber-500"}
          aria-label={title || "Partial"}
          role="img"
        >
          <path
            d="M12 2.5 L22.5 21 L1.5 21 Z"
            fill="currentColor"
            stroke="none"
            stroke-linejoin="round"
          />
          <path
            d="M12 9.5 L12 14.5"
            fill="none"
            stroke="white"
            stroke-width="2"
            stroke-linecap="round"
          />
          <circle cx="12" cy="17.8" r="1.2" fill="white" stroke="none" />
        </svg>
      )}

      {status === "no" && (
        <svg
          viewBox="0 0 24 24"
          className={className || SIZE + "text-rose-500"}
          aria-label={title || "Not supported"}
          role="img"
        >
          <circle cx="12" cy="12" r="11" fill="currentColor" stroke="none" />
          <path
            d="M8.5 8.5 L15.5 15.5"
            fill="none"
            stroke="white"
            stroke-width="2.5"
            stroke-linecap="round"
          />
          <path
            d="M15.5 8.5 L8.5 15.5"
            fill="none"
            stroke="white"
            stroke-width="2.5"
            stroke-linecap="round"
          />
        </svg>
      )}
    </>
  );
}
