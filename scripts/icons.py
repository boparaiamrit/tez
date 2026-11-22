"""Draw tez's icons: a violet tile with a white lightning bolt ("tez" means fast).

Writes into app/assets/icon/:
  tez.ico               Windows (exe, window, taskbar, Alt-Tab), 16 to 256 px
  tez.png               Linux desktop entry, 512 px
  tez.icns              macOS app, on Apple's icon grid (inset tile with a soft shadow)
  tray.rgba             32x32 raw RGBA for the Windows and Linux tray
  tray-template.rgba    32x32 raw RGBA for the macOS menu bar: the bolt alone, which macOS tints

Everything is drawn at 4x and scaled down, so edges are smooth. Small sizes (32 px and under) use a
bigger bolt and a full-bleed tile so they stay readable in the taskbar and tray.

Run: python scripts/icons.py   (needs Pillow and numpy)
"""
import os

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

OUT = os.path.join(os.path.dirname(__file__), "..", "app", "assets", "icon")
SS = 4  # supersampling

TOP_LEFT = np.array([142, 112, 255], dtype=np.float32)      # light violet
BOTTOM_RIGHT = np.array([84, 56, 226], dtype=np.float32)    # deep indigo-violet

# The bolt on a 32-unit grid (the same shape the tray has always used, a little bolder).
BOLT = [(17.5, 3.5), (8.0, 18.0), (15.0, 18.0), (12.5, 28.5), (24.0, 13.0), (17.0, 13.0), (22.0, 3.5)]


def bolt_points(cx, cy, size, k):
    """The bolt centred on (cx, cy), `size` being the tile it sits on, `k` its scale."""
    return [(cx + (x - 16.0) / 32.0 * size * k, cy + (y - 16.0) / 32.0 * size * k) for x, y in BOLT]


def tile(px_size, mac=False):
    w = px_size * SS
    small = px_size <= 32
    if mac:
        inset = round(w * 100 / 1024)               # Apple's grid: an 824/1024 tile
    else:
        inset = 0 if small else round(w * 0.03)
    body = w - 2 * inset
    radius = body * (0.2237 if mac else (0.18 if small else 0.22))

    # Diagonal gradient, top-left to bottom-right, plus a soft sheen across the top.
    yy, xx = np.mgrid[0:w, 0:w].astype(np.float32)
    t = np.clip(((xx - inset) + (yy - inset)) / (2.0 * body), 0.0, 1.0)[..., None]
    rgb = TOP_LEFT * (1 - t) + BOTTOM_RIGHT * t
    sheen = np.clip(1.0 - (yy - inset) / (0.55 * body), 0.0, 1.0)[..., None] * 0.16
    rgb = rgb * (1 - sheen) + 255.0 * sheen
    img = Image.fromarray(np.dstack([rgb, np.full((w, w, 1), 255.0)]).astype(np.uint8), "RGBA")

    mask = Image.new("L", (w, w), 0)
    ImageDraw.Draw(mask).rounded_rectangle([inset, inset, w - inset - 1, w - inset - 1], radius=radius, fill=255)

    # The bolt's shadow, inside the tile only.
    k = 0.92 if small else 0.74
    pts = bolt_points(w / 2, w / 2, body, k)
    shadow = Image.new("L", (w, w), 0)
    off = body * 0.018
    ImageDraw.Draw(shadow).polygon([(x, y + off) for x, y in pts], fill=110 if not small else 70)
    shadow = shadow.filter(ImageFilter.GaussianBlur(body * (0.025 if not small else 0.015)))
    dark = Image.new("RGBA", (w, w), (40, 20, 110, 255))
    img = Image.composite(dark, img, shadow)

    bolt = Image.new("L", (w, w), 0)
    ImageDraw.Draw(bolt).polygon(pts, fill=255)
    img = Image.composite(Image.new("RGBA", (w, w), (255, 255, 255, 255)), img, bolt)
    img.putalpha(mask)

    if mac:
        # A soft drop shadow under the tile, as macOS icons have.
        sh = Image.new("RGBA", (w, w), (0, 0, 0, 0))
        sm = mask.filter(ImageFilter.GaussianBlur(w * 0.022)).point(lambda a: a * 0.32)
        sh.putalpha(sm)
        sh = sh.transform(sh.size, Image.Transform.AFFINE, (1, 0, 0, 0, 1, -round(w * 0.012)))
        img = Image.alpha_composite(sh, img)
    return img.resize((px_size, px_size), Image.Resampling.LANCZOS)


def template(px_size):
    """The bolt alone in black: macOS draws template images in the menu bar's own colour."""
    w = px_size * SS
    img = Image.new("RGBA", (w, w), (0, 0, 0, 0))
    m = Image.new("L", (w, w), 0)
    ImageDraw.Draw(m).polygon(bolt_points(w / 2, w / 2, w, 1.12), fill=255)
    img.putalpha(m)
    return img.resize((px_size, px_size), Image.Resampling.LANCZOS)


def main():
    os.makedirs(OUT, exist_ok=True)
    ico_sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    frames = [tile(s) for s in ico_sizes]
    frames[-1].save(os.path.join(OUT, "tez.ico"), sizes=[(s, s) for s in ico_sizes], append_images=frames[:-1])
    tile(512).save(os.path.join(OUT, "tez.png"))
    tile(1024, mac=True).save(os.path.join(OUT, "tez.icns"))
    with open(os.path.join(OUT, "tray.rgba"), "wb") as f:
        f.write(tile(32).tobytes())
    with open(os.path.join(OUT, "tray-template.rgba"), "wb") as f:
        f.write(template(32).tobytes())
    # A contact sheet to look at.
    sheet = Image.new("RGBA", (16 + 32 + 48 + 64 + 256 + 6 * 24, 300), (236, 236, 240, 255))
    x = 24
    for s in [16, 32, 48, 64, 256]:
        sheet.alpha_composite(tile(s), (x, 300 - s - 22))
        x += s + 24
    sheet.save(os.path.join(OUT, "..", "..", "..", "target", "icon-sheet.png"))
    print("icons written to", os.path.normpath(OUT))


if __name__ == "__main__":
    main()
