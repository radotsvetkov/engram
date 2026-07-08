#!/usr/bin/env python3
"""github_issue_create — Engram skill (network, MUTATING). Create a GitHub issue.

Creates a new issue in a GitHub repository via the REST API. This is a real
write to an external system — same risk class as the `email` skill's "send"
action: once this call succeeds, the issue exists on GitHub and this skill
cannot un-create it. Requires a GITHUB_TOKEN (or GH_TOKEN) with 'repo' or
'issues:write' scope in the daemon environment. Stdlib only (urllib.request).

Request (stdin): {"repo": "owner/name", "title": "...", "body": "...", "labels": ["bug"]}
Output (stdout): {"issue_number": 123, "url": "https://github.com/owner/name/issues/123", "title": "..."}
The token is NEVER echoed back in the output.
"""
import json
import os
import sys
import urllib.error
import urllib.request

TIMEOUT = 20
API = "https://api.github.com"
_EXAMPLE = {"repo": "owner/name", "title": "Bug: something is broken", "body": "details here", "labels": ["bug"]}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE}))
        return 0

    repo_raw = q.get("repo")
    repo = repo_raw.strip().strip("/") if isinstance(repo_raw, str) else ""
    if "/" not in repo or repo.count("/") != 1 or not all(repo.split("/")):
        print(json.dumps({"error": "provide 'repo' as owner/name", "example": _EXAMPLE}))
        return 0

    title = q.get("title")
    if not isinstance(title, str) or not title.strip():
        print(json.dumps({"error": "provide 'title' (non-empty string)", "example": _EXAMPLE}))
        return 0

    body_raw = q.get("body")
    if body_raw is None:
        body = ""
    elif isinstance(body_raw, str):
        body = body_raw
    else:
        print(json.dumps({"error": "'body' must be a string if provided", "example": _EXAMPLE}))
        return 0

    labels = q.get("labels")
    if labels is not None and not (isinstance(labels, list) and all(isinstance(x, str) for x in labels)):
        print(json.dumps({"error": "'labels' must be a list of strings if provided", "example": _EXAMPLE}))
        return 0

    token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
    if not token:
        print(json.dumps({
            "error": "GITHUB_TOKEN (or GH_TOKEN) is required to create issues — set it in the daemon environment",
            "how_to_fix": "create a GitHub personal access token with 'repo' or 'issues:write' scope and set GITHUB_TOKEN",
        }))
        return 0

    payload = {"title": title.strip()}
    if body:
        payload["body"] = body
    if labels:
        payload["labels"] = labels

    req = urllib.request.Request(
        "%s/repos/%s/issues" % (API, repo),
        method="POST",
        data=json.dumps(payload).encode("utf-8"),
        headers={
            "Authorization": "Bearer %s" % token,
            "Accept": "application/vnd.github+json",
            "User-Agent": "engram-github-issue/1",
            "Content-Type": "application/json",
        },
    )

    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            resp = json.loads(r.read().decode("utf-8", "replace"))
        print(json.dumps({
            "issue_number": resp.get("number"),
            "url": resp.get("html_url"),
            "title": resp.get("title"),
        }, indent=2, default=str))
        return 0
    except urllib.error.HTTPError as e:
        try:
            err_body = json.loads(e.read().decode("utf-8", "replace"))
            if not isinstance(err_body, dict):
                err_body = {}
        except Exception:
            err_body = {}
        detail = (err_body.get("message") or "")[:500]
        if e.code == 404:
            print(json.dumps({"error": "repo not found, or the token lacks access to it: %s" % repo}))
            return 0
        if e.code == 401:
            print(json.dumps({"error": "GitHub rejected the token — it may be invalid, revoked, or expired"}))
            return 0
        if e.code == 403:
            print(json.dumps({
                "error": "forbidden — rate-limited by GitHub, or the token lacks sufficient scope "
                         "('repo' or 'issues:write')",
                "detail": detail,
            }))
            return 0
        if e.code == 422:
            print(json.dumps({
                "error": "validation failed — check that 'repo' exists and any 'labels' already exist on the repo",
                "detail": err_body.get("errors") or detail or None,
            }, default=str))
            return 0
        print(json.dumps({"error": "github error: HTTP %s" % e.code, "detail": detail}))
        return 1
    except Exception as e:
        print(json.dumps({"error": "github issue creation failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
