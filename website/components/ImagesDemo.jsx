// runtime:images at work on one photo: four chains, and for each the file
// esrun made from it. Nothing here is drawn or typed: every picture and every
// size comes from scripts/images-demo/build.mjs, through imagesDemoData.js.
//
// The code shown is the chain the script ran; `upload` is the source photo's
// bytes, `logo` an Image of the site's mark and `gif` the animation the script
// builds as input.
//
// Written the way CodeTabs is: the tabs and chips are lists whose items never
// change, and everything that does change is an expression reading the state
// directly, so the compiler re-renders it.

import DATA from "./imagesDemoData.js";

const TAB_BASE = "-mb-px whitespace-nowrap border-b-2 px-2.5 py-2 sm:px-4 text-[13px] font-semibold transition-colors ";
const CHIP_BASE = "rounded-full px-3 py-1 text-xs font-semibold ring-1 ring-inset transition-colors ";

const K = "text-brand-400";
const S = "text-emerald-300";
const F = "text-blue-300";
const N = "text-orange-300";

const TABS = [
  { id: "thumb", title: "Thumbnail" },
  { id: "watermark", title: "Watermark" },
  { id: "rotate", title: "Any angle" },
  { id: "animated", title: "Animation" },
];

const FORMATS = [
  { id: "jpeg", label: "JPEG", call: "jpeg()" },
  { id: "webp", label: "WebP", call: "webp()" },
  { id: "avif", label: "AVIF", call: "avif({ quality: 50 })" },
  { id: "jxl", label: "JPEG XL", call: "jxl()" },
];

// One decimal while it says something, none from 100 up.
const size = (bytes) => {
  const [value, unit] = bytes >= 1024 * 1024 ? [bytes / (1024 * 1024), "MB"] : [bytes / 1024, "KB"];
  return `${value >= 100 ? Math.round(value) : value.toFixed(1)} ${unit}`;
};

const facts = (out) => `${out.format} · ${out.width}×${out.height} · ${size(out.bytes)}`;

const ALT = {
  thumb: "The Cosmic Cliffs of the Carina Nebula, cropped to a square thumbnail.",
  watermark: "The square thumbnail with the ES Runtime mark in its lower-right corner.",
  rotate: "A square crop of the nebula rotated 12 degrees, its corners transparent.",
  animated: "An animation panning slowly along the nebula's cliffs and back.",
};

