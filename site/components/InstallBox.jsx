// A terminal-style install command with OS tabs and a copy button. The command
// wraps instead of scrolling so it stays readable; on copy it turns green and
// the button shows a ✓. State is the framework's component-local reactivity
// ($state -> signal; the compiler wraps the className/child reads in effects).
// Reactive primitives must come from the framework, never another signals
// package (single-source-of-truth rule in the @opentf/web SPEC).
const RAW = "https://raw.githubusercontent.com/Open-Tech-Foundation/ES-Runtime/main";
const UNIX = `curl -fsSL ${RAW}/install.sh | bash`;
const WIN = `irm ${RAW}/install.ps1 | iex`;

const CODE_BASE = "flex-1 break-all text-[13px] leading-relaxed transition-colors ";
const TAB_BASE = "-mb-px border-b-2 px-3 py-2 text-xs font-semibold transition-colors ";

export default function InstallBox() {
  let active = $state("unix"); // "unix" | "win"
  let copied = $state(false);

  function select(os) {
    active = os;
    copied = false;
  }

  function copy() {
    navigator.clipboard?.writeText(active === "win" ? WIN : UNIX);
    copied = true;
    setTimeout(() => (copied = false), 1600);
  }

  return (
    <div className="overflow-hidden rounded-xl border border-zinc-800 bg-zinc-950">
      {/* Tabs */}
      <div className="flex items-center gap-1 border-b border-zinc-800 px-2 pt-2">
        <button
          type="button"
          onclick={() => select("unix")}
          className={
            TAB_BASE +
            (active === "unix"
              ? "border-brand-500 text-white"
              : "border-transparent text-zinc-400 hover:text-zinc-200")
          }
        >
          Linux / macOS
        </button>
        <button
          type="button"
          onclick={() => select("win")}
          className={
            TAB_BASE +
            (active === "win"
              ? "border-brand-500 text-white"
              : "border-transparent text-zinc-400 hover:text-zinc-200")
          }
        >
          Windows (PowerShell)
        </button>
      </div>

      {/* Command */}
      <div className="flex items-start gap-3 px-4 py-3.5">
        <code className={CODE_BASE + " whitespace-pre-wrap " + (copied ? "text-emerald-400" : "text-zinc-100")}>
          <span className="select-none text-brand-400">
            {active === "win" ? "> " : "$ "}
          </span>
          {active === "win" ? WIN : UNIX}
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
