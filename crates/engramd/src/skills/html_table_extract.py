#!/usr/bin/env python3
"""html_table_extract — Engram skill (NETWORK-capable). Pull <table>s from HTML.

Parses HTML with the stdlib html.parser (NO bs4) and extracts every <table>
into structured {headers, rows}. Uses <th> cells for headers when present,
otherwise falls back to the first <tr>. Cell text is tag-stripped and
whitespace-collapsed. Pass raw `html`, OR a `url` to fetch (bounded read,
20s timeout, User-Agent set) — network only used when `url` is given.

Request (stdin): {"html": "<table>...</table>"}   OR   {"url": "https://..."}
Output (stdout): {source, table_count, tables: [{headers, rows, row_count, col_count}]}
"""
import json
import re
import sys
import urllib.error
import urllib.request
from html.parser import HTMLParser

TIMEOUT = 20
MAX_BYTES = 2_000_000


class _TableParser(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.tables = []          # each: {"rows": [ [ {"is_header":bool,"text":str}, ... ], ... ]}
        self._depth = 0           # table nesting depth
        self._cur_table = None
        self._cur_row = None
        self._cur_cell = None
        self._cur_is_header = False

    def handle_starttag(self, tag, attrs):
        if tag == "table":
            self._depth += 1
            self._cur_table = {"rows": []}
        elif self._cur_table is not None:
            if tag == "tr":
                self._cur_row = []
            elif tag in ("td", "th"):
                self._cur_cell = []
                self._cur_is_header = (tag == "th")

    def handle_data(self, data):
        if self._cur_cell is not None:
            self._cur_cell.append(data)

    def handle_endtag(self, tag):
        if self._cur_table is None:
            return
        if tag in ("td", "th") and self._cur_cell is not None:
            text = re.sub(r"\s+", " ", "".join(self._cur_cell)).strip()
            if self._cur_row is not None:
                self._cur_row.append({"is_header": self._cur_is_header, "text": text})
            self._cur_cell = None
        elif tag == "tr" and self._cur_row is not None:
            self._cur_table["rows"].append(self._cur_row)
            self._cur_row = None
        elif tag == "table":
            # close cell/row if malformed markup left them open
            if self._cur_row is not None:
                self._cur_table["rows"].append(self._cur_row)
                self._cur_row = None
            self.tables.append(self._cur_table)
            self._cur_table = None
            self._depth = max(0, self._depth - 1)


def _structure(raw_table):
    rows = raw_table["rows"]
    headers = []
    body_rows = []
    if not rows:
        return {"headers": [], "rows": [], "row_count": 0, "col_count": 0}
    # If the first row (or any row) has header cells, use them as headers.
    first = rows[0]
    first_all_header = first and all(c["is_header"] for c in first)
    if first_all_header:
        headers = [c["text"] for c in first]
        body_rows = rows[1:]
    else:
        # look for a dedicated header row anywhere; else use first row as headers
        header_row = next((r for r in rows if r and all(c["is_header"] for c in r)), None)
        if header_row is not None:
            headers = [c["text"] for c in header_row]
            body_rows = [r for r in rows if r is not header_row]
        else:
            headers = [c["text"] for c in first]
            body_rows = rows[1:]
    body = [[c["text"] for c in r] for r in body_rows]
    col_count = max([len(headers)] + [len(r) for r in body]) if (headers or body) else 0
    return {"headers": headers, "rows": body, "row_count": len(body), "col_count": col_count}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object"}))
        return 0

    html = q.get("html")
    url = (q.get("url") or "").strip()
    source = "inline"

    if not html and not url:
        print(json.dumps({
            "error": "provide 'html' (a string) or a 'url' to fetch",
            "example": {"html": "<table><tr><th>A</th><th>B</th></tr><tr><td>1</td><td>2</td></tr></table>"},
        }))
        return 0

    if not html and url:
        if not re.match(r"^https?://", url, re.IGNORECASE):
            print(json.dumps({"error": "url must start with http:// or https://"}))
            return 0
        try:
            req = urllib.request.Request(url, headers={
                "User-Agent": "engram-html_table_extract/1",
                "Accept": "text/html,application/xhtml+xml,*/*;q=0.8",
            })
            with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
                raw = resp.read(MAX_BYTES)
            html = raw.decode("utf-8", "replace")
            source = url
        except urllib.error.HTTPError as e:
            print(json.dumps({"error": "fetch failed: HTTP %s for %s" % (e.code, url)}))
            return 0
        except urllib.error.URLError as e:
            print(json.dumps({"error": "fetch failed: could not reach %s (%s)" % (url, e.reason)}))
            return 0
        except Exception as e:
            print(json.dumps({"error": "fetch failed: %s" % e}))
            return 1

    if not isinstance(html, str):
        print(json.dumps({"error": "'html' must be a string"}))
        return 0

    try:
        parser = _TableParser()
        parser.feed(html)
        parser.close()
    except Exception as e:
        print(json.dumps({"error": "could not parse HTML: %s" % e}))
        return 0

    tables = [_structure(t) for t in parser.tables]
    result = {"source": source, "table_count": len(tables), "tables": tables}
    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
