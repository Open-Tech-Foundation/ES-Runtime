// The capability sandbox, drawn as what it actually is: JavaScript in a V8
// isolate, every host call leaving through an op into Rust, and a wall there
// with a slot per capability — open only where the command line said so.
//
// The check happens at op dispatch (`OpDecl::requires`, crates/engine), not at
// the import. So a granted lane is a hole in the wall and a denied lane is a
// plug, and the pulse in a denied lane stops *at* the wall: `NotAllowedError`
// is raised before the effect, never a partial one (SECURITY.md, D65).
//
// The grant set changes on a timer. It is drawn from real command lines rather
// than by flipping bits, so every frame the diagram shows is a run somebody
// could actually start — including the two ends of the range, a bare `esrun
// app.js` that reaches nothing and an `--allow-all` that reaches everything.
// The command line and the wall always agree, because the wall is rendered
// *from* the command line.
//
// Geometry is written out rather than generated. Every label is anchored
// (`text-anchor` + `dominant-baseline="central"`) against a coordinate derived
// from the shape it sits in, so nothing depends on measured text width or on a
// webfont having loaded. Note that this compiler does *not* rewrite camelCase
// SVG attributes: they must be written `text-anchor`, not `textAnchor`, or the
// HTML parser drops them and every label loses its alignment.
//
// The panel is dark in both site themes — it is the one showcase surface on the
// page, and the brand orange and the host colours only carry on a dark ground.
// So there are no `dark:` variants in here: every colour is the dark one, and
// an open slot is filled with the panel's own background (zinc-900) because it
// is a hole rather than a shape.
//
// Keyframes and the pulse geometry live in app/global.css.

// Lane centres. Slots are 48 tall (cy ± 24).
const L = { net: 130, read: 200, listen: 270, write: 340, env: 410, run: 480 };

// Columns. Pulses start at x=200 and either reach x=700 or stop at x=384.
const WALL_X = 392;
const WALL_W = 220;
const MID = WALL_X + WALL_W / 2; // 502 — wall centre, and the command line's
const SLOT_X = 408;
const SLOT_W = 188;
const ICON = 430; // capability icon centre inside a slot
const NAME = 452; // capability name, anchored start
const LOCK = 574; // state icon centre inside a slot
const HOST_ICON = 716;
const HOST_TEXT = 742;

// Real runs, not random bits. Each is a command line and the grants it implies.
const RUNS = [
  { grants: ["net", "read", "listen"], cmd: "esrun --allow-net --allow-read --allow-listen app.js" },
  { grants: [], cmd: "esrun app.js" },
  { grants: ["read"], cmd: "esrun --allow-read app.js" },
  { grants: ["net", "env"], cmd: "esrun --allow-net --allow-env app.js" },
  { grants: ["read", "write", "listen"], cmd: "esrun --allow-read --allow-write --allow-listen app.js" },
  { grants: ["net", "read", "listen", "write", "env", "run"], cmd: "esrun --allow-all app.js" },
];

const has = (i, cap) => RUNS[i].grants.includes(cap);

// A 24×24 icon placed centred on (cx, cy) at `size` px.
const at = (cx, cy, size) =>
  `translate(${cx - size / 2} ${cy - size / 2}) scale(${size / 24})`;

const OPEN_SLOT = "esr-slot stroke-brand-500 fill-zinc-900";
const SHUT_SLOT =
  "esr-slot fill-zinc-700 stroke-zinc-600";
const OPEN_INK = "esr-ink stroke-brand-400";
const SHUT_INK = "esr-ink stroke-zinc-400";
const OPEN_TEXT = "esr-ink fill-brand-400";
const SHUT_TEXT = "esr-ink fill-zinc-400";
// Each host resource gets its own hue, so the right-hand column reads as a set
// of distinct things rather than a column of grey glyphs. This is identity, not
// state: an icon here only ever renders for a lane the command line granted,
// and granted-vs-denied stays orange-vs-grey inside the wall where it belongs.
const HOST = {
  net: ["esr-ink fill-sky-950", "esr-ink stroke-sky-400"],
  read: ["esr-ink fill-violet-950", "esr-ink stroke-violet-400"],
  listen: ["esr-ink fill-emerald-950", "esr-ink stroke-emerald-400"],
  write: ["esr-ink fill-rose-950", "esr-ink stroke-rose-400"],
  env: ["esr-ink fill-fuchsia-950", "esr-ink stroke-fuchsia-400"],
  run: ["esr-ink fill-indigo-950", "esr-ink stroke-indigo-400"],
};

