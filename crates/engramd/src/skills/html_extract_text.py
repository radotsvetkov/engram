#!/usr/bin/env python3
"""html_extract_text — Engram skill (NETWORK-capable). HTML -> readable text.

Strips HTML down to plain readable text using the stdlib html.parser (NO bs4):
drops <script>/<style>/<head> contents, inserts newlines at block boundaries
(p, div, br, li, h1-h6, tr, section, article, etc.), and collapses excess
whitespace. Handles unicode/multibyte safely (utf-8, errors="replace"). Pass
raw `html`, OR a `url` to fetch (bounded read, 20s timeout) — network only used
when `url` is given.

Request (stdin): {"html": "<h1>Hi</h1><p>Body</p>"}   OR   {"url": "https://..."}
Output (stdout): {source, title?, text, word_count, char_count}
"""
import json
import re
import sys
import urllib.error
import urllib.request
from html.parser import HTMLParser

TIMEOUT = 20
MAX_BYTES = 3_000_000
MAX_HTML_CHARS = 5_000_000

_SKIP = {"script", "style", "head", "noscript", "template", "svg"}
_BLOCK = {
    "p", "div", "br", "li", "ul", "ol", "tr", "table", "section", "article",
    "header", "footer", "nav", "aside", "figure", "figcaption", "blockquote",
    "pre", "hr", "form", "h1", "h2", "h3", "h4", "h5", "h6", "dd", "dt", "dl",
}


class _TextParser(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.parts = []
        self._skip_depth = 0
        self._in_title = False
        self.title = None

    def handle_starttag(self, tag, attrs):
        if tag in _SKIP:
            self._skip_depth += 1
        elif tag == "title":
            self._in_title = True
        elif tag in _BLOCK:
            self.parts.append("\n")

    def handle_startendtag(self, tag, attrs):
        if tag == "br":
            self.parts.append("\n")

    def handle_endtag(self, tag):
        if tag in _SKIP and self._skip_depth > 0:
            self._skip_depth -= 1
        elif tag == "title":
            self._in_title = False
        elif tag in _BLOCK:
            self.parts.append("\n")

    def handle_data(self, data):
        # Title lives inside <head> (a skipped region), so capture it before
        # the skip check.
        if self._in_title:
            t = re.sub(r"\s+", " ", data).strip()
            if t:
                self.title = ((self.title + " ") if self.title else "") + t
            return
        if self._skip_depth > 0:
            return
        self.parts.append(data)


def _clean(text):
    # collapse spaces/tabs within lines, then trim blank lines to at most one.
    lines = []
    for line in text.split("\n"):
        line = re.sub(r"[ \t\r\f\v]+", " ", line).strip()
        lines.append(line)
    out = []
    blank = False
    for line in lines:
        if not line:
            if not blank and out:
                out.append("")
            blank = True
        else:
            out.append(line)
            blank = False
    return "\n".join(out).strip()


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
            "example": {"html": "<h1>Title</h1><p>Some body text.</p>"},
        }))
        return 0

    if not html and url:
        if not re.match(r"^https?://", url, re.IGNORECASE):
            print(json.dumps({"error": "url must start with http:// or https://"}))
            return 0
        try:
            req = urllib.request.Request(url, headers={
                "User-Agent": "engram-html_extract_text/1",
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
    if len(html) > MAX_HTML_CHARS:
        html = html[:MAX_HTML_CHARS]

    try:
        parser = _TextParser()
        parser.feed(html)
        parser.close()
    except Exception as e:
        print(json.dumps({"error": "could not parse HTML: %s" % e}))
        return 0

    text = _clean("".join(parser.parts))
    words = text.split()
    result = {
        "source": source,
        "text": text,
        "word_count": len(words),
        "char_count": len(text),
    }
    if parser.title:
        result["title"] = parser.title
    # put title first for readability
    ordered = {"source": result["source"]}
    if "title" in result:
        ordered["title"] = result["title"]
    ordered["word_count"] = result["word_count"]
    ordered["char_count"] = result["char_count"]
    ordered["text"] = result["text"]
    print(json.dumps(ordered, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
