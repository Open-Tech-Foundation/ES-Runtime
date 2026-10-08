"""Image features, by running each runtime rather than by reading its docs.

Runs every case in cases.mjs on esrun (runtime:images), Node.js (sharp), Bun
(Bun.Image) and Deno (createImageBitmap + OffscreenCanvas), one process per
case, and judges what each wrote with Pillow: sizes, pixels and the magic bytes
of an encoded file. Prints the comparison table as the site renders it.

    python3 bench/images/features/features.py     (needs Pillow; sharp via pnpm install)
"""

import json
import os
import shutil
import struct
import subprocess
import sys
import zlib

from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
SAMPLES = os.path.join(HERE, ".samples")
OUT = os.path.join(HERE, ".out")
ESRUN = os.environ.get("ESRUN", os.path.join(ROOT, "target", "release", "esrun"))
FORMATS = ["jpeg", "png", "webp", "avif", "gif", "tiff"]
EXT = {"jpeg": "jpg", "png": "png", "webp": "webp", "avif": "avif", "gif": "gif", "tiff": "tiff"}

RUNTIMES = {
    "esrun": [ESRUN, "--allow-read", "--allow-write", "--allow-imports", "esrun.mjs"],
    "node": ["node", "node.mjs"],
    "bun": ["bun", "bun.mjs"],
    "deno": ["deno", "run", "--quiet", "--allow-read", "--allow-write", "deno.mjs"],
}


def chunk(kind, body):
    return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))


def samples():
    if os.path.exists(os.path.join(SAMPLES, "photo.jpg")):
        return
    os.makedirs(SAMPLES, exist_ok=True)
    s = os.path.join
    img = Image.new("RGB", (64, 48))
    img.putdata([(x * 4, y * 5, 128) for y in range(48) for x in range(64)])
    img.save(s(SAMPLES, "sample.png"))
    img.save(s(SAMPLES, "sample.jpg"), quality=90)
    img.save(s(SAMPLES, "sample.webp"), quality=90)
    img.save(s(SAMPLES, "sample.gif"))
    img.save(s(SAMPLES, "sample.tiff"))
    # Pillow writes no AVIF; sharp does, so none of the runtimes made its own input.
    subprocess.run(
        ["node", "-e", "require('sharp')(process.argv[1]).avif().toFile(process.argv[2])",
         s(SAMPLES, "sample.png"), s(SAMPLES, "sample.avif")],
        cwd=os.path.join(ROOT, "bench"), check=True,
    )
    stripes = Image.new("RGB", (200, 100), (255, 255, 255))
    stripes.paste((255, 0, 0), (0, 0, 50, 100))
    stripes.save(s(SAMPLES, "stripes.png"))
    fixtures = os.path.join(ROOT, "crates", "runtime-cli", "tests", "fixtures", "images")
    shutil.copy(os.path.join(fixtures, "oriented.jpg"), s(SAMPLES, "oriented.jpg"))
    shutil.copy(os.path.join(fixtures, "animated.gif"), s(SAMPLES, "animated.gif"))
    # A valid 20000x20000 PNG of black pixels: 400 MP, over every runtime's
    # 268 MP default, about a megabyte on disk and 1.2 GB once decoded. Valid,
    # so a runtime with no limit decodes it rather than failing on bad data.
    side = 20000
    z = zlib.compressobj(9)
    row = b"\0" * (1 + side * 3)
    idat = b"".join(z.compress(row) for _ in range(side)) + z.flush()
    ihdr = struct.pack(">IIBBBBB", side, side, 8, 2, 0, 0, 0)
    bomb = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")
    open(s(SAMPLES, "bomb.png"), "wb").write(bomb)
    open(s(SAMPLES, "malformed.png"), "wb").write(b"\x89PNG\r\n\x1a\n" + b"not an image" * 4)
    photo = os.path.join(HERE, "..", ".data", "photo.jpg")
    if not os.path.exists(photo):
        subprocess.run([sys.executable, "-I", os.path.join(HERE, "..", "gen.py"), os.path.dirname(photo)], check=True)
    shutil.copy(photo, s(SAMPLES, "photo.jpg"))


CASES = (
    [f"decode:{f}" for f in FORMATS]
    + [f"encode:{f}" for f in FORMATS]
    + ["fit:cover", "fit:contain", "fit:outside", "crop", "rotate", "rotate-free",
       "blur", "composite", "text", "animated-out", "progressive-jpg",
       "orient", "bomb", "malformed", "animation", "offthread"]
)

MAGIC = {
    "jpeg": lambda b: b[:3] == b"\xff\xd8\xff",
    "png": lambda b: b[:8] == b"\x89PNG\r\n\x1a\n",
    "webp": lambda b: b[:4] == b"RIFF" and b[8:12] == b"WEBP",
    "avif": lambda b: b[4:8] == b"ftyp" and b[8:12] in (b"avif", b"avis"),
    "gif": lambda b: b[:4] == b"GIF8",
    "tiff": lambda b: b[:4] in (b"II*\0", b"MM\0*"),
}


def size(result):
    with Image.open(result["out"]) as im:
        return im.size


def pixel(result, xy):
    with Image.open(result["out"]) as im:
        return im.convert("RGBA").getpixel(xy)


