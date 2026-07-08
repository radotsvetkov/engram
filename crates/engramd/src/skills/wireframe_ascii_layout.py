#!/usr/bin/env python3
"""wireframe_ascii_layout — Engram skill (no network).

Generates an ASCII-art text wireframe representing the standard structure of
a common page type (landing page, dashboard, form, or list view), scaled to
roughly the requested character width. Useful for sketching page layout in a
plain-text medium (chat, tickets, READMEs) before real mockups exist.

Request (stdin): {"layout_type": "landing_page"|"dashboard"|"form"|"list_view",
  "width"?: int (default 60)}
Output (stdout): {ascii_wireframe: str, layout_type: str}
"""
import json
import sys

LAYOUT_TYPES = ("landing_page", "dashboard", "form", "list_view")
MIN_WIDTH = 40
MAX_WIDTH = 120


def _bar(width, ch="-"):
    return "+" + ch * (width - 2) + "+"


def _row(width, text=""):
    inner = width - 4
    text = text[:inner]
    pad = inner - len(text)
    return "| " + text + " " * pad + " |"


def _blank_row(width):
    return _row(width, "")


def _split_evenly(total, n):
    """Split `total` chars into `n` column widths that sum back to `total`
    exactly (widths differ by at most 1) — avoids the classic
    integer-division remainder bug where columns don't add back up to the
    outer box width."""
    base, rem = divmod(total, n)
    return [base + (1 if i < rem else 0) for i in range(n)]


def _gen_landing_page(width):
    lines = []
    lines.append(_bar(width))
    lines.append(_row(width, "[LOGO]" + " " * 10 + "Nav: Home  Product  Pricing  Login"))
    lines.append(_bar(width))
    lines.append(_blank_row(width))
    lines.append(_row(width, "HERO HEADLINE: Your Product, Your Way".center(width - 4)))
    lines.append(_row(width, "Supporting subheadline goes here".center(width - 4)))
    lines.append(_blank_row(width))
    lines.append(_row(width, "[  Call To Action Button  ]".center(width - 4)))
    lines.append(_blank_row(width))
    lines.append(_bar(width))
    col_widths = _split_evenly(width - 4, 3)
    col_border = "+" + "+".join("-" * w for w in col_widths) + "+"
    lines.append(col_border)
    header = "|" + "|".join((" Feature %d" % (i + 1)).ljust(w) for i, w in enumerate(col_widths)) + "|"
    lines.append(header)
    body = "|" + "|".join(" icon + text".ljust(w) for w in col_widths) + "|"
    lines.append(body)
    lines.append(col_border)
    lines.append(_bar(width))
    lines.append(_row(width, "Footer: About | Contact | Terms | (c) Company".center(width - 4)))
    lines.append(_bar(width))
    return "\n".join(lines)


def _gen_dashboard(width):
    lines = []
    lines.append(_bar(width))
    lines.append(_row(width, "[LOGO]  Dashboard  Reports  Settings" + " " * 6 + "[user avatar]"))
    lines.append(_bar(width))

    # Two columns (sidebar | main), each row built to exactly `width` chars:
    # "|" + sidebar_inner + "|" + main_inner + "|" (three '|' chars total).
    sidebar_inner = max(12, width // 5)
    main_inner = width - sidebar_inner - 3
    sep_border = "+" + "-" * sidebar_inner + "+" + "-" * main_inner + "+"

    def _two_col(sidebar_text, main_text):
        return ("|" + sidebar_text[:sidebar_inner].ljust(sidebar_inner)
                + "|" + main_text[:main_inner].ljust(main_inner) + "|")

    lines.append(sep_border)

    sidebar_items = ["Overview", "Analytics", "Users", "Billing", "Settings"]

    # Row 0: KPI/stat tiles across the main area.
    n_tiles = 3
    gap = 2
    lead = 1
    tile_widths = _split_evenly(main_inner - lead - gap * (n_tiles - 1), n_tiles)
    tiles = ["[" + ("KPI %d" % (t + 1)).center(w - 2) + "]" for t, w in enumerate(tile_widths)]
    lines.append(_two_col(" " + sidebar_items[0], " " * lead + (" " * gap).join(tiles)))

    for item in sidebar_items[1:]:
        lines.append(_two_col(" " + item, ""))

    lines.append(sep_border)

    # Chart placeholder box filling the rest of the main area.
    chart_h = 5
    for i in range(chart_h):
        if i == 0 or i == chart_h - 1:
            main_text = _bar(main_inner)
        elif i == chart_h // 2:
            label = "Chart / graph placeholder"[:max(0, main_inner - 2)]
            main_text = "|" + label.center(main_inner - 2) + "|"
        else:
            main_text = "|" + " " * (main_inner - 2) + "|"
        lines.append(_two_col("", main_text))

    lines.append(sep_border)
    return "\n".join(lines)


def _gen_form(width):
    lines = []
    lines.append(_bar(width))
    lines.append(_row(width, "FORM TITLE".center(width - 4)))
    lines.append(_bar(width))
    lines.append(_blank_row(width))
    fields = ["Name:", "Email:", "Message:"]
    for f in fields:
        lines.append(_row(width, f))
        field_h = 3 if f == "Message:" else 1
        for _ in range(field_h):
            lines.append(_row(width, "[" + "_" * (width - 8) + "]"))
        lines.append(_blank_row(width))
    lines.append(_row(width, "[        Submit        ]".center(width - 4)))
    lines.append(_blank_row(width))
    lines.append(_bar(width))
    return "\n".join(lines)


def _gen_list_view(width, n_items=4):
    lines = []
    lines.append(_bar(width))
    lines.append(_row(width, "LIST TITLE"))
    lines.append(_row(width, "[ Search... ]   [Filter v]   [Sort v]"))
    lines.append(_bar(width))
    for i in range(n_items):
        lines.append(_row(width, "(o)  Item %d title" % (i + 1)))
        lines.append(_row(width, "     Secondary text / description" + " " * 6 + "[...]"))
        if i < n_items - 1:
            lines.append("|" + "-" * (width - 2) + "|")
    lines.append(_bar(width))
    return "\n".join(lines)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"layout_type": "landing_page", "width": 60},
        }))
        return 0

    layout_type = q.get("layout_type")
    if layout_type not in LAYOUT_TYPES:
        print(json.dumps({
            "error": "'layout_type' must be one of: %s" % ", ".join(LAYOUT_TYPES),
            "example": {"layout_type": "dashboard", "width": 60},
        }))
        return 0

    width = q.get("width", 60)
    try:
        width = int(width)
    except Exception:
        print(json.dumps({"error": "'width' must be an integer"}))
        return 0
    width = max(MIN_WIDTH, min(MAX_WIDTH, width))

    try:
        if layout_type == "landing_page":
            art = _gen_landing_page(width)
        elif layout_type == "dashboard":
            art = _gen_dashboard(width)
        elif layout_type == "form":
            art = _gen_form(width)
        else:
            art = _gen_list_view(width)
    except Exception as e:
        print(json.dumps({"error": "could not build wireframe: %s" % e}))
        return 0

    print(json.dumps({"ascii_wireframe": art, "layout_type": layout_type}, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
