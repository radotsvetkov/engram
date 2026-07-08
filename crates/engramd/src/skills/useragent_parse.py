#!/usr/bin/env python3
"""useragent_parse — Engram skill (no network). Best-effort regex parsing of
a User-Agent string. Heuristic only — UA strings are not authoritative and
can lie or be spoofed; do not use this for security decisions.

Request (stdin): {"ua": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 ..."}
Output (stdout): {raw_ua, browser, browser_version, os, os_version,
                   device_type, is_bot, note}
"""
import json
import re
import sys

_BOT_TOKENS = ("bot", "crawl", "spider", "slurp", "facebookexternalhit")


def _detect_browser(ua):
    # Order matters: Edge and Opera UAs also contain "Chrome/" and "Safari/".
    if "Edg/" in ua:
        m = re.search(r"Edg/([\d.]+)", ua)
        return "Edge", m.group(1) if m else None
    if "OPR/" in ua or "Opera" in ua:
        m = re.search(r"OPR/([\d.]+)", ua) or re.search(r"Opera[/ ]([\d.]+)", ua)
        return "Opera", m.group(1) if m else None
    if "Chrome/" in ua:
        m = re.search(r"Chrome/([\d.]+)", ua)
        return "Chrome", m.group(1) if m else None
    if "Firefox/" in ua:
        m = re.search(r"Firefox/([\d.]+)", ua)
        return "Firefox", m.group(1) if m else None
    if "Version/" in ua and "Safari/" in ua:
        m = re.search(r"Version/([\d.]+)", ua)
        return "Safari", m.group(1) if m else None
    if "MSIE " in ua or "Trident/" in ua:
        m = re.search(r"MSIE ([\d.]+)", ua) or re.search(r"rv:([\d.]+)", ua)
        return "Internet Explorer", m.group(1) if m else None
    return "Unknown", None


def _detect_os(ua):
    m = re.search(r"Windows NT ([\d.]+)", ua)
    if m:
        return "Windows", m.group(1)
    m = re.search(r"Mac OS X ([\d_.]+)", ua)
    if m:
        return "macOS", m.group(1).replace("_", ".")
    m = re.search(r"Android ([\d.]+)", ua)
    if m:
        return "Android", m.group(1)
    m = re.search(r"(?:iPhone OS|CPU OS) ([\d_]+)", ua)
    if m:
        return "iOS", m.group(1).replace("_", ".")
    if "Linux" in ua:
        return "Linux", None
    return "Unknown", None


def _detect_device_type(ua):
    # Checked before the generic mobile tokens because iPad Safari UAs also
    # include a "Mobile/xxxxx" build token.
    if "iPad" in ua or "Tablet" in ua:
        return "tablet"
    if "Mobile" in ua or "Android" in ua or "iPhone" in ua:
        return "mobile"
    return "desktop"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"ua": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36"},
        })); return 0

    ua = q.get("ua")
    if not isinstance(ua, str) or not ua.strip():
        print(json.dumps({
            "error": "missing required field 'ua' (User-Agent string)",
            "example": {"ua": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36"},
        })); return 0

    try:
        browser, browser_version = _detect_browser(ua)
        os_name, os_version = _detect_os(ua)
        device_type = _detect_device_type(ua)
        is_bot = any(tok in ua.lower() for tok in _BOT_TOKENS)

        result = {
            "raw_ua": ua,
            "browser": browser,
            "browser_version": browser_version,
            "os": os_name,
            "os_version": os_version,
            "device_type": device_type,
            "is_bot": is_bot,
            "note": "heuristic best-effort parsing of the User-Agent string — not authoritative, UAs can be spoofed.",
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "useragent_parse failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
