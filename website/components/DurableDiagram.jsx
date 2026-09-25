// A durable worker, drawn as the story the shop example tells: three customers
// want the last two hoodies, their calls queue in one worker's mailbox, each
// runs alone, each write commits to the worker's own SQLite file before its
// caller is answered — and when the process is killed, the worker comes back
// from that file with everything anyone was told still true.
//
// Every frame is something the runtime actually does, in the order it does it:
// the mailbox runs one call at a time (D80), a result is released only after
// its commit (the gate, D80/D128), a call that finds nothing left writes
// nothing, and `SIGKILL` loses no acknowledged write (the shop benchmark and the
// kill tests). The state reads "in memory only" between a `set` and its commit,
// because that window is exactly what the gate exists to hide.
//
// Written out, not generated, for the reason SandboxDiagram gives: every
// element exists in every frame, and a frame only changes text, classes and
// opacity through expressions that read `step` directly. A list built with
// `.map()` or an element that only exists in some frames is not re-rendered
// when the step changes, and an element created on the client outside the
// server-rendered tree can land outside the SVG namespace and draw nothing.
//
// Every label is anchored with `text-anchor` and `dominant-baseline="central"`
// against its shape; the compiler does not rewrite camelCase SVG attributes, so
// they are written kebab-case. The panel is dark in both site themes, so there
// are no `dark:` variants in here.

// One frame per step. `told` is what each customer has been answered so far.
const STEPS = [
  {
    queue: ["Ana", "Ben", "Chloé"],
    running: "",
    mem: "left: 2",
    saved: "on disk",
    disk: "left: 2",
    told: {},
    caption: "Three customers want the last two hoodies. Their calls queue in one worker's mailbox.",
  },
  {
    queue: ["Ben", "Chloé"],
    running: "Ana",
    mem: "left: 1",
    saved: "in memory only",
    disk: "left: 2",
    told: {},
    caption: "One call at a time. Ana's runs, and the stock drops to 1 — in memory, not yet on disk.",
  },
  {
    queue: ["Ben", "Chloé"],
    running: "Ana",
    mem: "left: 1",
    saved: "on disk",
    disk: "left: 1",
    commit: true,
    told: {},
    caption: "The write commits to the worker's own SQLite file.",
  },
  {
    queue: ["Chloé"],
    running: "Ben",
    mem: "left: 0",
    saved: "in memory only",
    disk: "left: 1",
    told: { Ana: "ok" },
    caption: "Only now is Ana answered. Ben's call runs next.",
  },
  {
    queue: ["Chloé"],
    running: "Ben",
    mem: "left: 0",
    saved: "on disk",
    disk: "left: 0",
    commit: true,
    told: { Ana: "ok" },
    caption: "It commits, and then Ben is answered. Nobody is told anything the disk has not heard.",
  },
  {
    queue: [],
    running: "Chloé",
    mem: "left: 0",
    saved: "on disk",
    disk: "left: 0",
    told: { Ana: "ok", Ben: "ok" },
    caption: "Chloé's call finds none left. No lock, no race: the last hoodie cannot be sold twice.",
  },
  {
    queue: [],
    running: "",
    mem: "",
    saved: "",
    disk: "left: 0",
    crash: true,
    told: { Ana: "ok", Ben: "ok", Chloé: "out" },
    caption: "SIGKILL. The process dies with no warning and no chance to tidy up.",
  },
  {
    queue: [],
    running: "",
    mem: "left: 0",
    saved: "read from disk",
    disk: "left: 0",
    restart: true,
    told: { Ana: "ok", Ben: "ok", Chloé: "out" },
    caption: "Restarted: the worker reopens from its file. Everything anyone was told is still true.",
  },
];

// Per-frame reads, each a plain function of the step so every expression in
// the tree depends on `step` directly.
const at = (s) => STEPS[s];
const isRunning = (s, name) => at(s).running === name;
const isQueued = (s, name) => at(s).queue.includes(name);
const told = (s, name) => at(s).told[name] ?? "";
const replyText = (s, name) => (told(s, name) === "ok" ? "✓ got one" : told(s, name) === "out" ? "✗ sold out" : "");
const replyChip = (s, name) =>
  "edw-t " + (told(s, name) === "ok" ? "fill-emerald-950" : told(s, name) === "out" ? "fill-rose-950" : "fill-zinc-900");
