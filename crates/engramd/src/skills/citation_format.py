#!/usr/bin/env python3
"""citation_format — Engram skill (no network). Format a basic single-author
citation in APA, MLA, or Chicago style for a book, journal article, or
website, using the well-documented common single-author templates.

Request (stdin): {"style": "APA"|"MLA"|"Chicago", "type": "book"|"article"|"website",
                   "fields": {"author": "Last, First", "title": ..., "year": ...,
                              "publisher": ..., "journal": ..., "volume": ...,
                              "issue": ..., "pages": ..., "site_name": ...,
                              "url": ..., "accessed": "YYYY-MM-DD"}}
Output (stdout): {citation, style, type, note}
"""
import json
import sys

_STYLES = ["APA", "MLA", "Chicago"]
_TYPES = ["book", "article", "website"]

_NOTE = ("basic single-author format — for multiple authors, editors, or "
         "edition numbers, adjust manually.")
_CHICAGO_NOTE = (_NOTE + " Chicago is shown in notes-bibliography style "
                 "(Author. Title. Publisher, Year.); the author-date variant "
                 "instead places the year right after the author "
                 "(Author. Year. Title. Publisher.).")


def _get(fields, key, default=""):
    v = fields.get(key)
    return str(v) if v not in (None, "") else default


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"style": "APA", "type": "book",
                        "fields": {"author": "Doe, Jane", "title": "A Great Book",
                                   "year": 2020, "publisher": "Acme Press"}},
        })); return 0

    style = q.get("style")
    ctype = q.get("type")
    fields = q.get("fields")

    if style not in _STYLES or ctype not in _TYPES:
        print(json.dumps({
            "error": "missing or unsupported 'style'/'type'",
            "valid_styles": _STYLES,
            "valid_types": _TYPES,
            "example": {"style": "APA", "type": "book",
                        "fields": {"author": "Doe, Jane", "title": "A Great Book",
                                   "year": 2020, "publisher": "Acme Press"}},
        })); return 0

    if not isinstance(fields, dict):
        print(json.dumps({
            "error": "missing required field 'fields' (object with citation details)",
            "example": {"style": "APA", "type": "book",
                        "fields": {"author": "Doe, Jane", "title": "A Great Book",
                                   "year": 2020, "publisher": "Acme Press"}},
        })); return 0

    try:
        author = _get(fields, "author", "Unknown Author")
        title = _get(fields, "title", "Untitled")
        year = _get(fields, "year", "n.d.")
        publisher = _get(fields, "publisher")
        journal = _get(fields, "journal")
        volume = _get(fields, "volume")
        issue = _get(fields, "issue")
        pages = _get(fields, "pages")
        site_name = _get(fields, "site_name")
        url = _get(fields, "url")
        accessed = _get(fields, "accessed")

        note = _CHICAGO_NOTE if style == "Chicago" else _NOTE

        if style == "APA":
            if ctype == "book":
                citation = "%s. (%s). %s. %s." % (author, year, title, publisher)
            elif ctype == "article":
                citation = "%s. (%s). %s. %s, %s(%s), %s." % (
                    author, year, title, journal, volume, issue, pages)
            else:  # website
                citation = "%s. (%s). %s. %s. Retrieved %s, from %s" % (
                    author, year, title, site_name, accessed, url)
        elif style == "MLA":
            if ctype == "book":
                citation = "%s. %s. %s, %s." % (author, title, publisher, year)
            elif ctype == "article":
                citation = '%s. "%s." %s, vol. %s, no. %s, %s, pp. %s.' % (
                    author, title, journal, volume, issue, year, pages)
            else:  # website
                citation = '%s. "%s." %s, %s, %s. Accessed %s.' % (
                    author, title, site_name, year, url, accessed)
        else:  # Chicago
            if ctype == "book":
                citation = "%s. %s. %s, %s." % (author, title, publisher, year)
            elif ctype == "article":
                citation = '%s. "%s." %s %s, no. %s (%s): %s.' % (
                    author, title, journal, volume, issue, year, pages)
            else:  # website
                citation = '%s. "%s." %s. Accessed %s. %s.' % (
                    author, title, site_name, accessed, url)

        result = {"citation": citation, "style": style, "type": ctype, "note": note}
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "citation_format failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
