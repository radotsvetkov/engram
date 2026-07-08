#!/usr/bin/env python3
"""slack_message_format — Engram skill (no network). Convert standard
Markdown (CommonMark-ish: **bold**/__bold__, single *italic*/_italic_,
[text](url) links, `- `/`* ` bullets, `# Header` headers, `code`, and
``` code blocks ```) into Slack's "mrkdwn" format, which is similar but
NOT identical to standard Markdown.

Conversions applied:
  **bold** or __bold__          -> *bold*            (Slack bold)
  *italic* or _italic_ (single) -> _italic_           (Slack italic)
  [text](url)                   -> <url|text>         (Slack link)
  "- item" / "* item" (bullets) -> "• item"      (Slack has no real
                                                        nested-list rendering)
  # Header / ## Header / ...    -> *Header*           (no header syntax in
                                                        mrkdwn)
  `inline code`                 -> unchanged (Slack supports backticks)
  ```fenced code blocks```      -> unchanged (Slack supports these too)

Request (stdin): {"markdown": "**Hello** _world_, see [docs](https://x.com)\\n- one\\n- two"}
Output (stdout): {"slack_mrkdwn": str}
"""
import json
import re
import sys

_CODE_FENCE_RE = re.compile(r"```.*?```", re.DOTALL)
_INLINE_CODE_RE = re.compile(r"`[^`\n]*`")
_BOLD_RE = re.compile(r"\*\*(.+?)\*\*|__(.+?)__", re.DOTALL)
_ITALIC_RE = re.compile(r"(?<!\*)\*(?!\*)([^*\n]+?)\*(?!\*)|(?<!_)_(?!_)([^_\n]+?)_(?!_)")
_LINK_RE = re.compile(r"\[([^\]]*)\]\(([^)]+)\)")
_BULLET_RE = re.compile(r"^(\s*)[-*]\s+(.*)$")
_HEADER_RE = re.compile(r"^(\s*)#{1,6}\s+(.*)$")


def _protect(text, pattern, tag):
    """Replace matches of `pattern` with placeholders so later passes skip them;
    return (protected_text, list_of_original_matches). `tag` namespaces the
    placeholders (e.g. "F" for fences, "C" for inline code) so that two
    independent protect/restore passes never collide on the same index."""
    saved = []

    def _sub(m):
        saved.append(m.group(0))
        return "\x00%s%d\x00" % (tag, len(saved) - 1)

    return pattern.sub(_sub, text), saved


def _restore(text, saved, tag):
    def _sub(m):
        return saved[int(m.group(1))]

    return re.sub(r"\x00%s(\d+)\x00" % re.escape(tag), _sub, text)


def _convert_bold(text):
    def _sub(m):
        inner = m.group(1) if m.group(1) is not None else m.group(2)
        return "*%s*" % inner

    return _BOLD_RE.sub(_sub, text)


def _convert_italic(text):
    def _sub(m):
        inner = m.group(1) if m.group(1) is not None else m.group(2)
        return "_%s_" % inner

    return _ITALIC_RE.sub(_sub, text)


def _convert_links(text):
    def _sub(m):
        label, url = m.group(1), m.group(2)
        if label:
            return "<%s|%s>" % (url, label)
        return "<%s>" % url

    return _LINK_RE.sub(_sub, text)


def _convert_line_level(text):
    out_lines = []
    for line in text.split("\n"):
        m = _HEADER_RE.match(line)
        if m:
            content = m.group(2).strip()
            out_lines.append("*%s*" % content if content else "")
            continue
        m = _BULLET_RE.match(line)
        if m:
            indent, content = m.group(1), m.group(2)
            out_lines.append("%s• %s" % (indent, content))
            continue
        out_lines.append(line)
    return "\n".join(out_lines)


def _to_slack_mrkdwn(markdown):
    # Protect fenced code blocks and inline code first — their contents must
    # not be touched by bold/italic/link/bullet/header conversions.
    protected, fences = _protect(markdown, _CODE_FENCE_RE, "F")
    protected, inline_codes = _protect(protected, _INLINE_CODE_RE, "C")

    # Order matters: links before bold/italic (so `**[text](url)**` bolds the
    # whole link). Italic MUST run before bold: the italic regex's lookaround
    # already ignores `**`/`__` pairs (so it only touches genuine single-
    # delimiter italics), whereas if bold ran first it would rewrite `**x**`
    # into Slack's single-asterisk bold `*x*` — which the italic pass would
    # then wrongly re-match and mangle into `_x_`. Line-level conversions
    # (bullets/headers) run last.
    converted = _convert_links(protected)
    converted = _convert_italic(converted)
    converted = _convert_bold(converted)
    converted = _convert_line_level(converted)

    # Restore inline code, then fenced code blocks (reverse order of protection).
    converted = _restore(converted, inline_codes, "C")
    converted = _restore(converted, fences, "F")
    return converted


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"markdown": "**bold** and _italic_"}}))
        return 0

    markdown = q.get("markdown")
    if not isinstance(markdown, str) or not markdown.strip():
        print(json.dumps({
            "error": "provide non-empty 'markdown'",
            "example": {"markdown": "**Hello** _world_, see [docs](https://example.com)\n- one\n- two"},
        }))
        return 0

    try:
        slack_mrkdwn = _to_slack_mrkdwn(markdown)
    except Exception as e:
        print(json.dumps({"error": "conversion failed: %s" % e}))
        return 1

    print(json.dumps({"slack_mrkdwn": slack_mrkdwn}, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
