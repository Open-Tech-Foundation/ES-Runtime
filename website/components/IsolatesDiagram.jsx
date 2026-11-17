// Isolates, drawn as what the pool does: tenants' code arrives as small
// isolates, each placed on the pool thread with the fewest; the isolates on a
// thread take turns; one asks its host for data and gets it, one reaches for
// the network and is refused, one gets stuck in a loop and is terminated while
// the rest go on.
//
// Every frame is something the runtime does (D227): least-loaded placement,
// one turn at a time per thread, `host` as the only way out, a denial that
// says "computation only", and `terminate()` ending a spinning call.
//
// HTML rather than SVG, because AFW's transitions animate the HTML elements a
// keyed list adds and removes: an isolate scales in when it starts and fades
// out when it ends, and each line of the log flies in. Which chip is taking
// its turn, asking its host or being refused is read from state of its own
// (`turns`, `marks`) rather than from the list items, so a chip that stays in
// the list still changes colour. The initial frame is fixed, so the server's
// HTML and the hydrated page agree; motion starts after mount, pauses while the
// panel is off screen, and transitions honour `prefers-reduced-motion`.
//
// The panel is dark in both site themes, like the other diagrams. Text on it
// meets WCAG AA (4.5:1): headings are zinc-100 (16:1 on zinc-900), secondary
// labels zinc-400 (6.8:1); zinc-500 (3.7:1) is too faint for text this small.

import { fade, fly, scale } from "@opentf/afw/transition";

const THREADS = [0, 1, 2, 3];

// The first twelve isolates.
const NAMES = [
  "acme · discount",
  "plugin · markdown",
  "rule · tax-eu",
  "model · analysis",
  "pdf · extract",
  "tenant-42 · hook",
  "plugin · sitemap",
  "rule · shipping",
  "model · summary",
  "csv · import",
  "tenant-7 · hook",
  "plugin · resize",
];

// What later tenants run; each new isolate gets a tenant number of its own, so
// no two chips on screen share a name.
const KINDS = ["discount", "markdown", "tax-rule", "analysis", "extract", "webhook"];

// What happens on each tick after the turns move on, round and round.
const SCRIPT = [
  "spawn",
  "host",
  "spawn",
  "deny",
  "spawn",
  "spin",
  "host",
  "terminate",
  "spawn",
  "retire",
  "host",
  "spawn",
  "retire",
  "deny",
  "retire",
  "retire",
];

const MAX_LOG = 4;
const TICK_MS = 1300;

// The first frame: three isolates on each thread.
const INITIAL = THREADS.map((t) =>
  [0, 1, 2].map((i) => ({ id: t * 3 + i + 1, name: NAMES[(t * 3 + i) % NAMES.length] })),
);

const CHIP =
  "inline-flex items-center gap-1.5 rounded-md px-2 py-1 font-mono text-[11px] leading-none ring-1 transition-colors duration-300 ";

function chipClass(kind) {
  if (kind === "host") return CHIP + "bg-emerald-950 text-emerald-200 ring-emerald-500";
  if (kind === "deny") return CHIP + "bg-rose-950 text-rose-200 ring-rose-500";
  if (kind === "spin") return CHIP + "animate-pulse bg-yellow-950 text-yellow-200 ring-yellow-400";
  if (kind === "turn") return CHIP + "bg-brand-950 text-brand-200 ring-brand-500";
  return CHIP + "bg-zinc-800 text-zinc-400 ring-zinc-700";
}

function dotClass(kind) {
  if (kind === "host") return "size-1.5 rounded-full bg-emerald-400";
  if (kind === "deny") return "size-1.5 rounded-full bg-rose-400";
  if (kind === "spin") return "size-1.5 rounded-full bg-yellow-400";
  if (kind === "turn") return "size-1.5 rounded-full bg-brand-400";
  return "size-1.5 rounded-full bg-zinc-600";
}

