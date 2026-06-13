// SVG status glyphs for support tables: "yes" (check), "partial" (warning),
// "no" (cross). Inline SVG so they scale and color crisply.

export default function StatusIcon({ status, title }) {
  if (status === "yes") {
    return (
      <svg
        viewBox="0 0 20 20"
        className="inline-block h-[18px] w-[18px] text-emerald-600"
        fill="currentColor"
        aria-label={title || "Supported"}
        role="img"
      >
        <path
          fillRule="evenodd"
          d="M10 18a8 8 0 100-16 8 8 0 000 16zm3.7-9.3a1 1 0 00-1.4-1.4L9 10.6 7.7 9.3a1 1 0 00-1.4 1.4l2 2a1 1 0 001.4 0l4-4z"
          clipRule="evenodd"
        />
      </svg>
    );
  }
  if (status === "partial") {
    return (
      <svg
        viewBox="0 0 20 20"
        className="inline-block h-[18px] w-[18px] text-amber-500"
        fill="currentColor"
        aria-label={title || "Partial"}
        role="img"
      >
        <path
          fillRule="evenodd"
          d="M8.3 2.5a2 2 0 013.4 0l6.5 11A2 2 0 0116.5 16.5h-13a2 2 0 01-1.7-3l6.5-11zM10 7a1 1 0 00-1 1v3a1 1 0 102 0V8a1 1 0 00-1-1zm0 7.5a1.1 1.1 0 100-2.2 1.1 1.1 0 000 2.2z"
          clipRule="evenodd"
        />
      </svg>
    );
  }
  // "no"
  return (
    <svg
      viewBox="0 0 20 20"
      className="inline-block h-[18px] w-[18px] text-zinc-300"
      fill="currentColor"
      aria-label={title || "Not supported"}
      role="img"
    >
      <path
        fillRule="evenodd"
        d="M10 18a8 8 0 100-16 8 8 0 000 16zm2.8-10.8a1 1 0 00-1.4 0L10 8.6 8.6 7.2a1 1 0 10-1.4 1.4L8.6 10l-1.4 1.4a1 1 0 101.4 1.4L10 11.4l1.4 1.4a1 1 0 001.4-1.4L11.4 10l1.4-1.4a1 1 0 000-1.4z"
        clipRule="evenodd"
      />
    </svg>
  );
}
