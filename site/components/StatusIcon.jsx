// SVG status glyphs for support tables and scope lists: "yes" (check),
// "partial" (warning), "no" (cross).
//
// These are drawn as STROKED paths (fill="none", stroke="currentColor"),
// deliberately avoiding fill-rule/clip-rule cut-outs: the @opentf/web compiler
// does not emit the kebab-case `fill-rule`/`clip-rule` SVG attributes, so an
// even-odd cut-out silently fills solid (a dot with no cross). Stroke geometry
// renders reliably. Pass `className` to override size/color.

export default function StatusIcon({ status, title, className }) {
  if (status === "yes") {
    return (
      <svg
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2.5"
        className={className || "inline-block h-[18px] w-[18px] text-emerald-600"}
        aria-label={title || "Supported"}
        role="img"
      >
        <path d="M5 12.5 L10 17.5 L19 6.5" fill="none" />
      </svg>
    );
  }
  if (status === "partial") {
    return (
      <svg
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        className={className || "inline-block h-[18px] w-[18px] text-amber-500"}
        aria-label={title || "Partial"}
        role="img"
      >
        <path d="M12 3.5 L22 20 L2 20 Z" fill="none" />
        <path d="M12 10 L12 14" fill="none" />
        <circle cx="12" cy="17" r="1.1" fill="currentColor" stroke="none" />
      </svg>
    );
  }
  // "no"
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.5"
      className={className || "inline-block h-[18px] w-[18px] text-zinc-300"}
      aria-label={title || "Not supported"}
      role="img"
    >
      <path d="M6 6 L18 18" fill="none" />
      <path d="M18 6 L6 18" fill="none" />
    </svg>
  );
}