function logClass(kind) {
  const base = "truncate font-mono text-[12px] leading-6 ";
  if (kind === "host") return base + "text-emerald-300";
  if (kind === "deny") return base + "text-rose-300";
  if (kind === "spin") return base + "text-yellow-300";
  return base + "text-zinc-400";
}

export default function IsolatesDiagram() {
  let lanes = $state(INITIAL);
  // The id of the isolate taking its turn, per thread. An id, not an index: a
  // chip that stays in a keyed list keeps the index it was made with.
  let turns = $state(INITIAL.map((lane) => lane[0].id));
  // id → "host" | "deny" | "spin", for the chips doing something visible.
  let marks = $state({});
  let log = $state([
    { id: 1, kind: "", text: "12 isolates on 4 threads, taking turns" },
  ]);

  const panel = $ref();

  onMount(() => {
    let tick = 0;
    let nextId = 100;
    let nextTenant = 13;
    let nextLog = 2;
    let spinning = null;
    // Where each thread's turn is, among its isolates.
    let at = [0, 0, 0, 0];

    const live = () => lanes.flat();
    const say = (kind, text) => {
      log = [...log, { id: nextLog++, kind, text }].slice(-MAX_LOG);
    };
    const pick = (offset) => {
      const all = live().filter((i) => marks[i.id] === undefined);
      return all.length ? all[(tick * 7 + offset) % all.length] : null;
    };

    const step = () => {
      tick += 1;
      // Clear last tick's host and deny marks; a spinning isolate stays marked.
      const kept = {};
      if (spinning !== null) kept[spinning] = "spin";
      marks = kept;
      // Every thread's turn moves to its next isolate.
      at = at.map((a, i) => (lanes[i].length ? (a + 1) % lanes[i].length : 0));

      const event = SCRIPT[tick % SCRIPT.length];
      if (event === "spawn") {
        const fewest = lanes.reduce((best, lane, i) => (lane.length < lanes[best].length ? i : best), 0);
        const tenant = nextTenant++;
        const name = `tenant-${tenant} · ${KINDS[tenant % KINDS.length]}`;
        lanes = lanes.map((lane, i) => (i === fewest ? [...lane, { id: nextId++, name }] : lane));
        say("", `new Isolate(${JSON.stringify(name.split(" · ")[1])}) → thread ${fewest + 1}`);
      } else if (event === "host") {
        const who = pick(1);
        if (who) {
          marks = { ...marks, [who.id]: "host" };
          say("host", `${who.name}  host.getUser(42)  ✓`);
        }
      } else if (event === "deny") {
        const who = pick(3);
        if (who) {
          marks = { ...marks, [who.id]: "deny" };
          say("deny", `${who.name}  fetch()  ✗ computation only`);
        }
      } else if (event === "spin") {
        const who = pick(5);
        if (who) {
          spinning = who.id;
          marks = { ...marks, [who.id]: "spin" };
          say("spin", `${who.name}  for (;;) {}  — its thread's others wait`);
        }
      } else if (event === "terminate") {
        if (spinning !== null) {
          const gone = live().find((i) => i.id === spinning);
          lanes = lanes.map((lane) => lane.filter((i) => i.id !== spinning));
          spinning = null;
          marks = {};
          if (gone) say("", `${gone.name}  terminate()  · ERR_ISOLATE_TERMINATED`);
        }
      } else if (event === "retire") {
        // The busiest thread's oldest isolate is done; it ends and leaves.
        const busiest = lanes.reduce((best, lane, i) => (lane.length > lanes[best].length ? i : best), 0);
        const oldest = lanes[busiest].find((i) => i.id !== spinning);
        if (oldest && live().length > 10) {
          lanes = lanes.map((lane) => lane.filter((i) => i.id !== oldest.id));
          say("", `${oldest.name}  done · terminate()`);
        }
      }
      at = at.map((a, i) => (lanes[i].length ? a % lanes[i].length : 0));
      turns = lanes.map((lane, i) => lane[at[i]]?.id ?? null);
    };

    // Runs only while the panel is on screen.
    let timer = null;
    const observer = new IntersectionObserver((entries) => {
      const visible = entries.some((e) => e.isIntersecting);
      if (visible && timer === null) timer = setInterval(step, TICK_MS);
      if (!visible && timer !== null) {
        clearInterval(timer);
        timer = null;
      }
    });
    observer.observe(panel);
    return () => {
      observer.disconnect();
      if (timer !== null) clearInterval(timer);
    };
  });

  return (
    <div
      ref={panel}
      role="img"
      aria-label="Tenants' code runs as small isolates on a pool of four threads. Each new isolate goes to the thread with the fewest, and the isolates on a thread take turns. One asks its host for a user record and gets it; one calls fetch and is refused, because an isolate can only compute; one loops forever and is terminated while the others carry on."
      className="grid gap-6 font-mono md:grid-cols-[13rem_1fr]"
    >
      {/* ---- the program: what the isolates may ask for -------------------- */}
      <div className="grid grid-cols-2 gap-3 md:flex md:flex-col md:gap-4">
        <div className="col-span-2 text-[11px] tracking-[0.14em] text-zinc-100">YOUR PROGRAM</div>
        <div className="rounded-xl bg-zinc-800 p-4 ring-1 ring-zinc-700">
          <div className="text-[13px] text-zinc-200">host</div>
          <div className="mt-2 space-y-1 text-[12px]">
            <div className="text-emerald-300">getUser(id)</div>
            <div className="text-emerald-300">log(line)</div>
          </div>
          <div className="mt-3 border-t border-zinc-700 pt-3 text-[11px] leading-relaxed text-zinc-400">
            the only way out
          </div>
        </div>
        <div className="rounded-xl bg-zinc-900 p-4 ring-1 ring-zinc-800">
          <div className="text-[11px] tracking-[0.14em] text-zinc-100">INSIDE AN ISOLATE</div>
          <div className="mt-2 space-y-1 text-[12px]">
            <div className="text-zinc-300">✓ compute</div>
            <div className="text-rose-300">✗ files</div>
            <div className="text-rose-300">✗ network</div>
            <div className="text-rose-300">✗ environment</div>
          </div>
        </div>
      </div>

      {/* ---- the pool -------------------------------------------------------- */}
      <div className="flex min-w-0 flex-col gap-3">
        <div className="flex items-baseline justify-between text-[11px] tracking-[0.14em] text-zinc-100">
          <span>
            ISOLATE POOL<span className="hidden sm:inline"> · ONE THREAD PER CORE</span>
          </span>
          <span className="shrink-0 text-zinc-300">{lanes.flat().length} live</span>
        </div>
        {THREADS.map((t) => (
          <div key={t} className="flex min-h-[3.25rem] items-center gap-3 rounded-xl bg-zinc-900/70 px-3 py-2 ring-1 ring-zinc-800">
            <div className="w-16 shrink-0 text-[11px] text-zinc-400">thread {t + 1}</div>
            <div className="flex flex-wrap gap-1.5">
              {lanes[t].map((item) => (
                <span
                  key={item.id}
                  in={scale({ start: 0.6, duration: 260 })}
                  out={fade({ duration: 240 })}
                  className={chipClass(marks[item.id] ?? (turns[t] === item.id ? "turn" : ""))}
                >
                  <span className={dotClass(marks[item.id] ?? (turns[t] === item.id ? "turn" : ""))} />
                  {item.name}
                  {marks[item.id] === "spin" ? " ⟳ looping" : ""}
                </span>
              ))}
            </div>
          </div>
        ))}

        {/* ---- what just happened ------------------------------------------- */}
        <div className="mt-1 rounded-xl bg-zinc-800/60 px-4 py-2 ring-1 ring-zinc-700">
          <div className="flex h-24 flex-col justify-end overflow-hidden">
            {log.map((line) => (
              <div key={line.id} in={fly({ y: 8, duration: 240 })} out={fade({ duration: 160 })} className={logClass(line.kind)}>
                {line.text}
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
