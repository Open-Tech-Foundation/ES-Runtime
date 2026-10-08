// The cases every probe answers, one per process (see features.sh). A probe
// prints one JSON line: `{ "out": file }` for an image it wrote, `{ "value": x }`
// for a number it measured, or `{ "error": message, "code": code }`. features.py
// judges the answer, so no probe decides whether it passed.
export const FORMATS = ["jpeg", "png", "webp", "avif", "gif", "tiff"];
export const EXT = { jpeg: "jpg", png: "png", webp: "webp", avif: "avif", gif: "gif", tiff: "tiff" };

export async function probe(kase, data, out, api) {
  try {
    const [name, arg] = kase.split(":");
    const result = await run(name, arg, data, out, api);
    console.log(JSON.stringify(result));
  } catch (e) {
    console.log(JSON.stringify({ error: String(e?.message ?? e).slice(0, 200), code: e?.code ?? e?.name ?? null }));
  }
}

async function run(name, arg, data, out, api) {
  const write = async (bytes, ext = "png") => {
    const path = `${out}/${name}${arg ? "-" + arg : ""}.${ext}`;
    await api.save(path, bytes);
    return { out: path };
  };
  switch (name) {
    case "decode":
      return write(await api.decodeResize(await api.read(`${data}/sample.${EXT[arg]}`), 32));
    case "encode":
      return write(await api.encode(await api.read(`${data}/sample.png`), arg), EXT[arg]);
    case "fit":
      return write(await api.fit(await api.read(`${data}/stripes.png`), arg));
    case "crop":
      return write(await api.crop(await api.read(`${data}/sample.png`)));
    case "rotate":
      return write(await api.rotate(await api.read(`${data}/sample.png`)));
    case "rotate-free":
      return write(await api.rotateFree(await api.read(`${data}/sample.png`)));
    case "blur":
      return write(await api.blur(await api.read(`${data}/sample.png`)));
    case "composite":
      return write(await api.composite(await api.read(`${data}/sample.png`)));
    case "text":
      return write(await api.text(await api.read(`${data}/sample.png`)));
    case "animated-out":
      return write(await api.animatedGif(await api.read(`${data}/animated.gif`)), "gif");
    case "progressive-jpg":
      return write(await api.progressiveJpeg(await api.read(`${data}/sample.png`)), "jpg");
    case "orient":
      return write(await api.reencode(await api.read(`${data}/oriented.jpg`)));
    case "bomb":
      return write(await api.reencode(await api.read(`${data}/bomb.png`)));
    case "malformed":
      return write(await api.reencode(await api.read(`${data}/malformed.png`)));
    case "animation":
      return { value: await api.frames(await api.read(`${data}/animated.gif`)) };
    case "offthread": {
      // Timer ticks while eight large decodes run: zero means the loop was held.
      const bytes = await api.read(`${data}/photo.jpg`);
      let ticks = 0;
      const timer = setInterval(() => ticks++, 1);
      await Promise.all(Array.from({ length: 8 }, () => api.decodeResize(bytes, 400)));
      clearInterval(timer);
      return { value: ticks };
    }
  }
  throw new Error(`unknown case ${name}`);
}

export const unsupported = () => {
  throw Object.assign(new Error("no such operation"), { code: "UNSUPPORTED_BY_PROBE" });
};
