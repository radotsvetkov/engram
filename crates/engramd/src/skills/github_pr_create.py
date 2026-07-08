#!/usr/bin/env python3
"""github_pr_create — Engram skill (network, MUTATING). Open a real GitHub pull request.

Performs a real write to an external GitHub repository — same risk class as the
existing `github_issue_create`/`email` skills' write actions. Needs GITHUB_TOKEN
(or GH_TOKEN) with 'repo'/'pull_requests:write' scope in the daemon environment.

Request (stdin): {"repo": "owner/name", "title": "...", "head": "feature-branch",
                  "base": "main", "body": "...", "draft": false}
  - head: the branch with your changes (owner:branch form also accepted for forks)
  - base: the branch you want to merge into (default "main")
Output (stdout): {pr_number, url, title} on success, or {error, how_to_fix}.
"""
import json
import os
import sys
import urllib.error
import urllib.request

TIMEOUT = 20
API = "https://api.github.com"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object"}))
        return 0

    repo = (q.get("repo") or "").strip().strip("/")
    title = q.get("title")
    head = q.get("head")
    base = q.get("base") or "main"
    body = q.get("body") or ""
    draft = bool(q.get("draft", False))

    if "/" not in repo or not isinstance(title, str) or not title.strip() or not isinstance(head, str) or not head.strip():
        print(json.dumps({
            "error": "provide 'repo' as owner/name, a non-empty 'title', and a non-empty 'head' branch",
            "example": {"repo": "octocat/hello-world", "title": "Fix typo", "head": "fix-typo", "base": "main", "body": "..."},
        }))
        return 0

    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    if not token:
        print(json.dumps({
            "error": "GITHUB_TOKEN (or GH_TOKEN) is required to create pull requests — set it in the daemon environment",
            "how_to_fix": "create a GitHub personal access token with 'repo' or 'pull_requests:write' scope and set GITHUB_TOKEN",
        }))
        return 0

    payload = {"title": title, "head": head, "base": base, "body": body, "draft": draft}
    req = urllib.request.Request(
        API + "/repos/%s/pulls" % repo,
        data=json.dumps(payload).encode("utf-8"),
        method="POST",
        headers={
            "Authorization": "Bearer %s" % token,
            "Accept": "application/vnd.github+json",
            "User-Agent": "engram-github-pr/1",
            "Content-Type": "application/json",
        },
    )
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            resp = json.loads(r.read().decode("utf-8", "replace"))
            print(json.dumps({
                "pr_number": resp.get("number"),
                "url": resp.get("html_url"),
                "title": resp.get("title"),
            }, indent=2, default=str))
            return 0
    except urllib.error.HTTPError as e:
        detail = ""
        try:
            body_json = json.loads(e.read().decode("utf-8", "replace"))
            detail = body_json.get("message", "")
            errors = body_json.get("errors")
            if errors:
                detail += " — " + json.dumps(errors)
        except Exception:
            pass
        if e.code == 404:
            print(json.dumps({"error": "repo not found or token lacks access: %s" % repo, "detail": detail}))
        elif e.code == 401:
            print(json.dumps({"error": "GitHub token is invalid or expired", "detail": detail}))
        elif e.code == 403:
            print(json.dumps({"error": "rate-limited or token lacks 'pull_requests:write' scope", "detail": detail}))
        elif e.code == 422:
            print(json.dumps({"error": "validation failed — check head/base branches exist and there isn't already an open PR for this head/base pair", "detail": detail}))
        else:
            print(json.dumps({"error": "github error: HTTP %s" % e.code, "detail": detail}))
        return 0
    except Exception as e:
        print(json.dumps({"error": "github PR creation failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
