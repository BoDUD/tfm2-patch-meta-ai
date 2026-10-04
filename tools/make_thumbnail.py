"""Draws package/thumbnail.png (512x512), the Workshop preview of Patch Meta AI.

    python tools/make_thumbnail.py

Needs Pillow and the DejaVu Sans fonts (any Linux; on Windows pass --font-dir C:/Windows/Fonts
and the script falls back to Arial).
"""

import argparse
import math
import os

from PIL import Image, ImageDraw, ImageFilter, ImageFont

W = H = 512
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def font(dirs, names, size):
    for d in dirs:
        for n in names:
            path = os.path.join(d, n)
            if os.path.exists(path):
                return ImageFont.truetype(path, size)
    return ImageFont.load_default()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--font-dir", action="append", default=[
        "/usr/share/fonts/truetype/dejavu", "/usr/share/fonts/dejavu", "C:/Windows/Fonts"])
    ap.add_argument("--out", default=os.path.join(ROOT, "package", "thumbnail.png"))
    args = ap.parse_args()
    bold = lambda s: font(args.font_dir, ["DejaVuSans-Bold.ttf", "arialbd.ttf"], s)
    regular = lambda s: font(args.font_dir, ["DejaVuSans.ttf", "arial.ttf"], s)

    img = Image.new("RGB", (W, H))
    px = img.load()
    for y in range(H):  # vertical gradient, slate blue to near black
        t = y / (H - 1)
        c = (int(22 - 12 * t), int(34 - 20 * t), int(54 - 30 * t))
        for x in range(W):
            px[x, y] = c
    d = ImageDraw.Draw(img, "RGBA")

    # chart panel
    px0, py0, px1, py1 = 36, 92, 330, 330
    d.rounded_rectangle((px0, py0, px1, py1), 18, fill=(255, 255, 255, 14), outline=(255, 255, 255, 40), width=2)
    for i in range(1, 4):  # grid
        y = py0 + (py1 - py0) * i / 4
        d.line((px0 + 14, y, px1 - 14, y), fill=(255, 255, 255, 22), width=1)
    mid = (py0 + py1) / 2
    for x in range(px0 + 16, px1 - 16, 14):  # dashed 50% line
        d.line((x, mid, x + 7, mid), fill=(255, 255, 255, 90), width=2)
    d.text((px1 - 52, mid - 22), "50%", font=regular(15), fill=(255, 255, 255, 140))

    def curve(points, color, glow):
        layer = Image.new("RGBA", (W, H), (0, 0, 0, 0))
        g = ImageDraw.Draw(layer)
        g.line(points, fill=glow, width=12, joint="curve")
        layer = layer.filter(ImageFilter.GaussianBlur(6))
        img.paste(layer, (0, 0), layer)
        d.line(points, fill=color, width=5, joint="curve")
        for p in points[::3]:
            d.ellipse((p[0] - 5, p[1] - 5, p[0] + 5, p[1] + 5), fill=color)

    xs = [px0 + 22 + i * (px1 - px0 - 44) / 12 for i in range(13)]
    rise = [mid + 28 - 120 * (1 - math.exp(-i / 5.0)) + 9 * math.sin(i * 1.7) for i in range(13)]
    fall = [mid - 10 + 85 * (1 - math.exp(-i / 6.0)) + 7 * math.sin(i * 2.3 + 1) for i in range(13)]
    curve(list(zip(xs, fall)), (255, 107, 107), (255, 80, 80, 120))
    curve(list(zip(xs, rise)), (88, 230, 160), (60, 220, 150, 140))
    d.polygon([(xs[-1] + 6, rise[-1] - 16), (xs[-1] + 20, rise[-1] - 2), (xs[-1] + 2, rise[-1] + 2)], fill=(88, 230, 160))

    # tier ladder
    tiers = [("S", (255, 196, 61)), ("A", (88, 230, 160)), ("B", (92, 170, 255)), ("C", (178, 140, 255)), ("D", (255, 107, 107))]
    lx, ly, size, gap = 360, 92, 40, 10
    for i, (letter, color) in enumerate(tiers):
        y = ly + i * (size + gap)
        d.rounded_rectangle((lx, y, lx + size, y + size), 9, fill=color)
        tw = d.textlength(letter, font=bold(26))
        d.text((lx + (size - tw) / 2, y + 4), letter, font=bold(26), fill=(14, 20, 30))
        bar = [96, 78, 62, 44, 26][i]
        d.rounded_rectangle((lx + size + 10, y + 12, lx + size + 10 + bar, y + size - 12), 6, fill=color + (150,))

    # patch tag
    d.rounded_rectangle((36, 34, 196, 70), 18, fill=(255, 196, 61))
    d.text((52, 40), "NEW PATCH", font=bold(21), fill=(20, 24, 34))
    d.text((210, 41), "real results, every patch", font=regular(19), fill=(220, 230, 245, 200))

    # title
    title = "PATCH META AI"
    f = bold(52)
    tw = d.textlength(title, font=f)
    d.text(((W - tw) / 2 + 3, 360 + 3), title, font=f, fill=(0, 0, 0, 120))
    d.text(((W - tw) / 2, 360), title, font=f, fill=(245, 248, 255))
    sub = "SMARTER BAN/PICK  \u2022  AUTO TIER LIST"
    fs = bold(21)
    sw = d.textlength(sub, font=fs)
    d.text(((W - sw) / 2, 428), sub, font=fs, fill=(88, 230, 160))
    foot = "Teamfight Manager 2"
    ff = regular(17)
    fw = d.textlength(foot, font=ff)
    d.text(((W - fw) / 2, 466), foot, font=ff, fill=(200, 210, 230, 170))

    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    img.save(args.out, optimize=True)
    print("wrote", args.out)


if __name__ == "__main__":
    main()