const replyInk = (s, name) => (told(s, name) === "out" ? "fill-rose-300" : "fill-emerald-300");
const callerBox = (s, name) =>
  "edw-t " + (isRunning(s, name) ? "fill-zinc-800 stroke-brand-500" : "fill-zinc-800/60 stroke-zinc-700");
const callerLine = (s, name) => "edw-t " + (isRunning(s, name) ? "stroke-brand-500" : "stroke-zinc-700");
const callerDash = (s, name) => (isQueued(s, name) ? "4 5" : "0");
const slotName = (s, i) => at(s).queue[i] ?? "";
const slotBox = (s, i) => "edw-t " + (at(s).queue[i] ? "fill-zinc-700 stroke-zinc-600" : "fill-zinc-900 stroke-zinc-700");
const savedChip = (s) =>
  "edw-t " +
  (at(s).saved === "in memory only" ? "fill-amber-950" : at(s).saved ? "fill-emerald-950" : "fill-zinc-900");
const savedInk = (s) => (at(s).saved === "in memory only" ? "fill-amber-300" : "fill-emerald-300");
const shown = (on) => (on ? "1" : "0");

// Columns.
const CARD_X = 330;
const CARD_W = 330;
const CARD_MID = CARD_X + CARD_W / 2; // 495
const DISK_X = 780; // the file's centre
const ROW_A = 170;
const ROW_B = 262;
const ROW_C = 354;
const DOTS_X = 480 - (STEPS.length - 1) * 7;

