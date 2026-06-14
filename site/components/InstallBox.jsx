// A terminal-style install command with a copy button. The command wraps
// instead of scrolling so it stays readable. On copy it turns green and the
// button shows a ✓ — driven by the framework's component-local reactivity
// ($state compiles to a signal; the compiler wraps the className/child reads in
// effects). Reactive primitives must come from the framework, never another
// signals package (single-source-of-truth rule in the @opentf/web SPEC).
const CMD =
  "curl -fsSL https://raw.githubusercontent.com/Open-Tech-Foundation/ES-Runtime/main/install.sh | bash";

export default function InstallBox() {
  let copied = $state(false);

  function copy() {
    navigator.clipboard?.writeText(CMD);
    copied = true;
    setTimeout(() => (copied = false), 1600);
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
            (copied ? "text-emerald-400" : "text-zinc-100")
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
            (copied
              ? "border-emerald-500 text-emerald-400"
              : "border-zinc-700 text-zinc-300 hover:border-brand-500 hover:text-brand-400")
          }
        >
          {copied ? "✓ Copied" : "Copy"}
        </button>
      </div>
    </div>
  );
}
