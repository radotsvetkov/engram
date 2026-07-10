#!/usr/bin/env python3
"""markdown_table_gen — Engram skill (no network). Build or parse a Markdown table.

BUILD (from {rows:[obj]} or {headers:[str], data:[[...]]}): emits a GitHub-
flavored markdown table (header, `---` separator, data rows), escaping `|` in
cells. PARSE (from {markdown:"..."}): reads a markdown table back into
{headers, rows}. Direction is auto-detected by which key is present.

Request (stdin): {"rows": [obj]} | {"headers": [...], "data": [[...]]} | {"markdown": "..."}
Output (stdout): {markdown}  OR  {headers, rows}
"""
import json, sys


def _cell(v):
    if v is None:
        return ""
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, (dict, list)):
        v = json.dumps(v, separators=(",", ":"), default=str)
    else:
        v = str(v)
    # Escape pipes and collapse newlines so the cell stays on one row.
    return v.replace("\\", "\\\\").replace("|", "\\|").replace("\n", "<br>")


def _build_from_headers(headers, data):
    headers = [str(h) for h in headers]
    lines = []
    lines.append("| " + " | ".join(_cell(h) for h in headers) + " |")
    lines.append("| " + " | ".join("---" for _ in headers) + " |")
    for row in data:
        if not isinstance(row, list):
            row = [row]
        cells = [_cell(row[i]) if i < len(row) else "" for i in range(len(headers))]
        lines.append("| " + " | ".join(cells) + " |")
    return "\n".join(lines)


def _split_row(line):
    """Split a markdown table row into cells, honoring backslash-escaped pipes."""
    s = line.strip()
    if s.startswith("|"):
        s = s[1:]
    if s.endswith("|"):
        s = s[:-1]
    cells = []
    buf = []
    i = 0
    while i < len(s):
        c = s[i]
        if c == "\\" and i + 1 < len(s):
            buf.append(s[i + 1])
            i += 2
            continue
        if c == "|":
            cells.append("".join(buf).strip())
            buf = []
            i += 1
            continue
        buf.append(c)
        i += 1
    cells.append("".join(buf).strip())
    # Restore soft line breaks encoded during build.
    return [c.replace("<br>", "\n") for c in cells]


def _is_separator(cells):
    if not cells:
        return False
    for c in cells:
        t = c.strip().replace(":", "").replace("-", "").replace(" ", "")
        if t != "" or "-" not in c:
            return False
    return True


def _parse(markdown):
    raw_lines = [ln for ln in markdown.splitlines() if ln.strip() != ""]
    # Keep only lines that look like table rows (contain a pipe).
    table_lines = [ln for ln in raw_lines if "|" in ln]
    if not table_lines:
        return None
    parsed = [_split_row(ln) for ln in table_lines]
    headers = parsed[0]
    body = parsed[1:]
    # Drop the separator row if present.
    if body and _is_separator(body[0]):
        body = body[1:]
    width = len(headers)
    rows = []
    for cells in body:
        obj = {}
        for i, h in enumerate(headers):
            key = h
            if key in obj:
                n = 2
                while ("%s_%d" % (h, n)) in obj:
                    n += 1
                key = "%s_%d" % (h, n)
            obj[key] = cells[i] if i < len(cells) else ""
        rows.append(obj)
    return {"headers": headers, "rows": rows}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"rows": [{"name": "Alice", "age": 30}]},
        })); return 0

    has_md = "markdown" in q and isinstance(q.get("markdown"), str)
    has_rows = "rows" in q
    has_hd = "headers" in q and "data" in q

    if not (has_md or has_rows or has_hd):
        print(json.dumps({
            "error": "provide 'rows' or 'headers'+'data' to build, or 'markdown' to parse",
            "example": {"rows": [{"name": "Alice", "age": 30}, {"name": "Bob", "age": 25}]},
        })); return 0

    try:
        if has_md:
            result = _parse(q["markdown"])
            if result is None:
                print(json.dumps({"error": "no markdown table found in 'markdown'"})); return 0
            print(json.dumps(result, indent=2, default=str)); return 0

        if has_rows:
            rows = q.get("rows")
            if not isinstance(rows, list):
                print(json.dumps({"error": "'rows' must be a list of objects"})); return 0
            # Union of keys, first-seen order.
            headers = []
            seen = set()
            for item in rows:
                if isinstance(item, dict):
                    for k in item.keys():
                        if k not in seen:
                            seen.add(k)
                            headers.append(k)
            if not headers:
                print(json.dumps({"error": "no object keys found in 'rows'"})); return 0
            data = [[item.get(h) for h in headers] if isinstance(item, dict) else [item] for item in rows]
            md = _build_from_headers(headers, data)
            print(json.dumps({"markdown": md}, indent=2, default=str)); return 0

        # headers + data
        headers = q.get("headers")
        data = q.get("data")
        if not isinstance(headers, list) or not headers:
            print(json.dumps({"error": "'headers' must be a non-empty list"})); return 0
        if not isinstance(data, list):
            print(json.dumps({"error": "'data' must be a list of rows"})); return 0
        md = _build_from_headers(headers, data)
        print(json.dumps({"markdown": md}, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "markdown_table_gen failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
