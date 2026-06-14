// A terminal-style install command with a copy button. On copy the command
// turns green and the button shows a tick. The command wraps instead of
// scrolling so the full line stays readable.
import { signal } from "@preact/signals-core";

const CMD =
  "curl -fsSL https://raw.githubusercontent.com/Open-Tech-Foundation/ES-Runtime/main/install.sh | bash";

export default function InstallBox() {
  const copied = signal(false);

  function copy() {
    navigator.clipboard?.writeText(CMD);
    copied.value = true;
    setTimeout(() => {
      copied.value = false;
    }, 1600);
  }

  return (
    <div className="overflow-hidden rounded-xl border border-zinc-800 bg-zinc-950">
      <div className="flex items-center gap-1.5 border-b border-zinc-800 px-4 py-2.5">
        <span className="h-2.5 w-2.5 rounded-full bg-zinc-700" />
        <span className="h-2.5 w-2.5 rounded-full bg-zinc-700" />
        <span className="h-2.5 w-2.5 rounded-full bg-zinc-700" />
        <span className="ml-2 text-xs font-medium text-zinc-500">Install</span>
      </div>
      <div className="flex items-start gap-3 px-4 py-3.5">
        <code
          className={
            "flex-1 break-all text-[13px] leading-relaxed transition-colors " +
            (copied.value ? "text-emerald-400" : "text-zinc-100")
          }
        >
          <span className="select-none text-brand-400">$ </span>
          {CMD}
        </code>
        <button
          type="button"
          onclick={copy}
          className={
            "inline-flex shrink-0 items-center gap-1 rounded-md border px-2.5 py-1 text-xs font-medium transition-colors " +
            (copied.value
              ? "border-emerald-500 text-emerald-400"
              : "border-zinc-700 text-zinc-300 hover:border-brand-500 hover:text-brand-400")
          }
        >
          {copied.value ? (
            <>
              <svg
                viewBox="0 0 20 20"
                fill="currentColor"
                className="h-3.5 w-3.5"
                aria-hidden="true"
              >
                <path
                  fillRule="evenodd"
                  d="M16.7 5.3a1 1 0 010 1.4l-7.5 7.5a1 1 0 01-1.4 0L3.3 9.7a1 1 0 011.4-1.4l3.8 3.8 6.8-6.8a1 1 0 011.4 0z"
                  clipRule="evenodd"
                />
              </svg>
              Copied
            </>
          ) : (
            "Copy"
          )}
        </button>
      </div>
    </div>
  );
}
