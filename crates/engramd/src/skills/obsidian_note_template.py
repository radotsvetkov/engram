#!/usr/bin/env python3
"""obsidian_note_template — Engram skill (no network). Generate an
Obsidian-flavored Markdown note with YAML frontmatter, a heading, body
content, and a Related section of wikilinks.

Request (stdin): {"title": "Meeting Notes", "tags": ["work", "meetings"], "links": ["Project X"], "content": "Discussed roadmap."}
Output (stdout): {filename: "<slug>.md", markdown: str}
"""
import datetime
import json
import re
import sys


def _slugify(title):
    slug = title.strip().lower()
    slug = re.sub(r"[^a-z0-9\s-]", "", slug)
    slug = re.sub(r"[\s-]+", "-", slug).strip("-")
    return slug or "untitled"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"title": "Meeting Notes", "tags": ["work"], "links": ["Project X"]}}))
        return 0

    title = q.get("title")
    if not isinstance(title, str) or not title.strip():
        print(json.dumps({
            "error": "provide non-empty 'title'",
            "example": {"title": "Meeting Notes", "tags": ["work", "meetings"], "links": ["Project X"], "content": "Discussed roadmap."},
        }))
        return 0
    title = title.strip()

    tags = q.get("tags", [])
    if not isinstance(tags, list) or not all(isinstance(t, str) for t in tags):
        print(json.dumps({"error": "'tags' must be a list of strings", "example": {"tags": ["work", "meetings"]}}))
        return 0

    links = q.get("links", [])
    if not isinstance(links, list) or not all(isinstance(l, str) for l in links):
        print(json.dumps({"error": "'links' must be a list of strings", "example": {"links": ["Project X"]}}))
        return 0

    content = q.get("content")
    if content is not None and not isinstance(content, str):
        print(json.dumps({"error": "'content' must be a string if provided"}))
        return 0
    if not content:
        content = "_(no content provided yet — start writing here)_"

    try:
        created = datetime.date.today().isoformat()

        tags_yaml = "[" + ", ".join(tags) + "]" if tags else "[]"
        frontmatter = "---\ntags: %s\ncreated: %s\n---\n" % (tags_yaml, created)

        body = "%s\n# %s\n\n%s\n" % (frontmatter, title, content)

        if links:
            related = "\n## Related\n" + "\n".join("- [[%s]]" % link.strip() for link in links) + "\n"
            body += related

        filename = "%s.md" % _slugify(title)
    except Exception as e:
        print(json.dumps({"error": "template generation failed: %s" % e}))
        return 1

    print(json.dumps({
        "filename": filename,
        "markdown": body,
    }, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
