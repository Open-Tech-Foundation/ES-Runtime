"""Writes the benchmark inputs: a 12 MP photo-like JPEG and a 1280x800
screenshot-like PNG. Deterministic, so every machine measures the same bytes."""

import random
import sys

from PIL import Image, ImageDraw, ImageFilter

out = sys.argv[1]
rng = random.Random(159)

photo = Image.new("RGB", (4000, 3000))
draw = ImageDraw.Draw(photo)
for y in range(3000):
    draw.line([(0, y), (4000, y)], fill=(y * 255 // 3000, 120, 255 - y * 255 // 3000))
for _ in range(400):
    x, y, r = rng.randrange(4000), rng.randrange(3000), rng.randrange(20, 300)
    draw.ellipse([x - r, y - r, x + r, y + r], fill=tuple(rng.randrange(256) for _ in range(3)))
photo = photo.filter(ImageFilter.GaussianBlur(3))
noise = Image.effect_noise((4000, 3000), 12).convert("RGB")
photo = Image.blend(photo, noise, 0.08)
photo.save(f"{out}/photo.jpg", quality=90)

screen = Image.new("RGB", (1280, 800), (245, 246, 248))
draw = ImageDraw.Draw(screen)
for i in range(60):
    x, y = rng.randrange(1200), rng.randrange(760)
    draw.rectangle([x, y, x + rng.randrange(40, 300), y + rng.randrange(10, 60)], fill=tuple(rng.randrange(256) for _ in range(3)))
for i in range(200):
    draw.text((rng.randrange(1200), rng.randrange(780)), "runtime:images", fill=(30, 30, 30))
screen.save(f"{out}/screen.png")
