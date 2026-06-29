#!/usr/bin/env python3
"""github — Engram skill (keyless for public data). Look up a GitHub repo.

Repo stats, the latest release, or recent open issues — via the public GitHub
API (no key; ~60 requests/hour unauthenticated). Set GITHUB_TOKEN in the daemon
env to raise the rate limit. Stdlib only.

Request (stdin): {"repo": "NousResearch/hermes-agent", "what": "info"}
  - what: "info" (default) | "release" | "issues"
Output (stdout): depends on `what`.
"""
import json
import os
import sys
import urllib.request

TIMEOUT = 20
API = "https://api.github.com"


def _get(path):
    req = urllib.request.Request(API + path, headers={
        "User-Agent": "engram-github/1", "Accept": "application/vnd.github+json"})
    tok = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    if tok:
        req.add_header("Authorization", "Bearer " + tok)
    with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
        return json.loads(r.read().decode("utf-8", "replace"))


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    repo = (q.get("repo") or "").strip().strip("/")
    if "/" not in repo:
        print(json.dumps({"error": "provide 'repo' as owner/name", "example": {"repo": "torvalds/linux"}}))
        return 0
    what = (q.get("what") or "info").lower()
    try:
        if what == "release":
            r = _get("/repos/%s/releases/latest" % repo)
            print(json.dumps({"repo": repo, "tag": r.get("tag_name"), "name": r.get("name"),
                              "published": r.get("published_at"), "url": r.get("html_url"),
                              "notes": (r.get("body") or "")[:600]}, indent=2, default=str))
        elif what == "issues":
            r = _get("/repos/%s/issues?state=open&per_page=10&sort=updated" % repo)
            issues = [{"number": i.get("number"), "title": i.get("title"),
                       "url": i.get("html_url"), "comments": i.get("comments")}
                      for i in r if "pull_request" not in i]
            print(json.dumps({"repo": repo, "open_issues": issues}, indent=2, default=str))
        else:
            r = _get("/repos/%s" % repo)
            print(json.dumps({
                "repo": r.get("full_name"), "description": r.get("description"),
                "stars": r.get("stargazers_count"), "forks": r.get("forks_count"),
                "open_issues": r.get("open_issues_count"), "language": r.get("language"),
                "license": (r.get("license") or {}).get("spdx_id"),
                "updated": r.get("pushed_at"), "url": r.get("html_url"),
            }, indent=2, default=str))
        return 0
    except urllib.error.HTTPError as e:
        if e.code == 404:
            print(json.dumps({"error": "repo or resource not found: %s" % repo}))
            return 0
        if e.code == 403:
            print(json.dumps({"error": "rate limited — set GITHUB_TOKEN in the daemon env to raise the limit"}))
            return 0
        print(json.dumps({"error": "github error: HTTP %s" % e.code}))
        return 1
    except Exception as e:
        print(json.dumps({"error": "github lookup failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
