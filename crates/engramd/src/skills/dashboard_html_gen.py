#!/usr/bin/env python3
"""dashboard_html_gen — Engram skill (no network). Generate a real,
self-contained HTML dashboard page: a header, a responsive CSS-grid of
stat tiles (one per metric), and an inline SVG horizontal bar chart
comparing all the metrics' values. All CSS is inlined in a single <style>
tag (no external JS/CSS/font dependencies), all text is escaped (html.escape
for HTML, xml.sax.saxutils.escape for the embedded SVG), and the generated
document is sanity-checked for balanced html/head/body tags before being
returned.

Request (stdin): {
  "title": "Ops Dashboard",
  "metrics": [
    {"label": "Uptime", "value": 99.9, "unit": "%"},
    {"label": "Requests/s", "value": 1200, "unit": ""}
  ]
}
Output (stdout): {html, metric_count}
"""
import html
import json
import re
import sys
from xml.sax.saxutils import escape as xml_escape

_PALETTE = ["#4C6EF5", "#12B886", "#F59F00", "#E64980", "#7048E8", "#1098AD"]

_EXAMPLE = {
    "title": "Ops Dashboard",
    "metrics": [
        {"label": "Uptime", "value": 99.9, "unit": "%"},
        {"label": "Requests/s", "value": 1200, "unit": ""},
    ],
}

_CSS = """
:root {
  color-scheme: light;
  --bg: #f5f6f8;
  --card-bg: #ffffff;
  --border: #e2e4e9;
  --text: #1a1a1a;
  --muted: #666b76;
}
* { box-sizing: border-box; }
body {
  margin: 0;
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
  background: var(--bg);
  color: var(--text);
}
header {
  padding: 24px 32px;
  background: var(--card-bg);
  border-bottom: 1px solid var(--border);
}
header h1 { margin: 0; font-size: 22px; }
main { padding: 24px 32px; }
.grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
  gap: 16px;
  margin-bottom: 32px;
}
.card {
  background: var(--card-bg);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 18px 20px;
  box-shadow: 0 1px 3px rgba(0,0,0,0.06);
}
.card .label { font-size: 13px; color: var(--muted); margin-bottom: 6px; }
.card .value { font-size: 28px; font-weight: 600; }
.chart-section {
  background: var(--card-bg);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 20px;
}
.chart-section h2 { margin: 0 0 12px 0; font-size: 16px; color: var(--muted); }
svg { max-width: 100%; height: auto; display: block; }
"""


def _fmt_value(v):
    if isinstance(v, float) and v.is_integer():
        return str(int(v))
    return str(v)


def _build_tiles(metrics):
    out = []
    for m in metrics:
        label = html.escape(str(m["label"]))
        value_text = html.escape("%s%s" % (_fmt_value(m["value"]), m.get("unit") or ""))
        out.append(
            '<div class="card"><div class="label">%s</div><div class="value">%s</div></div>'
            % (label, value_text)
        )
    return "\n".join(out)


def _build_bar_chart_svg(metrics):
    width = 640
    label_x = 20
    bar_x = 190
    max_bar_width = 340
    row_height = 44
    top_pad = 10
    bottom_pad = 10
    bar_height = 22

    values = [m["value"] for m in metrics]
    max_value = max(values) if values else 0
    height = top_pad + len(metrics) * row_height + bottom_pad

    parts = []
    parts.append(
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 %d %d" width="%d" height="%d" '
        'font-family="Helvetica, Arial, sans-serif">' % (width, height, width, height)
    )
    for i, m in enumerate(metrics):
        label = m["label"]
        value = m["value"]
        unit = m.get("unit") or ""
        color = _PALETTE[i % len(_PALETTE)]
        row_y = top_pad + i * row_height
        bar_y = row_y + (row_height - bar_height) / 2
        bar_w = (value / max_value * max_bar_width) if max_value > 0 else 0
        bar_w = max(bar_w, 1 if value > 0 else 0)
        text_y = row_y + row_height / 2 + 5

        parts.append(
            '<text x="%d" y="%.1f" font-size="13" fill="#333333">%s</text>'
            % (label_x, text_y, xml_escape(str(label)))
        )
        parts.append(
            '<rect x="%d" y="%.1f" width="%.2f" height="%d" fill="%s" rx="3"/>'
            % (bar_x, bar_y, bar_w, bar_height, color)
        )
        value_text = "%s%s" % (_fmt_value(value), unit)
        parts.append(
            '<text x="%.1f" y="%.1f" font-size="12" fill="#1a1a1a">%s</text>'
            % (bar_x + bar_w + 8, text_y, xml_escape(value_text))
        )
    parts.append("</svg>")
    return "".join(parts)


def _build_html(title, metrics):
    esc_title = html.escape(title)
    tiles = _build_tiles(metrics)
    chart_svg = _build_bar_chart_svg(metrics)
    return """<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>%s</title>
<style>%s</style>
</head>
<body>
<header><h1>%s</h1></header>
<main>
<section class="grid">
%s
</section>
<section class="chart-section">
<h2>Metric Comparison</h2>
%s
</section>
</main>
</body>
</html>
""" % (esc_title, _CSS, esc_title, tiles, chart_svg)


def _tags_balanced(doc):
    # Match "<head" only when followed by '>' or whitespace, so "<header>"
    # (which also starts with the substring "<head") is not miscounted as
    # an open <head> tag.
    for tag in ("html", "head", "body"):
        opens = len(re.findall(r"<%s[\s>]" % tag, doc))
        closes = doc.count("</%s>" % tag)
        if opens == 0 or opens != closes:
            return False
    return True


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

    metrics = q.get("metrics")
    if not isinstance(metrics, list) or len(metrics) == 0:
        print(json.dumps({"error": "'metrics' must be a non-empty list", "example": _EXAMPLE}))
        return 0

    validated = []
    for i, m in enumerate(metrics):
        if not isinstance(m, dict):
            print(json.dumps({"error": "metric at index %d must be a JSON object" % i, "example": _EXAMPLE}))
            return 0
        label = m.get("label")
        if not isinstance(label, str) or not label.strip():
            print(json.dumps({"error": "metric at index %d missing non-empty 'label'" % i, "example": _EXAMPLE}))
            return 0
        value = m.get("value")
        if isinstance(value, bool) or not isinstance(value, (int, float)):
            print(json.dumps({
                "error": "metric at index %d has non-numeric 'value': %r" % (i, value),
                "example": _EXAMPLE,
            }))
            return 0
        if value < 0:
            print(json.dumps({
                "error": "metric at index %d has negative 'value'; only non-negative values are supported" % i,
            }))
            return 0
        unit = m.get("unit", "")
        if unit is None:
            unit = ""
        if not isinstance(unit, str):
            print(json.dumps({"error": "metric at index %d has non-string 'unit'" % i}))
            return 0
        validated.append({"label": label, "value": value, "unit": unit})

    try:
        doc = _build_html(title, validated)
        if not _tags_balanced(doc):
            raise ValueError("generated HTML has unbalanced html/head/body tags")
    except Exception as e:
        print(json.dumps({"error": "internal error building HTML: %s" % e}))
        return 1

    print(json.dumps({"html": doc, "metric_count": len(validated)}, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