def judge(case, result):
    """True, False, or a short reason the cell is something else."""
    if result is None:
        return False
    name, _, arg = case.partition(":")
    if name == "bomb":
        return "error" in result
    if name == "malformed":
        return "error" in result and str(result.get("code") or "").startswith("ERR_")
    if name in ("animation",):
        return result.get("value") == 2
    if name == "offthread":
        return (result.get("value") or 0) > 0
    if "out" not in result:
        return False
    if name == "decode":
        return size(result) == (32, 24)
    if name == "encode":
        return MAGIC[arg](open(result["out"], "rb").read(16))
    if name == "fit":
        dims = size(result)
        if arg == "outside":
            return dims == (200, 100)
        if dims != (100, 100):
            return False
        if arg == "cover":  # the red quarter is cropped away
            return pixel(result, (2, 50))[:3] != (255, 0, 0)
        if arg == "contain":  # padded above and below, not stretched
            r, g, b, a = pixel(result, (50, 5))
            return a == 0 or (r, g, b) == (0, 0, 0)
    if name == "crop":
        return size(result) == (20, 20)
    if name == "rotate":
        return size(result) == (48, 64)
    if name == "rotate-free":
        # sample.png is 64x48: a 45-degree turn makes a larger square canvas.
        w, h = size(result)
        return w == h and (w, h) != (64, 48)
    if name == "text":
        # The overlay is black text on white. Every source pixel has blue 128,
        # so no source pixel has all three channels under 64.
        with Image.open(result["out"]) as im:
            return any(r < 64 and g < 64 and b < 64 for r, g, b in im.convert("RGB").getdata())
    if name == "animated-out":
        with Image.open(result["out"]) as im:
            return getattr(im, "n_frames", 1) == 2
    if name == "progressive-jpg":
        # A progressive JPEG carries a Start Of Frame marker for each scan
        # (SOF2, 0xFFC2); a baseline one carries none.
        return b"\xff\xc2" in open(result["out"], "rb").read()
    if name in ("blur", "composite"):
        return size(result) == (64, 48)
    if name == "orient":
        return size(result) == (48, 64)
    return False


def run(rt, case):
    out = os.path.join(OUT, rt)
    os.makedirs(out, exist_ok=True)
    try:
        proc = subprocess.run(RUNTIMES[rt] + [case, SAMPLES, out], cwd=HERE, capture_output=True, text=True, timeout=60)
    except subprocess.TimeoutExpired:
        return {"crash": "timed out"}
    lines = [l for l in proc.stdout.splitlines() if l.startswith("{")]
    if not lines:
        return {"crash": (proc.stderr or "no output").strip().splitlines()[-1][:160] if proc.stderr.strip() else "no output"}
    return json.loads(lines[-1])


ROWS = [
    ("Decode JPEG", ["decode:jpeg"]), ("Decode PNG", ["decode:png"]), ("Decode WebP", ["decode:webp"]),
    ("Decode AVIF", ["decode:avif"]), ("Decode GIF", ["decode:gif"]), ("Decode TIFF", ["decode:tiff"]),
    ("Encode JPEG", ["encode:jpeg"]), ("Encode PNG", ["encode:png"]), ("Encode WebP", ["encode:webp"]),
    ("Encode AVIF", ["encode:avif"]), ("Encode GIF", ["encode:gif"]), ("Encode TIFF", ["encode:tiff"]),
    ("Resize: `cover`", ["fit:cover"]), ("Resize: `contain`", ["fit:contain"]), ("Resize: `outside`", ["fit:outside"]),
    ("Crop", ["crop"]), ("Rotate 90°", ["rotate"]), ("Rotate 45° (any angle)", ["rotate-free"]),
    ("Blur", ["blur"]), ("Composite", ["composite"]), ("Text overlay", ["text"]),
    ("Animated output keeps frames", ["animated-out"]), ("Progressive JPEG", ["progressive-jpg"]),
    ("EXIF orientation applied by default", ["orient"]),
    ("Refuses a decompression bomb", ["bomb"]),
    ("Error code on malformed input", ["malformed"]),
    ("Reports animation frames", ["animation"]),
    ("Event loop runs while decoding", ["offthread"]),
]


def main():
    samples()
    shutil.rmtree(OUT, ignore_errors=True)
    present = [rt for rt in RUNTIMES if shutil.which(RUNTIMES[rt][0]) or os.path.exists(RUNTIMES[rt][0])]
    verdicts, raw = {}, {}
    for rt in present:
        for case in CASES:
            result = run(rt, case)
            raw[(rt, case)] = result
            verdicts[(rt, case)] = "crash" not in result and judge(case, result) is True
            print(f"{rt:6} {case:14} {'yes' if verdicts[(rt, case)] else 'no ':3} {json.dumps(result)[:110]}", file=sys.stderr)
    cols = [rt for rt in ["esrun", "node", "bun", "deno"] if rt in present]
    heads = {"esrun": "esrun<br/>`runtime:images`", "node": "Node.js<br/>sharp", "bun": "Bun<br/>`Bun.Image`",
             "deno": "Deno<br/>`createImageBitmap`"}
    print("| | " + " | ".join(heads[rt] for rt in cols) + " |")
    print("| --- |" + " :---: |" * len(cols))
    for label, cases in ROWS:
        cells = ["<Yes />" if all(verdicts[(rt, c)] for c in cases) else "<No />" for rt in cols]
        print(f"| {label} | " + " | ".join(cells) + " |")


main()
