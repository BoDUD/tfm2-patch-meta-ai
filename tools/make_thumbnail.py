"""Draws the Workshop preview of Patch Meta AI (512x512).

    python tools/make_thumbnail.py                    package/thumbnail.png, the current look (classic)
    python tools/make_thumbnail.py --style options    three other looks and a sheet comparing them,
                                                      in dist/thumb_options/ (option_1..3.png, options.png)
    python tools/make_thumbnail.py --style 2 --out x.png   one of them

The looks: classic (win-rate chart, tier bars), 1 (draft board: both teams' picks around a win-chance
gauge), 2 (tier ladder on a light page over a dark title band), 3 (meta radar: a scope over the
meta with the best pick locked on). Everything is drawn here from shapes and text - no game art.

Needs Pillow and the DejaVu Sans fonts (any Linux) or the Windows fonts (C:/Windows/Fonts: Segoe UI,
else Arial).
"""

import argparse
import math
import os

from PIL import Image, ImageDraw, ImageFilter, ImageFont

W = H = 512
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FONT_DIRS = ["/usr/share/fonts/truetype/dejavu", "/usr/share/fonts/dejavu", "C:/Windows/Fonts"]


def font(dirs, names, size):
    for d in dirs:
        for n in names:
            path = os.path.join(d, n)
            if os.path.exists(path):
                return ImageFont.truetype(path, size)
    return ImageFont.load_default()


# ------------------------------------------------------------------------------------------ classic


def classic(bold, regular, out):
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

    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    img.save(out, optimize=True)
    print("wrote", out)


# ------------------------------------------------------------------- drawing kit for the options

K = 2  # drawn at twice the size and scaled down: smooth edges on every shape


class Fonts:
    """Fonts by role; sizes are in 512-pixel units."""

    def __init__(self, dirs):
        self.dirs = dirs

    def _get(self, names, size):
        return font(self.dirs, names, round(size * K))

    def black(self, size):
        return self._get(["seguibl.ttf", "DejaVuSans-Bold.ttf", "arialbd.ttf"], size)

    def bold(self, size):
        return self._get(["segoeuib.ttf", "DejaVuSans-Bold.ttf", "arialbd.ttf"], size)

    def regular(self, size):
        return self._get(["segoeui.ttf", "DejaVuSans.ttf", "arial.ttf"], size)

    def mono(self, size):
        return self._get(["consolab.ttf", "DejaVuSansMono-Bold.ttf", "courbd.ttf"], size)


class Pen:
    """ImageDraw in 512-pixel units."""

    def __init__(self, draw):
        self.d = draw

    @staticmethod
    def _b(box):
        return tuple(v * K for v in box)

    @staticmethod
    def _p(points):
        return [(x * K, y * K) for x, y in points]

    def rect(self, box, fill=None, outline=None, width=1):
        self.d.rectangle(self._b(box), fill=fill, outline=outline, width=round(width * K))

    def rrect(self, box, radius, fill=None, outline=None, width=1):
        self.d.rounded_rectangle(self._b(box), radius * K, fill=fill, outline=outline, width=round(width * K))

    def ellipse(self, box, fill=None, outline=None, width=1):
        self.d.ellipse(self._b(box), fill=fill, outline=outline, width=round(width * K))

    def circle(self, cx, cy, r, fill=None, outline=None, width=1):
        self.ellipse((cx - r, cy - r, cx + r, cy + r), fill, outline, width)

    def line(self, points, fill, width=1):
        self.d.line(self._p(points), fill=fill, width=round(width * K), joint="curve")

    def polygon(self, points, fill=None, outline=None):
        self.d.polygon(self._p(points), fill=fill, outline=outline)

    def arc(self, box, start, end, fill, width):
        self.d.arc(self._b(box), start, end, fill=fill, width=round(width * K))

    def pieslice(self, box, start, end, fill):
        self.d.pieslice(self._b(box), start, end, fill=fill)

    def text(self, xy, text, font, fill, anchor="la"):
        self.d.text((xy[0] * K, xy[1] * K), text, font=font, fill=fill, anchor=anchor)

    def width(self, text, font):
        return self.d.textlength(text, font=font) / K