export default function ImagesDemo() {
  let tab = $state("thumb");
  let format = $state("avif");
  // Most browsers cannot show JPEG XL yet. Checked once: where it fails, the
  // WebP stands in and the caption says so, with the JPEG XL file's size.
  let jxlShown = $state(false);

  onMount(() => {
    const probe = new window.Image();
    probe.onload = () => (jxlShown = probe.naturalWidth > 0);
    probe.src = DATA.formats.jxl.src;
  });

  const current = () => {
    if (tab === "thumb") return DATA.formats[format];
    if (tab === "watermark") return DATA.watermark;
    if (tab === "rotate") return DATA.rotated;
    return DATA.animated.output;
  };
  const shown = () => (tab === "thumb" && format === "jxl" && !jxlShown ? DATA.formats.webp.src : current().src);

  return (
    <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_minmax(0,22rem)] lg:items-start">
      <div className="min-w-0">
        <div role="group" aria-label="Image examples" className="flex items-center gap-1 overflow-x-auto border-b border-zinc-800 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
          {TABS.map((t) => (
            <button
              type="button"
              aria-pressed={tab === t.id}
              onclick={() => (tab = t.id)}
              className={TAB_BASE + (tab === t.id ? "border-brand-500 text-white" : "border-transparent text-zinc-400 hover:text-zinc-200")}
            >
              {t.title}
            </button>
          ))}
        </div>

        <pre className="mt-4 overflow-x-auto whitespace-pre text-[13px] leading-relaxed text-zinc-300">
          <code className={tab === "thumb" ? "" : "hidden"}>
            <span className={K}>const</span> thumb = <span className={K}>new</span> <span className={F}>Image</span>(upload){"\n"}
            {"  "}.<span className={F}>resize</span>(<span className={N}>480</span>, <span className={N}>480</span>, {"{ "}fit: <span className={S}>"cover"</span>{" });\n\n"}
            <span className={K}>await</span> thumb.<span className={F}>{FORMATS.find((f) => f.id === format).call}</span>.<span className={F}>bytes</span>();
          </code>
          <code className={tab === "watermark" ? "" : "hidden"}>
            <span className={K}>await new</span> <span className={F}>Image</span>(upload){"\n"}
            {"  "}.<span className={F}>resize</span>(<span className={N}>480</span>, <span className={N}>480</span>, {"{ "}fit: <span className={S}>"cover"</span>{" })\n"}
            {"  "}.<span className={F}>composite</span>([{"{ "}input: logo, gravity: <span className={S}>"southeast"</span>{" }])\n"}
            {"  "}.<span className={F}>webp</span>(){"\n"}
            {"  "}.<span className={F}>bytes</span>();
          </code>
          <code className={tab === "rotate" ? "" : "hidden"}>
            <span className={K}>await new</span> <span className={F}>Image</span>(upload){"\n"}
            {"  "}.<span className={F}>resize</span>(<span className={N}>360</span>, <span className={N}>360</span>, {"{ "}fit: <span className={S}>"cover"</span>, position: <span className={S}>"west"</span>{" })\n"}
            {"  "}.<span className={F}>rotate</span>(<span className={N}>12</span>, {"{ "}background: <span className={S}>"#0000"</span>{" })\n"}
            {"  "}.<span className={F}>webp</span>(){"\n"}
            {"  "}.<span className={F}>bytes</span>();
          </code>
          <code className={tab === "animated" ? "" : "hidden"}>
            <span className={K}>await new</span> <span className={F}>Image</span>(gif, {"{ "}animated: <span className={N}>true</span>{" })\n"}
            {"  "}.<span className={F}>resize</span>(<span className={N}>320</span>){"\n"}
            {"  "}.<span className={F}>webp</span>({"{ "}quality: <span className={N}>60</span>{" })\n"}
            {"  "}.<span className={F}>bytes</span>();
          </code>
        </pre>

        <div className={"mt-5 flex flex-wrap gap-2 " + (tab === "thumb" ? "" : "hidden")}>
          {FORMATS.map((f) => (
            <button
              type="button"
              aria-pressed={format === f.id}
              onclick={() => (format = f.id)}
              className={
                CHIP_BASE +
                (format === f.id
                  ? "bg-brand-500/15 text-brand-300 ring-brand-500/60"
                  : "text-zinc-400 ring-zinc-700 hover:text-zinc-200 hover:ring-zinc-500")
              }
            >
              {f.label} <span className="font-mono font-normal text-zinc-500">{size(DATA.formats[f.id].bytes)}</span>
            </button>
          ))}
        </div>

        <p className="mt-5 text-sm leading-relaxed text-zinc-400">
          {tab === "thumb"
            ? `From a ${DATA.source.width}×${DATA.source.height} JPEG of ${size(DATA.source.bytes)}: one chain, and the format is one method.`
            : tab === "watermark"
              ? "An overlay is any image or another chain, placed by gravity or by offset, with sharp's 25 blend modes."
              : tab === "rotate"
                ? "Any angle, onto the rotated image's bounding box. Multiples of 90 stay exact quarter turns."
                : `Every step runs on every frame: a ${DATA.animated.input.frames}-frame GIF of ${size(DATA.animated.input.bytes)} becomes an animated WebP that keeps all ${DATA.animated.output.frames}.`}
        </p>
      </div>

      <figure className="mx-auto w-full max-w-[22rem]">
        <div className={"esr-checker flex items-center " + (tab === "animated" ? "aspect-video" : "aspect-square") + " justify-center overflow-hidden rounded-xl ring-1 ring-white/10"}>
          <img
            src={shown()}
            alt={ALT[tab]}
            width={current().width}
            height={current().height}
            loading="lazy"
            decoding="async"
            className="max-h-full max-w-full object-contain"
          />
        </div>
        <figcaption className="mt-3 text-center font-mono text-xs text-zinc-400">
          {facts(current()) + (tab === "animated" ? ` · ${current().frames} frames` : "")}
          <span className={"mt-1 block font-sans text-zinc-500 " + (tab === "thumb" && format === "jxl" && !jxlShown ? "" : "hidden")}>
            This browser cannot show JPEG XL, so the WebP stands in.
          </span>
        </figcaption>
      </figure>
    </div>
  );
}
