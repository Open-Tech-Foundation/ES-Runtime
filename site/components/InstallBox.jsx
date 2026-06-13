// A terminal-style install command with a copy button. Imperative copy
// feedback (no signal needed) — flips the label briefly on click.
const CMD =
  "curl -fsSL https://raw.githubusercontent.com/Open-Tech-Foundation/ES-Runtime/main/install.sh | bash";

function copy(e) {
  const btn = e.currentTarget;
  navigator.clipboard?.writeText(CMD);
  const prev = btn.getAttribute("data-label");
  btn.textContent = "Copied";
  setTimeout(() => {
    btn.textContent = prev;
  }, 1200);
}

export default function InstallBox() {
  return (
    <div className="overflow-hidden rounded-xl border border-zinc-800 bg-zinc-950">
      <div className="flex items-center gap-1.5 border-b border-zinc-800 px-4 py-2.5">
        <span className="h-2.5 w-2.5 rounded-full bg-zinc-700" />
        <span className="h-2.5 w-2.5 rounded-full bg-zinc-700" />
        <span className="h-2.5 w-2.5 rounded-full bg-zinc-700" />
        <span className="ml-2 text-xs font-medium text-zinc-500">Install</span>
      </div>
      <div className="flex items-center gap-3 px-4 py-3.5">
        <code className="flex-1 overflow-x-auto whitespace-nowrap text-[13px] text-zinc-100">
          <span className="select-none text-brand-400">$ </span>
          {CMD}
        </code>
        <button
          type="button"
          data-label="Copy"
          onclick={copy}
          className="shrink-0 rounded-md border border-zinc-700 px-2.5 py-1 text-xs font-medium text-zinc-300 transition-colors hover:border-brand-500 hover:text-brand-400"
        >
          Copy
        </button>
      </div>
    </div>
  );
}
