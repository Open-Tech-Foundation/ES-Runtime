// Home-page benchmark roller: a 50vh container that scrolls the full set of
// micro-benchmark cards as a seamless vertical marquee (~3-5 visible at a time).
// The card set is rendered twice and the track scrolls up by exactly half its
// height (see .bench-roll-* in global.css), so the loop never jumps. Hover
// pauses it; reduced-motion turns it into a normal scroll.
//
// The req/s HTTP story is charted separately (RpsChart in the Benchmarks
// section tabs) as a fixed headline; the
// in-process `http` micro-metric is intentionally left out here — it measures the
// server together with the client, which is not the claim this belt implies.
//
// The set is a subset rather than every row in the data — the roller is a shop
// window, and a few rows do not belong in one (`http` for the reason above, and
// the narrower Web API and parser rows, which need their section's context to
// mean anything). Which rows those are is the harness's call, not this file's:
// bench/run.sh marks each row `card`, `chart` or `hidden`, and everything marked
// `card` shows up here in the order the run defines. Nothing to keep in step.
import BenchCard from "./BenchCard.jsx";
import { cardRows } from "../src/bench-rows.js";

const METRICS = cardRows();

export default function BenchRoller() {
  let paused = $state(false);

  return (
    <div>
      <div className="mb-3 flex items-center justify-between">
        <h2 className="text-sm font-semibold text-zinc-900 dark:text-zinc-100">
          Benchmarks
        </h2>
        <button
          type="button"
          aria-pressed={paused}
          aria-label={paused ? "Resume benchmark motion" : "Pause benchmark motion"}
          title={paused ? "Resume benchmark motion" : "Pause benchmark motion"}
          onclick={() => (paused = !paused)}
          className="bench-roll-toggle rounded-md p-2 text-zinc-700 hover:bg-zinc-100 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-brand-500 dark:text-zinc-300 dark:hover:bg-zinc-800"
        >
          <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
            {paused ? <path d="m8 5 12 7-12 7V5Z" /> : <path d="M8 5v14M16 5v14" />}
          </svg>
        </button>
      </div>
      <div className="bench-roll-container relative overflow-hidden" style={{ height: "50vh" }}>
        <div className={paused ? "bench-roll-track is-paused" : "bench-roll-track"}>
          {METRICS.map((m) => (
            <div className="pb-4">
              <BenchCard metric={m} />
            </div>
          ))}
          <div aria-hidden="true">
            {METRICS.map((m) => (
              <div className="pb-4">
                <BenchCard metric={m} />
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