export default function DurableDiagram() {
  let step = $state(0);

  onMount(() => {
    const timer = setInterval(() => {
      step = (step + 1) % STEPS.length;
    }, 2600);
    // Returned rather than `onCleanup(...)`: the lifecycle macros only resolve
    // at component top level (see SandboxDiagram).
    return () => clearInterval(timer);
  });

  return (
    <div className="esr-scroller overflow-x-auto" data-lenis-prevent>
      <svg
        viewBox="0 0 960 540"
        role="img"
        aria-label="Three customers call one durable worker holding the stock of a hoodie. Their calls wait in the worker's mailbox and run one at a time. Each call's write commits to the worker's own SQLite file before that customer is answered, so two customers get a hoodie and the third is told it is sold out. Then the process is killed; on restart the worker reopens from its file and the stock is exactly what the customers were told."
        className="h-auto w-full min-w-[860px]"
        style="font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"
      >
        {/* ---- column headers ----------------------------------------------- */}
        <g className="fill-zinc-500" font-size="11" letter-spacing="1.4" text-anchor="middle" dominant-baseline="central">
          <text x="115" y="40">CALLERS</text>
          <text x={CARD_MID} y="40">ONE DURABLE WORKER</text>
          <text x={DISK_X} y="40">ITS FILE · SQLITE</text>
        </g>
        <line x1="32" y1="60" x2="928" y2="60" className="stroke-zinc-800" stroke-width="1" />

        {/* ---- callers: request lines first, so the boxes sit on top ---------- */}
        <g stroke-width="1.5">
          <line x1="190" y1={ROW_A} x2={CARD_X} y2={ROW_A} className={callerLine(step, "Ana")} stroke-dasharray={callerDash(step, "Ana")} />
          <line x1="190" y1={ROW_B} x2={CARD_X} y2={ROW_B} className={callerLine(step, "Ben")} stroke-dasharray={callerDash(step, "Ben")} />
          <line x1="190" y1={ROW_C} x2={CARD_X} y2={ROW_C} className={callerLine(step, "Chloé")} stroke-dasharray={callerDash(step, "Chloé")} />
        </g>
        <g stroke-width="1.5">
          <rect x="40" y={ROW_A - 26} width="150" height="52" rx="10" className={callerBox(step, "Ana")} />
          <rect x="40" y={ROW_B - 26} width="150" height="52" rx="10" className={callerBox(step, "Ben")} />
          <rect x="40" y={ROW_C - 26} width="150" height="52" rx="10" className={callerBox(step, "Chloé")} />
        </g>
        <g font-size="14" dominant-baseline="central" className="fill-zinc-200">
          <text x="58" y={ROW_A - 8}>Ana</text>
          <text x="58" y={ROW_B - 8}>Ben</text>
          <text x="58" y={ROW_C - 8}>Chloé</text>
        </g>
        <g font-size="12" dominant-baseline="central" className="fill-zinc-500">
          <text x="58" y={ROW_A + 11}>take(1)</text>
          <text x="58" y={ROW_B + 11}>take(1)</text>
          <text x="58" y={ROW_C + 11}>take(1)</text>
        </g>

        {/* What each caller has been told — present in every frame, shown once
            there is an answer. */}
        <g className="edw-o" opacity={shown(told(step, "Ana"))}>
          <rect x="206" y={ROW_A + 10} width="104" height="26" rx="13" className={replyChip(step, "Ana")} />
          <text x="258" y={ROW_A + 23} font-size="12" text-anchor="middle" dominant-baseline="central" className={replyInk(step, "Ana")}>
            {replyText(step, "Ana")}
          </text>
        </g>
        <g className="edw-o" opacity={shown(told(step, "Ben"))}>
          <rect x="206" y={ROW_B + 10} width="104" height="26" rx="13" className={replyChip(step, "Ben")} />
          <text x="258" y={ROW_B + 23} font-size="12" text-anchor="middle" dominant-baseline="central" className={replyInk(step, "Ben")}>
            {replyText(step, "Ben")}
          </text>
        </g>
        <g className="edw-o" opacity={shown(told(step, "Chloé"))}>
          <rect x="206" y={ROW_C + 10} width="104" height="26" rx="13" className={replyChip(step, "Chloé")} />
          <text x="258" y={ROW_C + 23} font-size="12" text-anchor="middle" dominant-baseline="central" className={replyInk(step, "Chloé")}>
            {replyText(step, "Chloé")}
          </text>
        </g>

        {/* ---- the worker ---------------------------------------------------- */}
        <rect
          x={CARD_X}
          y="96"
          width={CARD_W}
          height="332"
          rx="14"
          className={"edw-t " + (at(step).crash ? "fill-zinc-900 stroke-rose-500" : "fill-zinc-800 stroke-zinc-700")}
          stroke-width="1.5"
        />
        <text x={CARD_MID} y="124" font-size="14" text-anchor="middle" dominant-baseline="central" className="fill-zinc-200">
          Stock("hoodie")
        </text>

        <g font-size="11" letter-spacing="1.2" dominant-baseline="central" className="fill-zinc-500">
          <text x="352" y="162">MAILBOX</text>
          <text x="352" y="214">RUNNING</text>
          <text x="352" y="280">STATE</text>
        </g>

        {/* Mailbox: who is waiting, in order. */}
        <g stroke-width="1">
          <rect x="436" y="148" width="62" height="28" rx="7" className={slotBox(step, 0)} />
          <rect x="506" y="148" width="62" height="28" rx="7" className={slotBox(step, 1)} />
          <rect x="576" y="148" width="62" height="28" rx="7" className={slotBox(step, 2)} />
        </g>
        <g font-size="12" text-anchor="middle" dominant-baseline="central" className="fill-zinc-300">
          <text x="467" y="162">{slotName(step, 0)}</text>
          <text x="537" y="162">{slotName(step, 1)}</text>
          <text x="607" y="162">{slotName(step, 2)}</text>
        </g>

        {/* Running: the one call inside the worker right now. */}
        <rect
          x="436"
          y="198"
          width="202"
          height="32"
          rx="8"
          className={"edw-t " + (at(step).running ? "fill-brand-950 stroke-brand-500" : "fill-zinc-900 stroke-zinc-700")}
          stroke-width="1.5"
        />
        <text
          x="537"
          y="214"
          font-size="13"
          text-anchor="middle"
          dominant-baseline="central"
          className={at(step).running ? "fill-brand-300" : "fill-zinc-600"}
        >
          {at(step).running ? `take(1) · ${at(step).running}` : at(step).crash ? "—" : "idle"}
        </text>

        {/* State: resident, so a read is a map lookup. */}
        <rect x="436" y="252" width="202" height="100" rx="10" className="fill-zinc-900 stroke-zinc-700" stroke-width="1" />
        <text x="537" y="288" font-size="30" font-weight="700" text-anchor="middle" dominant-baseline="central" className="fill-zinc-100">
          {at(step).mem}
        </text>
        <g className="edw-o" opacity={shown(at(step).saved)}>
          <rect x="467" y="316" width="140" height="22" rx="11" className={savedChip(step)} />
          <text x="537" y="327" font-size="11" text-anchor="middle" dominant-baseline="central" className={savedInk(step)}>
            {at(step).saved}
          </text>
        </g>
        <text
          x="537"
          y="302"
          font-size="13"
          text-anchor="middle"
          dominant-baseline="central"
          className="edw-o fill-rose-400"
          opacity={shown(at(step).crash)}
        >
          gone with the process
        </text>

        {/* The worker's promise, and the crash stamped over it. */}
        <text x={CARD_MID} y="393" font-size="12" text-anchor="middle" dominant-baseline="central" className="fill-zinc-500">
          answer only after commit
        </text>
        <g className="edw-o" opacity={shown(at(step).crash)}>
          <rect x="395" y="376" width="200" height="34" rx="8" className="fill-rose-950 stroke-rose-500" stroke-width="1.5" />
          <text x={CARD_MID} y="393" font-size="14" font-weight="700" text-anchor="middle" dominant-baseline="central" className="fill-rose-300">
            ⚡ SIGKILL
          </text>
        </g>

        {/* ---- the commit: worker → its own file, and back on a restart ------- */}
        <line
          x1={CARD_X + CARD_W}
          y1="302"
          x2={DISK_X - 56}
          y2="302"
          className={"edw-t " + (at(step).commit || at(step).restart ? "stroke-emerald-400" : "stroke-zinc-700")}
          stroke-width="1.5"
        />
        <circle cx="0" cy="302" r="5" className={"fill-emerald-400 " + (at(step).commit ? "edw-commit" : "edw-still")} />
        <circle cx="0" cy="302" r="5" className={"fill-emerald-400 " + (at(step).restart ? "edw-reload" : "edw-still")} />

        {/* The file: a database cylinder, lit while a commit lands. */}
        <g stroke-width="1.5">
          <path
            d={`M${DISK_X - 56} 226 v140 a56 18 0 0 0 112 0 v-140`}
            className={"edw-t " + (at(step).commit ? "fill-emerald-950 stroke-emerald-400" : "fill-zinc-800 stroke-zinc-600")}
          />
          <ellipse
            cx={DISK_X}
            cy="226"
            rx="56"
            ry="18"
            className={"edw-t " + (at(step).commit ? "fill-emerald-900 stroke-emerald-400" : "fill-zinc-700 stroke-zinc-600")}
          />
        </g>
        <text x={DISK_X} y="284" font-size="12" text-anchor="middle" dominant-baseline="central" className="fill-zinc-400">
          hoodie.db
        </text>
        <text x={DISK_X} y="316" font-size="22" font-weight="700" text-anchor="middle" dominant-baseline="central" className="fill-zinc-100">
          {at(step).disk}
        </text>
        <text
          x={DISK_X}
          y="404"
          font-size="12"
          text-anchor="middle"
          dominant-baseline="central"
          className={"edw-o " + (at(step).commit ? "fill-emerald-300" : "fill-zinc-400")}
          opacity={shown(at(step).commit || at(step).crash)}
        >
          {at(step).commit ? "✓ committed" : "untouched"}
        </text>

        {/* ---- what is happening, in words ------------------------------------ */}
        <rect x="40" y="462" width="880" height="48" rx="10" className="fill-zinc-800/60 stroke-zinc-700" stroke-width="1" />
        <text x="480" y="486" font-size="13" text-anchor="middle" dominant-baseline="central" className="fill-zinc-300">
          {at(step).caption}
        </text>

        {/* Progress: which step of the story this is. */}
        <g>
          <circle cx={DOTS_X} cy="528" r="3" className={"edw-t " + (step === 0 ? "fill-brand-500" : "fill-zinc-700")} />
          <circle cx={DOTS_X + 14} cy="528" r="3" className={"edw-t " + (step === 1 ? "fill-brand-500" : "fill-zinc-700")} />
          <circle cx={DOTS_X + 28} cy="528" r="3" className={"edw-t " + (step === 2 ? "fill-brand-500" : "fill-zinc-700")} />
          <circle cx={DOTS_X + 42} cy="528" r="3" className={"edw-t " + (step === 3 ? "fill-brand-500" : "fill-zinc-700")} />
          <circle cx={DOTS_X + 56} cy="528" r="3" className={"edw-t " + (step === 4 ? "fill-brand-500" : "fill-zinc-700")} />
          <circle cx={DOTS_X + 70} cy="528" r="3" className={"edw-t " + (step === 5 ? "fill-brand-500" : "fill-zinc-700")} />
          <circle cx={DOTS_X + 84} cy="528" r="3" className={"edw-t " + (step === 6 ? "fill-brand-500" : "fill-zinc-700")} />
          <circle cx={DOTS_X + 98} cy="528" r="3" className={"edw-t " + (step === 7 ? "fill-brand-500" : "fill-zinc-700")} />
        </g>
      </svg>
    </div>
  );
}
