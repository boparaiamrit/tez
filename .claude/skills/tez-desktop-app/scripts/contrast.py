"""WCAG contrast of every text/background pair tez's UI uses, for both palettes in ui.rs.
Body text needs 4.5:1, large text and UI parts (icons, borders that carry meaning, chart lines) 3:1."""
import re
import sys

src = open(sys.argv[1], encoding="utf-8").read()


def palette(fn):
    body = src[src.index(f"pub fn {fn}() -> Pal {{"):]
    body = body[: body.index("\n    }\n")]
    out = {}
    for name, kind, hexv in re.findall(r"(\w+): (c|ca)\(0x([0-9a-fA-F]+)\)", body):
        v = int(hexv, 16)
        out[name] = (v >> 24 & 255, v >> 16 & 255, v >> 8 & 255, (v & 255) / 255) if kind == "ca" else (v >> 16 & 255, v >> 8 & 255, v & 255, 1.0)
    series = re.search(r"series: \[(.*?)\]", body).group(1)
    for i, h in enumerate(re.findall(r"0x([0-9a-fA-F]+)", series)):
        v = int(h, 16)
        out[f"series{i}"] = (v >> 16 & 255, v >> 8 & 255, v & 255, 1.0)
    out["white"] = (255, 255, 255, 1.0)
    return out


def over(fg, bg):
    a = fg[3]
    return tuple(round(fg[i] * a + bg[i] * (1 - a)) for i in range(3)) + (1.0,)


def lum(c):
    def ch(x):
        x /= 255
        return x / 12.92 if x <= 0.03928 else ((x + 0.055) / 1.055) ** 2.4
    return 0.2126 * ch(c[0]) + 0.7152 * ch(c[1]) + 0.0722 * ch(c[2])


def ratio(a, b):
    la, lb = lum(a), lum(b)
    return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)


# (foreground, background, background base for translucent backgrounds, need, where)
PAIRS = [
    ("text", "bg", None, 4.5, "body text on the panel"),
    ("text2", "bg", None, 4.5, "secondary text on the panel"),
    ("text3", "bg", None, 4.5, "muted text on the panel"),
    ("text3", "card", None, 4.5, "muted text in cards"),
    ("text2", "card", None, 4.5, "secondary text in cards"),
    ("text3", "side", None, 4.5, "sidebar captions"),
    ("text2", "side", None, 4.5, "sidebar icons/sub-items"),
    ("text", "surface3", None, 4.5, "selected sidebar row"),
    ("text3", "surface2", None, 4.5, "placeholder in fields/menus"),
    ("text2", "surface2", None, 4.5, "segmented control, idle"),
    ("bg", "accent", None, 4.5, "primary button label"),
    ("accent_text", "accent_soft", "card", 4.5, "accent pill/link on tint"),
    ("accent_text", "bg", None, 4.5, "links"),
    ("green", "green_soft", "card", 4.5, "Running pill"),
    ("amber", "amber_soft", "card", 4.5, "warning pill"),
    ("red", "red_soft", "card", 4.5, "error pill"),
    ("blue", "blue_soft", "card", 4.5, "info pill"),
    ("text2", "surface3", None, 4.5, "neutral pill"),
    ("amber", "card", None, 4.5, "amber numbers in cards"),
    ("red", "card", None, 4.5, "red numbers in cards"),
    ("green", "card", None, 4.5, "green text in cards"),
    ("white", "toggle", None, 3.0, "switch thumb on (UI part)"),
    ("line2", "card", None, 1.0, "(info) field border vs card"),
    ("series0", "card", None, 3.0, "chart: tez line"),
    ("text2", "card", None, 3.0, "chart: machine line"),
    ("series1", "card", None, 3.0, "chart series 2"),
    ("series2", "card", None, 3.0, "chart series 3"),
    ("series3", "card", None, 3.0, "chart series 4"),
]

bad = 0
for theme in ["dark", "light"]:
    pal = palette(theme)
    print(f"== {theme}")
    for fg, bg, base, need, where in PAIRS:
        b = pal[bg]
        if b[3] < 1:
            b = over(b, pal[base or "bg"])
        f = pal[fg]
        if f[3] < 1:
            f = over(f, b)
        r = ratio(f, b)
        flag = "OK  " if r >= need else "FAIL"
        if r < need:
            bad += 1
        print(f"  {flag} {r:5.2f}:1 (need {need}) {fg} on {bg}  - {where}")
print(f"{bad} failing pairs")
