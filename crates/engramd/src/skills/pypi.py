#!/usr/bin/env python3
"""pypi — Engram skill (keyless). Info about a Python package on PyPI.

Uses the public PyPI JSON API (https://pypi.org/pypi/<package>/json, no key).
Request (stdin): {"package": "requests"}
Output (stdout): {name, version, summary, author, license, requires_python,
                  home_page, url}. HTTP 404 -> {"error": "no PyPI package named ..."}.
"""
import json
import sys
import urllib.error
import urllib.parse
import urllib.request

TIMEOUT = 20


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    package = (q.get("package") or q.get("name") or q.get("pkg") or "").strip()
    if not package:
        print(json.dumps({"error": "provide 'package'", "example": {"package": "requests"}}))
        return 0
    url = "https://pypi.org/pypi/%s/json" % urllib.parse.quote(package, safe="")
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "engram-pypi/1"})
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            data = json.loads(r.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as e:
        if e.code == 404:
            print(json.dumps({"error": "no PyPI package named %s" % package}))
            return 0
        print(json.dumps({"error": "PyPI lookup failed: HTTP %s" % e.code}))
        return 1
    except Exception as e:
        print(json.dumps({"error": "pypi failed: %s" % e}))
        return 1
    try:
        info = data.get("info") if isinstance(data, dict) else None
        if not isinstance(info, dict):
            print(json.dumps({"error": "unexpected PyPI response for %s" % package}))
            return 0
        name = info.get("name") or package
        home_page = info.get("home_page") or ""
        if not home_page:
            project_urls = info.get("project_urls")
            if isinstance(project_urls, dict):
                for v in project_urls.values():
                    if v:
                        home_page = v
                        break
        result = {
            "name": name,
            "version": info.get("version"),
            "summary": info.get("summary"),
            "author": info.get("author"),
            "license": info.get("license"),
            "requires_python": info.get("requires_python"),
            "home_page": home_page or None,
            "url": "https://pypi.org/project/%s/" % name,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "pypi failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
