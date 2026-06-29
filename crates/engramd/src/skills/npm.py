#!/usr/bin/env python3
"""npm — Engram skill (keyless). Info about a published npm package.

Looks up a package on the public npm registry. Request shape: {"package": "<name>"}
(scoped names like "@scope/pkg" are supported). It reads the latest dist-tag and
that version's metadata. Output shape: {name, version, description, license,
homepage, repository, url}.
"""
import json, sys
import urllib.request, urllib.parse, urllib.error


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    package = (q.get("package") or q.get("name") or "").strip()
    if not package:
        print(json.dumps({
            "error": "missing required field 'package'",
            "example": {"package": "express"},
        }))
        return 0

    # Encode the package name for the URL. Scoped names (@scope/pkg) keep their
    # single slash, so encode the @scope and the pkg parts separately.
    if package.startswith("@") and "/" in package:
        scope, _, name = package.partition("/")
        path = urllib.parse.quote(scope, safe="@") + "/" + urllib.parse.quote(name, safe="")
    else:
        path = urllib.parse.quote(package, safe="")
    api = "https://registry.npmjs.org/" + path

    try:
        req = urllib.request.Request(api, headers={
            "User-Agent": "engram-skill-npm/1.0",
            "Accept": "application/json",
        })
        with urllib.request.urlopen(req, timeout=20) as resp:
            raw = resp.read()
        data = json.loads(raw.decode("utf-8", "replace"))

        latest = ((data.get("dist-tags") or {}).get("latest"))
        versions = data.get("versions") or {}
        if not latest:
            # Fall back to the newest known version if dist-tags is missing.
            latest = max(versions.keys()) if versions else None
        v = versions.get(latest) or {} if latest else {}

        # license can be a string or an object like {"type": "MIT"}.
        lic = v.get("license")
        if isinstance(lic, dict):
            lic = lic.get("type") or lic.get("name")
        elif isinstance(lic, list):
            lic = ", ".join(
                (x.get("type") if isinstance(x, dict) else str(x)) for x in lic
            ) or None

        repo = v.get("repository")
        repo_url = (repo or {}).get("url") if isinstance(repo, dict) else (
            repo if isinstance(repo, str) else None
        )

        name = data.get("name") or package
        result = {
            "name": name,
            "version": latest,
            "description": v.get("description"),
            "license": lic,
            "homepage": v.get("homepage"),
            "repository": repo_url,
            "url": "https://www.npmjs.com/package/" + name,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except urllib.error.HTTPError as e:
        if e.code == 404:
            print(json.dumps({"error": "no npm package named %s" % package}))
            return 0
        print(json.dumps({"error": "npm failed: HTTP %s %s" % (e.code, e.reason)}))
        return 0
    except urllib.error.URLError as e:
        print(json.dumps({"error": "npm failed: network error: %s" % e.reason}))
        return 0
    except Exception as e:
        print(json.dumps({"error": "npm failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
