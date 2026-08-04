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
// The list is explicit rather than every row in the data: the roller is a
// shop window, and a few rows do not belong in one (`http` for the reason above,
// `rss_load`, whose point is its memory rather than its time, and the paired
// `_large`/`_many` variants, which say the same thing twice at this size). Keep
// it in step with new rows by hand — `bench/run.sh --list` names them all.
import BenchCard from "./BenchCard.jsx";

const METRICS = [
  { key: "startup", label: "Cold start", unit: "ms" },
  { key: "rss", label: "Peak memory", unit: "MB" },
  { key: "crypto", label: "WebCrypto", unit: "ms" },
  { key: "sha256", label: "SubtleCrypto SHA-256", unit: "ms" },
  { key: "fetch", label: "fetch (local server)", unit: "ms" },
  { key: "timers", label: "setTimeout churn", unit: "ms" },
  { key: "streams", label: "ReadableStream piping", unit: "ms" },
  { key: "async", label: "async/await throughput", unit: "ms" },
  { key: "fsread_small", label: "File read (small)", unit: "ms" },
  { key: "fsread_large", label: "File read (large)", unit: "ms" },
  { key: "fswrite_small", label: "File write (small)", unit: "ms" },
  { key: "fswrite_large", label: "File write (large)", unit: "ms" },
  { key: "fsappend_small", label: "File append (small)", unit: "ms" },
  { key: "fsappend_large", label: "File append (large)", unit: "ms" },
  { key: "fsstat_small", label: "File stat (small)", unit: "ms" },
  { key: "fsexists_small", label: "File exists (small)", unit: "ms" },
  { key: "glob", label: "Glob scan", unit: "ms" },
  { key: "json", label: "JSON parse/stringify", unit: "ms" },
  { key: "jsonbig", label: "JSON (large documents)", unit: "ms" },
  { key: "url", label: "URL parsing", unit: "ms" },
  { key: "encoding", label: "TextEncoder/TextDecoder", unit: "ms" },
  { key: "base64", label: "base64 (atob/btoa)", unit: "ms" },
  { key: "structured", label: "structuredClone", unit: "ms" },
  { key: "compute", label: "Tight compute loop", unit: "ms" },
  { key: "bigscript", label: "Parse + run ~100 KB", unit: "ms" },
  { key: "modules", label: "Load a 300-module graph", unit: "ms" },
  { key: "regex", label: "Regex match/validate/replace", unit: "ms" },
  { key: "strings", label: "String building", unit: "ms" },
  { key: "errors", label: "throw/catch + stack", unit: "ms" },
  { key: "buffers", label: "TypedArray / DataView", unit: "ms" },
  { key: "headers", label: "Headers / Request", unit: "ms" },
  { key: "formdata", label: "Multipart FormData", unit: "ms" },
  { key: "date_intl", label: "Date + Intl formatting", unit: "ms" },
  { key: "crypto_asym", label: "ECDSA sign + verify", unit: "ms" },
  { key: "crypto_kdf", label: "PBKDF2", unit: "ms" },
  { key: "spawn", label: "Spawn a child process", unit: "ms" },
  { key: "fsstat_many", label: "File stat (1000 paths)", unit: "ms" },
  { key: "fsexists_many", label: "File exists (1000 paths)", unit: "ms" },
  { key: "rss_loaded", label: "Peak memory (under load)", unit: "MB" },
];

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
