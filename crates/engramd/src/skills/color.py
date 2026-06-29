#!/usr/bin/env python3
"""color — Engram skill (no network). Color math: hex/rgb -> hsl, WCAG luminance & contrast.

Request: {"color": "#3366ff" | "3366ff" | "#36f" | "rgb(51,102,255)"}.
Parses to (r,g,b) 0-255, then emits hex, rgb, hsl (h 0-360, s/l 0-100),
WCAG relative luminance, contrast ratios vs white/black, and the best
text color ("black" or "white") for that background.
"""
import json, sys, re


def parse_color(s):
    """Return (r, g, b) ints 0-255 or None if unparseable."""
    if not isinstance(s, str):
        return None
    t = s.strip().lower()

    # rgb(r, g, b) form (also accepts rgba(...) with ignored alpha)
    m = re.match(r"^rgba?\(\s*([^)]+)\)$", t)
    if m:
        parts = [p.strip() for p in m.group(1).replace("/", ",").split(",")]
        nums = []
        for p in parts[:3]:
            if p == "":
                return None
            try:
                if p.endswith("%"):
                    val = round(float(p[:-1]) * 255.0 / 100.0)
                else:
                    val = int(round(float(p)))
            except Exception:
                return None
            nums.append(val)
        if len(nums) < 3:
            return None
        if any(n < 0 or n > 255 for n in nums):
            return None
        return (nums[0], nums[1], nums[2])

    # hex form, with or without leading '#'
    h = t[1:] if t.startswith("#") else t
    if re.fullmatch(r"[0-9a-f]{3}", h):
        return (int(h[0] * 2, 16), int(h[1] * 2, 16), int(h[2] * 2, 16))
    if re.fullmatch(r"[0-9a-f]{4}", h):  # short with alpha; ignore alpha
        return (int(h[0] * 2, 16), int(h[1] * 2, 16), int(h[2] * 2, 16))
    if re.fullmatch(r"[0-9a-f]{6}", h):
        return (int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16))
    if re.fullmatch(r"[0-9a-f]{8}", h):  # with alpha; ignore alpha
        return (int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16))
    return None


def rgb_to_hsl(r, g, b):
    rf, gf, bf = r / 255.0, g / 255.0, b / 255.0
    mx, mn = max(rf, gf, bf), min(rf, gf, bf)
    l = (mx + mn) / 2.0
    if mx == mn:
        h = 0.0
        s = 0.0
    else:
        d = mx - mn
        s = d / (2.0 - mx - mn) if l > 0.5 else d / (mx + mn)
        if mx == rf:
            h = (gf - bf) / d + (6.0 if gf < bf else 0.0)
        elif mx == gf:
            h = (bf - rf) / d + 2.0
        else:
            h = (rf - gf) / d + 4.0
        h /= 6.0
    return [round(h * 360.0), round(s * 100.0), round(l * 100.0)]


def _lin(c):
    c = c / 255.0
    return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4


def relative_luminance(r, g, b):
    return 0.2126 * _lin(r) + 0.7152 * _lin(g) + 0.0722 * _lin(b)


def contrast_ratio(l1, l2):
    hi, lo = (l1, l2) if l1 >= l2 else (l2, l1)
    return (hi + 0.05) / (lo + 0.05)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    color = q.get("color") if isinstance(q, dict) else None
    if color is None or (isinstance(color, str) and color.strip() == ""):
        print(json.dumps({
            "error": "missing required field 'color'",
            "example": {"color": "#3366ff"},
        }))
        return 0

    try:
        rgb = parse_color(color)
        if rgb is None:
            print(json.dumps({"error": "could not parse color %s" % color}))
            return 0

        r, g, b = rgb
        lum = relative_luminance(r, g, b)
        lum_white = relative_luminance(255, 255, 255)  # = 1.0
        lum_black = relative_luminance(0, 0, 0)        # = 0.0
        cw = contrast_ratio(lum, lum_white)
        cb = contrast_ratio(lum, lum_black)
        best = "black" if cb >= cw else "white"

        result = {
            "hex": "#%02x%02x%02x" % (r, g, b),
            "rgb": [r, g, b],
            "hsl": rgb_to_hsl(r, g, b),
            "luminance": lum,
            "contrast_white": round(cw, 2),
            "contrast_black": round(cb, 2),
            "best_text": best,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "color failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
