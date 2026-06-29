#!/usr/bin/env python3
"""pwned — Engram skill (keyless). Check if a password leaked WITHOUT sending it.

Uses the Have I Been Pwned Pwned Passwords range API with k-anonymity: only the
first 5 chars of the password's SHA-1 hash ever leave this machine. Request shape:
{"password": "<secret>"}. Output: {"pwned": bool, "count": int, "advice": str}.
The password and its full hash are NEVER included in the output.
"""
import json, sys, hashlib, urllib.request, urllib.error


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"password": "hunter2"}})); return 0

    password = q.get("password")
    if password is None or password == "":
        print(json.dumps({"error": "provide a password",
                          "example": {"password": "hunter2"}})); return 0

    if not isinstance(password, str):
        password = str(password)

    try:
        h = hashlib.sha1(password.encode("utf-8")).hexdigest().upper()
        prefix, suffix = h[:5], h[5:]

        url = "https://api.pwnedpasswords.com/range/%s" % prefix
        req = urllib.request.Request(url, headers={"User-Agent": "engram-pwned/1"})
        with urllib.request.urlopen(req, timeout=20) as resp:
            body = resp.read().decode("utf-8", "replace")

        count = 0
        for line in body.splitlines():
            line = line.strip()
            if not line or ":" not in line:
                continue
            line_suffix, _, line_count = line.partition(":")
            if line_suffix.strip().upper() == suffix:
                try:
                    count = int(line_count.strip())
                except ValueError:
                    count = 0
                break

        pwned = count > 0
        advice = ("This password has appeared in breaches — do not use it."
                  if pwned else "Not found in known breaches.")
        result = {"pwned": pwned, "count": count, "advice": advice}
        print(json.dumps(result, indent=2, default=str)); return 0
    except urllib.error.HTTPError as e:
        print(json.dumps({"error": "pwned failed: HTTP %s from HIBP" % e.code,
                          "how_to_fix": "HIBP may be rate-limiting; retry shortly."}))
        return 0
    except urllib.error.URLError as e:
        print(json.dumps({"error": "pwned failed: network error: %s" % e.reason,
                          "how_to_fix": "Check connectivity to api.pwnedpasswords.com."}))
        return 0
    except Exception as e:
        print(json.dumps({"error": "pwned failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
