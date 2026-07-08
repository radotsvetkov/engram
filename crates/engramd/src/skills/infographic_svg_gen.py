#!/usr/bin/env python3
"""infographic_svg_gen — Engram skill (no network). Generate a real,
self-contained SVG infographic: a title header followed by one horizontal
bar per stat, each bar's length proportional to its value relative to the
max value in the list (scaled to fit a fixed-width plot area). Bars cycle
through a small fixed color palette. All label/unit text is XML-escaped
before being interpolated, and the generated SVG is verified by parsing it
with xml.etree.ElementTree before being returned.

Request (stdin): {
  "title": "Q3 Growth",
  "stats": [
    {"label": "Users", "value": 12000, "unit": ""},
    {"label": "Revenue", "value": 45000, "unit": "$"}
  ]
}
Output (stdout): {svg, stat_count}
"""
import json
import sys
import xml.etree.ElementTree as ET
from xml.sax.saxutils import escape

_PALETTE = ["#4C6EF5", "#12B886", "#F59F00", "#E64980", "#7048E8", "#1098AD"]

_WIDTH = 640
_LABEL_X = 20
_BAR_X = 190
_MAX_BAR_WIDTH = 340
_ROW_HEIGHT = 60
_HEADER_HEIGHT = 80
_BOTTOM_PAD = 20
_BAR_HEIGHT = 28

_EXAMPLE = {
    "title": "Q3 Growth",
    "stats": [
        {"label": "Users", "value": 12000, "unit": ""},
        {"label": "Revenue", "value": 45000, "unit": "$"},
    ],
}


def _esc(value):
    return escape(str(value))


def _fmt_value(v):
    if isinstance(v, float) and v.is_integer():
        return str(int(v))
    return str(v)


def _build_svg(title, stats):
    n = len(stats)
    height = _HEADER_HEIGHT + n * _ROW_HEIGHT + _BOTTOM_PAD
    values = [s["value"] for s in stats]
    max_value = max(values) if values else 0

    parts = []
    parts.append(
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 %d %d" '
        'width="%d" height="%d" font-family="Helvetica, Arial, sans-serif">'
        % (_WIDTH, height, _WIDTH, height)
    )
    parts.append('<rect x="0" y="0" width="%d" height="%d" fill="#ffffff"/>' % (_WIDTH, height))
    parts.append(
        '<text x="20" y="36" font-size="24" font-weight="bold" fill="#1a1a1a">%s</text>'
        % _esc(title)
    )
    parts.append(
        '<line x1="20" y1="52" x2="%d" y2="52" stroke="#e0e0e0" stroke-width="1"/>' % (_WIDTH - 20)
    )

    for i, stat in enumerate(stats):
        label = stat["label"]
        value = stat["value"]
        unit = stat.get("unit") or ""
        color = _PALETTE[i % len(_PALETTE)]
        row_y = _HEADER_HEIGHT + i * _ROW_HEIGHT
        bar_y = row_y + (_ROW_HEIGHT - _BAR_HEIGHT) / 2
        bar_w = (value / max_value * _MAX_BAR_WIDTH) if max_value > 0 else 0
        bar_w = max(bar_w, 1 if value > 0 else 0)
        text_y = row_y + _ROW_HEIGHT / 2 + 5

        parts.append(
            '<text x="%d" y="%.1f" font-size="14" fill="#333333">%s</text>'
            % (_LABEL_X, text_y, _esc(label))
        )
        parts.append(
            '<rect x="%d" y="%.1f" width="%.2f" height="%d" fill="%s" rx="3"/>'
            % (_BAR_X, bar_y, bar_w, _BAR_HEIGHT, color)
        )
        value_text = "%s%s" % (_fmt_value(value), unit)
        parts.append(
            '<text x="%.1f" y="%.1f" font-size="13" fill="#1a1a1a">%s</text>'
            % (_BAR_X + bar_w + 10, text_y, _esc(value_text))
        )

    parts.append("</svg>")
    return "".join(parts)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE}))
        return 0

    title = q.get("title")
    if not isinstance(title, str) or not title.strip():
        print(json.dumps({"error": "missing required field 'title' (non-empty string)", "example": _EXAMPLE}))
        return 0

    stats = q.get("stats")
    if not isinstance(stats, list) or len(stats) == 0:
        print(json.dumps({"error": "'stats' must be a non-empty list", "example": _EXAMPLE}))
        return 0

    validated = []
    for i, s in enumerate(stats):
        if not isinstance(s, dict):
            print(json.dumps({"error": "stat at index %d must be a JSON object" % i, "example": _EXAMPLE}))
            return 0
        label = s.get("label")
        if not isinstance(label, str) or not label.strip():
            print(json.dumps({"error": "stat at index %d missing non-empty 'label'" % i, "example": _EXAMPLE}))
            return 0
        value = s.get("value")
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            print(json.dumps({
                "error": "stat at index %d has non-numeric 'value': %r" % (i, value),
                "example": _EXAMPLE,
            }))
            return 0
        if value < 0:
            print(json.dumps({
                "error": "stat at index %d has negative 'value'; only non-negative values are supported" % i,
            }))
            return 0
        unit = s.get("unit", "")
        if unit is None:
            unit = ""
        if not isinstance(unit, str):
            print(json.dumps({"error": "stat at index %d has non-string 'unit'" % i}))
            return 0
        validated.append({"label": label, "value": value, "unit": unit})

    try:
        svg = _build_svg(title, validated)
        ET.fromstring(svg)  # verify well-formed XML before returning
    except Exception as e:
        print(json.dumps({"error": "internal error building SVG: %s" % e}))
        return 1

    print(json.dumps({"svg": svg, "stat_count": len(validated)}, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