class Canvas:
    def __init__(self, background):
        self.img = Image.new("RGB", (W * K, H * K), background)
        self.pen = Pen(ImageDraw.Draw(self.img, "RGBA"))

    def gradient(self, top, bottom, y0=0, y1=H):
        d = self.pen.d
        for y in range(y0 * K, y1 * K):
            t = (y - y0 * K) / max(1, (y1 - y0) * K - 1)
            d.line((0, y, W * K, y), fill=tuple(round(a + (b - a) * t) for a, b in zip(top, bottom)))

    def glow(self, draw, blur):
        """Shapes drawn by `draw(pen)` on a layer of their own, blurred, laid over the canvas."""
        layer = Image.new("RGBA", self.img.size, (0, 0, 0, 0))
        draw(Pen(ImageDraw.Draw(layer)))
        layer = layer.filter(ImageFilter.GaussianBlur(blur * K))
        self.img.paste(layer, (0, 0), layer)

    def done(self):
        return self.img.resize((W, H), Image.LANCZOS)


def fit(fonts_at, text, size, max_width, pen):
    """The largest font size up to `size` at which `text` fits in `max_width`."""
    while size > 8:
        f = fonts_at(size)
        if pen.width(text, f) <= max_width:
            return f
        size -= 1
    return fonts_at(size)


def glyph(pen, kind, cx, cy, r, fill):
    """A plain geometric mark standing for a champion (no game art)."""
    if kind == "tri":
        pen.polygon([(cx, cy - r), (cx + r * 0.9, cy + r * 0.62), (cx - r * 0.9, cy + r * 0.62)], fill)
    elif kind == "diamond":
        pen.polygon([(cx, cy - r), (cx + r * 0.72, cy), (cx, cy + r), (cx - r * 0.72, cy)], fill)
    elif kind == "hex":
        pen.polygon([(cx + r * math.cos(math.radians(30 + 60 * i)), cy + r * math.sin(math.radians(30 + 60 * i))) for i in range(6)], fill)
    elif kind == "star":
        pts = []
        for i in range(10):
            rr = r if i % 2 == 0 else r * 0.45
            a = math.radians(-90 + 36 * i)
            pts.append((cx + rr * math.cos(a), cy + rr * math.sin(a)))
        pen.polygon(pts, fill)
    elif kind == "ring":
        pen.circle(cx, cy, r * 0.8, outline=fill, width=max(2, r * 0.32))
    elif kind == "bolt":
        pen.polygon([(cx + r * 0.25, cy - r), (cx - r * 0.55, cy + r * 0.12), (cx - r * 0.02, cy + r * 0.12),
                     (cx - r * 0.25, cy + r), (cx + r * 0.55, cy - r * 0.12), (cx + r * 0.02, cy - r * 0.12)], fill)
    elif kind == "cross":
        a = r * 0.3
        pen.polygon([(cx - a, cy - r), (cx + a, cy - r), (cx + a, cy - a), (cx + r, cy - a), (cx + r, cy + a), (cx + a, cy + a),
                     (cx + a, cy + r), (cx - a, cy + r), (cx - a, cy + a), (cx - r, cy + a), (cx - r, cy - a), (cx - a, cy - a)], fill)
    elif kind == "chevron":
        pen.polygon([(cx - r, cy - r * 0.55), (cx, cy + r * 0.25), (cx + r, cy - r * 0.55), (cx + r, cy + r * 0.05),
                     (cx, cy + r * 0.85), (cx - r, cy + r * 0.05)], fill)
    else:  # square
        pen.rrect((cx - r * 0.7, cy - r * 0.7, cx + r * 0.7, cy + r * 0.7), r * 0.25, fill=fill)


def arrow(pen, cx, cy, size, up, fill):
    """A small solid triangle: up (buffed / rising) or down."""
    s = size
    if up:
        pen.polygon([(cx, cy - s * 0.6), (cx + s * 0.62, cy + s * 0.42), (cx - s * 0.62, cy + s * 0.42)], fill)
    else:
        pen.polygon([(cx, cy + s * 0.6), (cx + s * 0.62, cy - s * 0.42), (cx - s * 0.62, cy - s * 0.42)], fill)