// A denied lane still shows its resource — the thing exists, the run just
// cannot reach it, and that reads better than a gap where a row should be.
// It is the *line* that is missing, not the destination.
const HOST_OFF_CHIP = "esr-ink fill-zinc-800";
const HOST_OFF_INK = "esr-ink stroke-zinc-600";
const HOST_ON_TEXT = "esr-ink fill-zinc-300";
const HOST_OFF_TEXT = "esr-ink fill-zinc-600";

const PASS_PULSE = "esr-pulse esr-pass fill-brand-500";
const BLOCK_PULSE = "esr-pulse esr-block fill-zinc-500";

export default function SandboxDiagram() {
  // Index into RUNS. A scalar rather than a set of booleans: one signal, and
  // every derived class in the tree reads it directly inside its own JSX
  // expression, so nothing depends on proxy-based deep reactivity.
  let run = $state(0);

  onMount(() => {
    // One full pulse cycle per configuration, so a lane is never re-armed
    // mid-flight and the change reads as a new run rather than a glitch.
    const timer = setInterval(() => {
      let next = run;
      while (next === run) next = Math.floor(Math.random() * RUNS.length);
      run = next;
    }, 5400);
    // Returned, not `onCleanup(...)`: the compiler only resolves the lifecycle
    // macros at component top level. Nested inside this callback the name
    // survives into the bundle as a bare identifier and throws at mount, which
    // took the reactive bindings down with it and froze the diagram on its
    // server-rendered frame.
    return () => clearInterval(timer);
  });

  return (
    <div className="esr-scroller overflow-x-auto" data-lenis-prevent>
      <svg
        viewBox="0 0 960 620"
        role="img"
        aria-label="JavaScript running in a V8 isolate calls out through the op boundary into Rust, where each capability has a slot in a wall. The slots the command line granted are open and those calls reach the host resource on the right; the rest are closed and those calls stop at the wall with NotAllowedError before any effect, so their resource is shown greyed with no line to it. The command line and the grants it implies change every few seconds."
        className="h-auto w-full min-w-[860px]"
        style="font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"
      >
        <defs>
          {/* Lucide-style 24×24 outlines, with no stroke or fill of their own:
              each <use> supplies both, so one definition serves both themes. */}
          <g id="i-globe">
            <circle cx="12" cy="12" r="10" />
            <path d="M2 12h20" />
            <path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" />
          </g>
          <g id="i-folder">
            <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" />
          </g>
          <g id="i-file">
            <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" />
            <path d="M14 2v5h5" />
            <path d="M9 13h6" />
            <path d="M9 17h6" />
          </g>
          <g id="i-login">
            <path d="M15 3h4a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2h-4" />
            <path d="M10 17l5-5-5-5" />
            <path d="M15 12H3" />
          </g>
          <g id="i-server">
            <rect x="2" y="3" width="20" height="8" rx="2" />
            <rect x="2" y="13" width="20" height="8" rx="2" />
            <path d="M6 7h.01" />
            <path d="M6 17h.01" />
          </g>
          <g id="i-pencil">
            <path d="M12 20h9" />
            <path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z" />
          </g>
          <g id="i-key">
            <path d="M21 2l-2 2" />
            <path d="M12.4 11.6a5.5 5.5 0 1 1-7.8 7.8 5.5 5.5 0 0 1 7.8-7.8z" />
            <path d="M12.4 11.6L15.5 8.5l3 3L22 8l-3.5-3.5" />
          </g>
          <g id="i-terminal">
            <path d="M4 17l6-6-6-6" />
            <path d="M12 19h8" />
          </g>
          <g id="i-unlocked">
            <rect x="3" y="11" width="18" height="11" rx="2" />
            <path d="M7 11V7a5 5 0 0 1 9.9-1" />
          </g>
          <g id="i-locked">
            <rect x="3" y="11" width="18" height="11" rx="2" />
            <path d="M7 11V7a5 5 0 0 1 10 0v4" />
          </g>
          {/* The Rust mark: a toothed ring around an R. Drawn rather than
              linked, so it takes the theme's colour like everything else. */}
          <g id="i-rust">
            <circle cx="12" cy="12" r="8.4" />
            <circle cx="12" cy="12" r="5.2" />
            <path d="M20.4 12H23M12 3.6V1M12 20.4V23M3.6 12H1" />
            <path d="M18.35 5.65l1.85-1.85M5.65 5.65L3.8 3.8M18.35 18.35l1.85 1.85M5.65 18.35L3.8 20.2" />
          </g>
        </defs>

        {/* ---- band headers: what each side of the boundary actually is ----- */}
        {/* The outline mark only: this panel is dark in both site themes, so
            there is no light variant to swap to. */}
        <image href="/img/v8-outline.svg" x="89" y="18" width="26" height="26" />
        <use
          href="#i-rust"
          transform={at(MID, 31, 26)}
          fill="none"
          stroke-width="1.5"
          className="esr-rust"
        />
        <text
          x={MID}
          y="31"
          font-size="9"
          font-weight="700"
          text-anchor="middle"
          dominant-baseline="central"
          className="esr-rust-text"
        >
          R
        </text>
        <use
          href="#i-server"
          transform={at(780, 31, 24)}
          fill="none"
          stroke-width="1.6"
          className="stroke-zinc-500"
        />

        <g
          className="fill-zinc-500"
          font-size="11"
          letter-spacing="1.4"
          text-anchor="middle"
          dominant-baseline="central"
        >
          <text x="102" y="62">V8 ISOLATE</text>
          <text x={MID} y="62">OP BOUNDARY · RUST</text>
          <text x="780" y="62">THE HOST</text>
        </g>
        <line
          x1="32"
          y1="80"
          x2="928"
          y2="80"
          className="stroke-zinc-800"
          stroke-width="1"
        />

        {/* ---- call sites, inside the isolate -------------------------------- */}
        <g
          className="fill-zinc-300"
          font-size="13"
          text-anchor="start"
          dominant-baseline="central"
        >
          <text x="32" y={L.net}>fetch(url)</text>
          <text x="32" y={L.read}>file(path).text()</text>
          <text x="32" y={L.listen}>serve(opts, fn)</text>
          <text x="32" y={L.write}>write(path, bytes)</text>
          <text x="32" y={L.env}>env.DATABASE_URL</text>
          <text x="32" y={L.run}>new Command("sh")</text>
        </g>

        {/* ---- every call reaches the wall ----------------------------------- */}
        <g className="stroke-zinc-700" stroke-width="1.5">
          <line x1="200" y1={L.net} x2={WALL_X} y2={L.net} />
          <line x1="200" y1={L.read} x2={WALL_X} y2={L.read} />
          <line x1="200" y1={L.listen} x2={WALL_X} y2={L.listen} />
          <line x1="200" y1={L.write} x2={WALL_X} y2={L.write} />
          <line x1="200" y1={L.env} x2={WALL_X} y2={L.env} />
          <line x1="200" y1={L.run} x2={WALL_X} y2={L.run} />
        </g>

        {/* ---- the wall ------------------------------------------------------ */}
        <rect
          x={WALL_X}
          y="94"
          width={WALL_W}
          height="422"
          rx="14"
          className="fill-zinc-800 stroke-zinc-700"
          stroke-width="1"
        />

        {/* ---- the slots, and everything that follows from their state -------
            An open slot is a hole filled with the card's own background; a
            closed one is a plug, darker than the wall around it. */}
        <g stroke-width="1.5">
          <rect
            x={SLOT_X}
            y={L.net - 24}
            width={SLOT_W}
            height="48"
            rx="9"
            className={has(run, "net") ? OPEN_SLOT : SHUT_SLOT}
          />
          <rect
            x={SLOT_X}
            y={L.read - 24}
            width={SLOT_W}
            height="48"
            rx="9"
            className={has(run, "read") ? OPEN_SLOT : SHUT_SLOT}
          />
          <rect
            x={SLOT_X}
            y={L.listen - 24}
            width={SLOT_W}
            height="48"
            rx="9"
            className={has(run, "listen") ? OPEN_SLOT : SHUT_SLOT}
          />
          <rect
            x={SLOT_X}
            y={L.write - 24}
            width={SLOT_W}
            height="48"
            rx="9"
            className={has(run, "write") ? OPEN_SLOT : SHUT_SLOT}
          />
          <rect
            x={SLOT_X}
            y={L.env - 24}
            width={SLOT_W}
            height="48"
            rx="9"
            className={has(run, "env") ? OPEN_SLOT : SHUT_SLOT}
          />
          <rect
            x={SLOT_X}
            y={L.run - 24}
            width={SLOT_W}
            height="48"
            rx="9"
            className={has(run, "run") ? OPEN_SLOT : SHUT_SLOT}
          />
        </g>

        {/* Drawn here, between the slots and their labels, so a pulse crossing an
            open slot passes *under* the capability name and icon instead of
            sitting on top of them. */}
        <g className={has(run, "net") ? PASS_PULSE : BLOCK_PULSE} style="animation-delay: 0s">
          <circle cx="0" cy={L.net} r="4.5" />
        </g>
        <g className={has(run, "read") ? PASS_PULSE : BLOCK_PULSE} style="animation-delay: 0.5s">
          <circle cx="0" cy={L.read} r="4.5" />
        </g>
        <g className={has(run, "listen") ? PASS_PULSE : BLOCK_PULSE} style="animation-delay: 1s">
          <circle cx="0" cy={L.listen} r="4.5" />
        </g>
        <g className={has(run, "write") ? PASS_PULSE : BLOCK_PULSE} style="animation-delay: 1.5s">
          <circle cx="0" cy={L.write} r="4.5" />
        </g>
        <g className={has(run, "env") ? PASS_PULSE : BLOCK_PULSE} style="animation-delay: 2s">
          <circle cx="0" cy={L.env} r="4.5" />
        </g>
        <g className={has(run, "run") ? PASS_PULSE : BLOCK_PULSE} style="animation-delay: 2.5s">
          <circle cx="0" cy={L.run} r="4.5" />
        </g>

        {/* Capability icon, and the padlock that says whether the slot is open. */}
        <g fill="none" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <use href="#i-globe" transform={at(ICON, L.net, 18)} className={has(run, "net") ? OPEN_INK : SHUT_INK} />
          <use href="#i-folder" transform={at(ICON, L.read, 18)} className={has(run, "read") ? OPEN_INK : SHUT_INK} />
          <use href="#i-login" transform={at(ICON, L.listen, 18)} className={has(run, "listen") ? OPEN_INK : SHUT_INK} />
          <use href="#i-pencil" transform={at(ICON, L.write, 18)} className={has(run, "write") ? OPEN_INK : SHUT_INK} />
          <use href="#i-key" transform={at(ICON, L.env, 18)} className={has(run, "env") ? OPEN_INK : SHUT_INK} />
          <use href="#i-terminal" transform={at(ICON, L.run, 18)} className={has(run, "run") ? OPEN_INK : SHUT_INK} />

          <use href={has(run, "net") ? "#i-unlocked" : "#i-locked"} transform={at(LOCK, L.net, 15)} className={has(run, "net") ? OPEN_INK : SHUT_INK} />
          <use href={has(run, "read") ? "#i-unlocked" : "#i-locked"} transform={at(LOCK, L.read, 15)} className={has(run, "read") ? OPEN_INK : SHUT_INK} />
          <use href={has(run, "listen") ? "#i-unlocked" : "#i-locked"} transform={at(LOCK, L.listen, 15)} className={has(run, "listen") ? OPEN_INK : SHUT_INK} />
          <use href={has(run, "write") ? "#i-unlocked" : "#i-locked"} transform={at(LOCK, L.write, 15)} className={has(run, "write") ? OPEN_INK : SHUT_INK} />
          <use href={has(run, "env") ? "#i-unlocked" : "#i-locked"} transform={at(LOCK, L.env, 15)} className={has(run, "env") ? OPEN_INK : SHUT_INK} />
          <use href={has(run, "run") ? "#i-unlocked" : "#i-locked"} transform={at(LOCK, L.run, 15)} className={has(run, "run") ? OPEN_INK : SHUT_INK} />
        </g>

        <g font-size="15" text-anchor="start" dominant-baseline="central">
          <text x={NAME} y={L.net} className={has(run, "net") ? OPEN_TEXT : SHUT_TEXT}>net</text>
          <text x={NAME} y={L.read} className={has(run, "read") ? OPEN_TEXT : SHUT_TEXT}>read</text>
          <text x={NAME} y={L.listen} className={has(run, "listen") ? OPEN_TEXT : SHUT_TEXT}>listen</text>
          <text x={NAME} y={L.write} className={has(run, "write") ? OPEN_TEXT : SHUT_TEXT}>write</text>
          <text x={NAME} y={L.env} className={has(run, "env") ? OPEN_TEXT : SHUT_TEXT}>env</text>
          <text x={NAME} y={L.run} className={has(run, "run") ? OPEN_TEXT : SHUT_TEXT}>run</text>
        </g>

        {/* ---- only a granted lane continues, and only it reaches anything ---- */}
        <g className="stroke-brand-500" stroke-width="1.5">
          {has(run, "net") && <line x1={WALL_X + WALL_W} y1={L.net} x2="700" y2={L.net} />}
          {has(run, "read") && <line x1={WALL_X + WALL_W} y1={L.read} x2="700" y2={L.read} />}
          {has(run, "listen") && <line x1={WALL_X + WALL_W} y1={L.listen} x2="700" y2={L.listen} />}
          {has(run, "write") && <line x1={WALL_X + WALL_W} y1={L.write} x2="700" y2={L.write} />}
          {has(run, "env") && <line x1={WALL_X + WALL_W} y1={L.env} x2="700" y2={L.env} />}
          {has(run, "run") && <line x1={WALL_X + WALL_W} y1={L.run} x2="700" y2={L.run} />}
        </g>

        <g>
          <circle cx={HOST_ICON} cy={L.net} r="14" className={has(run, "net") ? HOST.net[0] : HOST_OFF_CHIP} />
          <circle cx={HOST_ICON} cy={L.read} r="14" className={has(run, "read") ? HOST.read[0] : HOST_OFF_CHIP} />
          <circle cx={HOST_ICON} cy={L.listen} r="14" className={has(run, "listen") ? HOST.listen[0] : HOST_OFF_CHIP} />
          <circle cx={HOST_ICON} cy={L.write} r="14" className={has(run, "write") ? HOST.write[0] : HOST_OFF_CHIP} />
          <circle cx={HOST_ICON} cy={L.env} r="14" className={has(run, "env") ? HOST.env[0] : HOST_OFF_CHIP} />
          <circle cx={HOST_ICON} cy={L.run} r="14" className={has(run, "run") ? HOST.run[0] : HOST_OFF_CHIP} />
        </g>
        <g fill="none" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
          <use href="#i-globe" transform={at(HOST_ICON, L.net, 19)} className={has(run, "net") ? HOST.net[1] : HOST_OFF_INK} />
          <use href="#i-file" transform={at(HOST_ICON, L.read, 19)} className={has(run, "read") ? HOST.read[1] : HOST_OFF_INK} />
          <use href="#i-server" transform={at(HOST_ICON, L.listen, 19)} className={has(run, "listen") ? HOST.listen[1] : HOST_OFF_INK} />
          <use href="#i-file" transform={at(HOST_ICON, L.write, 19)} className={has(run, "write") ? HOST.write[1] : HOST_OFF_INK} />
          <use href="#i-key" transform={at(HOST_ICON, L.env, 19)} className={has(run, "env") ? HOST.env[1] : HOST_OFF_INK} />
          <use href="#i-terminal" transform={at(HOST_ICON, L.run, 19)} className={has(run, "run") ? HOST.run[1] : HOST_OFF_INK} />
        </g>
        <g font-size="13" text-anchor="start" dominant-baseline="central">
          <text x={HOST_TEXT} y={L.net} className={has(run, "net") ? HOST_ON_TEXT : HOST_OFF_TEXT}>api.example.com</text>
          <text x={HOST_TEXT} y={L.read} className={has(run, "read") ? HOST_ON_TEXT : HOST_OFF_TEXT}>./config.json</text>
          <text x={HOST_TEXT} y={L.listen} className={has(run, "listen") ? HOST_ON_TEXT : HOST_OFF_TEXT}>:8080</text>
          <text x={HOST_TEXT} y={L.write} className={has(run, "write") ? HOST_ON_TEXT : HOST_OFF_TEXT}>./out/report.csv</text>
          <text x={HOST_TEXT} y={L.env} className={has(run, "env") ? HOST_ON_TEXT : HOST_OFF_TEXT}>DATABASE_URL</text>
          <text x={HOST_TEXT} y={L.run} className={has(run, "run") ? HOST_ON_TEXT : HOST_OFF_TEXT}>/bin/sh</text>
        </g>

        {/* ---- refusal marks, flashing as a blocked pulse arrives ------------- */}
        <g stroke-width="2.2" stroke-linecap="round" className="stroke-red-400">
          {!has(run, "net") && (
            <g className="esr-deny" style="animation-delay: 0s">
              <line x1="367" y1={L.net - 7} x2="381" y2={L.net + 7} />
              <line x1="381" y1={L.net - 7} x2="367" y2={L.net + 7} />
            </g>
          )}
          {!has(run, "read") && (
            <g className="esr-deny" style="animation-delay: 0.5s">
              <line x1="367" y1={L.read - 7} x2="381" y2={L.read + 7} />
              <line x1="381" y1={L.read - 7} x2="367" y2={L.read + 7} />
            </g>
          )}
          {!has(run, "listen") && (
            <g className="esr-deny" style="animation-delay: 1s">
              <line x1="367" y1={L.listen - 7} x2="381" y2={L.listen + 7} />
              <line x1="381" y1={L.listen - 7} x2="367" y2={L.listen + 7} />
            </g>
          )}
          {!has(run, "write") && (
            <g className="esr-deny" style="animation-delay: 1.5s">
              <line x1="367" y1={L.write - 7} x2="381" y2={L.write + 7} />
              <line x1="381" y1={L.write - 7} x2="367" y2={L.write + 7} />
            </g>
          )}
          {!has(run, "env") && (
            <g className="esr-deny" style="animation-delay: 2s">
              <line x1="367" y1={L.env - 7} x2="381" y2={L.env + 7} />
              <line x1="381" y1={L.env - 7} x2="367" y2={L.env + 7} />
            </g>
          )}
          {!has(run, "run") && (
            <g className="esr-deny" style="animation-delay: 2.5s">
              <line x1="367" y1={L.run - 7} x2="381" y2={L.run + 7} />
              <line x1="381" y1={L.run - 7} x2="367" y2={L.run + 7} />
            </g>
          )}
        </g>

        {/* ---- the command line the wall is rendered from ---------------------- */}
        <line
          x1={MID}
          y1="516"
          x2={MID}
          y2="548"
          className="stroke-zinc-700"
          stroke-width="1.5"
        />
        <rect
          x="242"
          y="548"
          width="520"
          height="44"
          rx="10"
          className="fill-zinc-800/60 stroke-zinc-700"
          stroke-width="1"
        />
        <text
          x={MID}
          y="570"
          font-size="13"
          text-anchor="middle"
          dominant-baseline="central"
          className="fill-zinc-300"
        >
          {RUNS[run].cmd}
        </text>
      </svg>
    </div>
  );
}
