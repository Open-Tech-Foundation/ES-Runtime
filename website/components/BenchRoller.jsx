// Home-page benchmark roller: a 50vh container that scrolls the full set of
// micro-benchmark cards as a seamless vertical marquee (~3-5 visible at a time).
// The card set is rendered twice and the track scrolls up by exactly half its
// height (see .bench-roll-* in global.css), so the loop never jumps. Hover
// pauses it; reduced-motion turns it into a normal scroll.
//
// The req/s HTTP story is shown separately (RpsChart) as a fixed headline; the
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
  return (
    <div className="bench-roll-container relative overflow-hidden" style={{ height: "50vh" }}>
      <div className="bench-roll-track">
        {METRICS.map((m) => (
          <div className="pb-4">
            <BenchCard metric={m} />
          </div>
        ))}
        {METRICS.map((m) => (
          <div className="pb-4">
            <BenchCard metric={m} />
          </div>
        ))}
      </div>
    </div>
  );
}