def centered(pen, y, text, font, fill):
    pen.text((W / 2, y), text, font, fill, anchor="ma")


TEXT = (245, 247, 252)
BLUE = (91, 115, 255)
RED = (239, 100, 113)
GREEN = (76, 195, 138)
MINT = (88, 230, 160)
GOLD = (242, 193, 78)
TEAL = (55, 213, 179)
CORAL = (255, 122, 89)
SLATE = (138, 143, 163)
INK = (22, 23, 33)


# --------------------------------------------------------------------------- option 1: draft board


def option_1(F):
    """Both teams' picks around a win-chance gauge; the best pick for the turn lit."""
    c = Canvas((8, 11, 19))
    c.gradient((17, 23, 40), (6, 8, 14))
    c.glow(lambda p: p.ellipse((-200, 110, 170, 470), fill=BLUE + (95,)), 70)
    c.glow(lambda p: p.ellipse((342, 110, 712, 470), fill=RED + (95,)), 70)
    p = c.pen

    # title
    title_font = fit(F.black, "PATCH META AI", 58, 468, p)
    p.text((W / 2 + 2, 26 + 3), "PATCH META AI", title_font, (0, 0, 0, 140), anchor="ma")
    p.text((W / 2, 26), "PATCH META AI", title_font, TEXT, anchor="ma")

    # the two pick columns
    kinds_blue = ["hex", "bolt", "star", None, None]
    kinds_red = ["diamond", "tri", "cross", "ring", None]
    for i in range(5):
        y = 114 + i * 52
        for side, x0, color, kind in ((0, 22, BLUE, kinds_blue[i]), (1, 362, RED, kinds_red[i])):
            x1 = x0 + 128
            turn = side == 0 and i == 3
            if turn:
                c.glow(lambda q, y=y: q.rrect((x0 - 2, y - 2, x1 + 2, y + 46), 12, outline=MINT + (255,), width=5), 6)
            if kind is None and not turn:
                p.rrect((x0, y, x1, y + 44), 10, fill=(255, 255, 255, 10), outline=color + (70,), width=1)
                continue
            p.rrect((x0, y, x1, y + 44), 10, fill=(color[0] // 5, color[1] // 5, color[2] // 4, 235),
                    outline=(MINT + (255,)) if turn else (color + (150,)), width=2 if turn else 1)
            bar_x = x0 if side == 0 else x1 - 6
            p.rrect((bar_x, y, bar_x + 6, y + 44), 3, fill=color)
            tx = x0 + 30 if side == 0 else x1 - 30
            p.circle(tx, y + 22, 15, fill=(9, 12, 22), outline=color + (220,), width=2)
            if turn:
                glyph(p, "cross", tx, y + 22, 9, MINT)
                p.rrect((x0 + 52, y + 10, x1 - 8, y + 34), 8, fill=MINT)
                p.text(((x0 + 52 + x1 - 8) / 2, y + 22), "+4.2", F.black(15), INK, anchor="mm")
                continue
            glyph(p, kind, tx, y + 22, 9, TEXT)
            lx = x0 + 52 if side == 0 else x1 - 52 - [58, 44, 64, 50, 40][i]
            p.rrect((lx, y + 13, lx + [58, 44, 64, 50, 40][i], y + 21), 4, fill=(255, 255, 255, 150))
            sx = x0 + 52 if side == 0 else x1 - 52 - 34
            p.rrect((sx, y + 27, sx + 34, y + 32), 3, fill=(255, 255, 255, 60))

    # win-chance gauge
    cx, cy, r = 256, 262, 84
    share = 0.62
    box = (cx - r, cy - r, cx + r, cy + r)
    p.arc(box, 180, 360, (255, 255, 255, 28), 18)
    cut = 180 + 180 * share
    c.glow(lambda q: q.arc(box, 180, cut, BLUE + (200,), 18), 6)
    p.arc(box, 180, cut, BLUE, 16)
    p.arc(box, cut, 360, RED, 16)
    for k in range(11):  # ticks
        a = math.radians(180 + 18 * k)
        r0, r1 = r - 22, r - 15 if k % 5 else r - 27
        p.line([(cx + r0 * math.cos(a), cy + r0 * math.sin(a)), (cx + r1 * math.cos(a), cy + r1 * math.sin(a))],
               (255, 255, 255, 110), 2)
    a = math.radians(cut)
    p.line([(cx, cy), (cx + (r - 30) * math.cos(a), cy + (r - 30) * math.sin(a))], TEXT, 4)
    p.circle(cx, cy, 7, fill=TEXT)
    p.text((cx, cy + 14), "62%", F.black(44), TEXT, anchor="ma")
    p.text((cx, cy + 66), "WIN CHANCE", F.bold(13), (190, 200, 225), anchor="ma")

    # the advice under the gauge
    p.rrect((170, 356, 252, 382), 13, fill=MINT)
    p.text((211, 369), "PICK", F.black(14), INK, anchor="mm")
    p.rrect((260, 356, 342, 382), 13, outline=RED + (255,), width=2)
    p.text((301, 369), "BAN", F.black(14), RED, anchor="mm")

    sub_font = fit(F.bold, "DRAFT ADVISOR  \u2022  SMARTER BAN/PICK", 22, 470, p)
    centered(p, 408, "DRAFT ADVISOR  \u2022  SMARTER BAN/PICK", sub_font, MINT)
    centered(p, 446, "AUTO TIER LIST  \u2022  TEAMFIGHT MANAGER 2", F.regular(15), (190, 200, 225, 200))
    return c.done()


# --------------------------------------------------------------------------- option 2: tier ladder


def option_2(F):
    """A tier list on a light page, champions moving up and down, over a dark title band."""
    c = Canvas((243, 239, 231))
    p = c.pen
    for k in range(-W, W * 2, 26):  # faint diagonal ruling
        p.line([(k, 0), (k + 300, 330)], (60, 50, 30, 10), 8)

    tiers = [
        ("S", CORAL, ["star", "bolt"]),
        ("A", GOLD, ["hex", "diamond", "tri", "ring"]),
        ("B", BLUE, ["cross", "square", "chevron", "hex", "star"]),
        ("C", TEAL, ["ring", "tri", "diamond"]),
        ("D", SLATE, ["square", "cross"]),
    ]
    moves = {(0, 1): True, (1, 2): True, (2, 0): False, (2, 4): True, (3, 1): False, (4, 0): False}
    y0, rh, gap = 24, 50, 9
    for i, (letter, color, kinds) in enumerate(tiers):
        y = y0 + i * (rh + gap)
        p.rrect((26, y + 3, 486, y + rh + 3), 13, fill=(0, 0, 0, 22))  # shadow
        p.rrect((26, y, 486, y + rh), 13, fill=(255, 255, 255, 235), outline=(0, 0, 0, 25), width=1)
        p.rrect((26, y, 26 + 62, y + rh), 13, fill=color)
        p.rect((26 + 48, y, 26 + 62, y + rh), fill=color)
        p.text((26 + 31, y + rh / 2), letter, F.black(32), INK if letter != "D" else (250, 250, 250), anchor="mm")
        for j, kind in enumerate(kinds):
            tx, ty = 26 + 62 + 34 + j * 54, y + rh / 2
            p.circle(tx, ty, 19, fill=INK)
            glyph(p, kind, tx, ty, 10, color if letter != "D" else (205, 208, 220))
            if (i, j) in moves:
                up = moves[(i, j)]
                bx, by = tx + 15, ty - 14
                p.circle(bx, by, 8, fill=(255, 255, 255), outline=(0, 0, 0, 30), width=1)
                arrow(p, bx, by + (0.5 if up else -0.5), 8, up, GREEN if up else RED)
    # the patch stamp, at the end of the S row
    p.rrect((384, y0 + 9, 476, y0 + rh - 9), 16, fill=INK)
    p.text((430, y0 + rh / 2), "NEW PATCH", F.black(12), GOLD, anchor="mm")

    # title band
    p.rect((0, 330, W, H), fill=INK)
    seg = W / 5
    for i, (_, color, _) in enumerate(tiers):
        p.rect((i * seg, 330, (i + 1) * seg, 337), fill=color)
    title_font = fit(F.black, "PATCH META AI", 60, 470, p)
    centered(p, 352, "PATCH META AI", title_font, (247, 243, 234))
    sub_font = fit(F.bold, "AUTO TIER LIST  \u2022  SMARTER BAN/PICK", 21, 460, p)
    centered(p, 428, "AUTO TIER LIST  \u2022  SMARTER BAN/PICK", sub_font, GOLD)
    centered(p, 466, "Teamfight Manager 2", F.regular(15), (163, 169, 182))
    return c.done()


# --------------------------------------------------------------------------- option 3: meta radar


def option_3(F):
    """A scope over the meta: champions as blips, the strong ones linked, the best pick locked on."""
    c = Canvas((4, 6, 11))
    c.gradient((7, 12, 20), (3, 4, 8))
    c.glow(lambda q: q.ellipse((36, -10, 476, 420), fill=TEAL + (60,)), 80)
    p = c.pen
    cx, cy, R = 256, 196, 150

    # rings, cross hairs, ticks
    for k, r in enumerate((R, R * 0.75, R * 0.5, R * 0.25)):
        p.circle(cx, cy, r, outline=TEAL + (110 if k == 0 else 55,), width=2 if k == 0 else 1.5)
    p.line([(cx - R, cy), (cx + R, cy)], TEAL + (45,), 1.5)
    p.line([(cx, cy - R), (cx, cy + R)], TEAL + (45,), 1.5)
    for k in range(72):
        a = math.radians(k * 5)
        r0 = R + 4
        r1 = R + (13 if k % 9 == 0 else 8)
        p.line([(cx + r0 * math.cos(a), cy + r0 * math.sin(a)), (cx + r1 * math.cos(a), cy + r1 * math.sin(a))],
               TEAL + (130 if k % 9 == 0 else 60,), 1.5)

    # the sweep, fading behind the beam (angles run clockwise from 3 o'clock)
    head = 322
    for k in range(75):
        p.pieslice((cx - R, cy - R, cx + R, cy + R), head - k - 1, head - k, TEAL + (round(70 * (1 - k / 75)),))
    a = math.radians(head)
    p.line([(cx, cy), (cx + R * math.cos(a), cy + R * math.sin(a))], TEAL + (230,), 2.5)

    # blips: (radius share, angle, strong)
    blips = [(0.86, 200, False), (0.62, 160, True), (0.35, 120, False), (0.78, 95, True), (0.5, 60, False),
             (0.9, 25, False), (0.3, 300, True), (0.7, 250, False), (0.45, 210, False), (0.92, 140, False),
             (0.2, 30, False), (0.58, 5, True)]
    pos = [(cx + f * R * math.cos(math.radians(t)), cy + f * R * math.sin(math.radians(t)), s) for f, t, s in blips]
    target = (cx + 0.66 * R * math.cos(math.radians(-62)), cy + 0.66 * R * math.sin(math.radians(-62)))
    strong = [(x, y) for x, y, s in pos if s]
    for x, y in strong:  # the strong ones linked to the pick
        p.line([(x, y), target], TEAL + (70,), 1.5)
    for (x0, y0), (x1, y1) in zip(strong, strong[1:]):
        p.line([(x0, y0), (x1, y1)], TEAL + (45,), 1.5)
    c.glow(lambda q: [q.circle(x, y, 9, fill=TEAL + (200,)) for x, y in strong], 5)
    for x, y, s in pos:
        p.circle(x, y, 5.5 if s else 3.5, fill=(TEAL if s else (120, 150, 160)) + (255,))

    # the lock-on
    tx, ty = target
    c.glow(lambda q: q.circle(tx, ty, 14, outline=(170, 255, 230, 255), width=5), 6)
    p.circle(tx, ty, 13, outline=(200, 255, 240), width=2.5)
    p.circle(tx, ty, 5, fill=(230, 255, 248))
    b = 24
    for sx, sy in ((-1, -1), (1, -1), (-1, 1), (1, 1)):
        x0, y0 = tx + sx * b, ty + sy * b
        p.line([(x0, y0 - sy * 9), (x0, y0), (x0 - sx * 9, y0)], (200, 255, 240), 2.5)
    p.rrect((tx + 32, ty - 34, tx + 32 + 96, ty - 6), 7, fill=(6, 30, 26, 235), outline=TEAL + (255,), width=1.5)
    p.text((tx + 32 + 48, ty - 20), "BEST +5.3", F.mono(14), (200, 255, 240), anchor="mm")

    # title with a glow
    title_font = fit(F.black, "PATCH META AI", 58, 470, p)
    c.glow(lambda q: q.text((W / 2, 372), "PATCH META AI", title_font, TEAL + (220,), anchor="ma"), 9)
    centered(p, 372, "PATCH META AI", title_font, TEXT)
    sub_font = fit(F.bold, "SMARTER BAN/PICK  \u2022  AUTO TIER LIST", 20, 460, p)
    centered(p, 440, "SMARTER BAN/PICK  \u2022  AUTO TIER LIST", sub_font, TEAL)
    centered(p, 474, "Teamfight Manager 2", F.regular(15), (150, 175, 180))
    return c.done()


OPTIONS = {
    "1": ("Draft board", option_1),
    "2": ("Tier ladder", option_2),
    "3": ("Meta radar", option_3),
}


def contact_sheet(images, names, F):
    """The options side by side at full size, and below each at the sizes the Workshop lists them."""
    pad, label_h = 24, 40
    sheet = Image.new("RGB", (len(images) * (W + pad) + pad, pad + H + label_h + 128 + pad * 2), (40, 43, 52))
    d = ImageDraw.Draw(sheet)
    label = font(F.dirs, ["segoeuib.ttf", "DejaVuSans-Bold.ttf", "arialbd.ttf"], 20)
    small = font(F.dirs, ["segoeui.ttf", "DejaVuSans.ttf", "arial.ttf"], 14)
    for i, (img, name) in enumerate(zip(images, names)):
        x = pad + i * (W + pad)
        sheet.paste(img, (x, pad))
        d.text((x, pad + H + 10), f"Option {i + 1}: {name}", font=label, fill=(235, 238, 245))
        y = pad + H + label_h + pad // 2
        sheet.paste(img.resize((128, 128), Image.LANCZOS), (x, y))
        sheet.paste(img.resize((64, 64), Image.LANCZOS), (x + 128 + 16, y + 64))
        d.text((x + 128 + 16, y + 40), "128 / 64 px", font=small, fill=(170, 176, 190))
    return sheet


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--font-dir", action="append", default=list(FONT_DIRS))
    ap.add_argument("--style", default="classic", choices=["classic", "1", "2", "3", "options"])
    ap.add_argument("--out", help="the image to write (classic: package/thumbnail.png)")
    ap.add_argument("--out-dir", default=os.path.join(ROOT, "dist", "thumb_options"), help="--style options: where to write")
    args = ap.parse_args()

    if args.style == "classic":
        bold = lambda s: font(args.font_dir, ["DejaVuSans-Bold.ttf", "arialbd.ttf"], s)
        regular = lambda s: font(args.font_dir, ["DejaVuSans.ttf", "arial.ttf"], s)
        classic(bold, regular, args.out or os.path.join(ROOT, "package", "thumbnail.png"))
        return

    F = Fonts(args.font_dir)
    if args.style == "options":
        os.makedirs(args.out_dir, exist_ok=True)
        images, names = [], []
        for key, (name, draw) in OPTIONS.items():
            img = draw(F)
            path = os.path.join(args.out_dir, f"option_{key}.png")
            img.save(path, optimize=True)
            print("wrote", path)
            images.append(img)
            names.append(name)
        path = os.path.join(args.out_dir, "options.png")
        contact_sheet(images, names, F).save(path, optimize=True)
        print("wrote", path)
        return

    name, draw = OPTIONS[args.style]
    out = args.out or os.path.join(args.out_dir, f"option_{args.style}.png")
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    draw(F).save(out, optimize=True)
    print("wrote", out)


if __name__ == "__main__":
    main()
