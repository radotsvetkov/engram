#!/usr/bin/env python3
"""email — Engram seed skill (Process/python3, capability: Net).

Wraps the `himalaya` CLI (https://github.com/pimalaya/himalaya) to list, read, and
send email. Stdlib only; shells out to himalaya so there are no Python deps. This
is the showcase for Engram's trust model: SENDING is egress, so a Process skill
holding the Net capability is refused on a tainted run — a prompt-injection that
arrives *inside* an email it just read physically cannot drive a compose+send.

Request (stdin), one JSON object:
    {"action": "list",  "folder": "INBOX", "limit": 10}
    {"action": "read",  "id": "42", "folder": "INBOX"}
    {"action": "send",  "to": "a@b.com", "subject": "hi", "body": "...",
                         "cc": "c@d.com", "account": "personal"}

Setup (the real work, per the audit): install himalaya and configure an account
in ~/.config/himalaya/config.toml (app-password / OAuth / IMAP+SMTP). With himalaya
absent or unconfigured the skill SUCCEEDS (exit 0) and returns an actionable
"how_to_fix" payload rather than crashing.
"""

import json
import shutil
import subprocess
import sys

HIMALAYA = "himalaya"


def _need_himalaya():
    if shutil.which(HIMALAYA):
        return None
    return {
        "error": "himalaya CLI not found",
        "how_to_fix": {
            "install": "https://github.com/pimalaya/himalaya (cargo install himalaya, brew, or a release binary)",
            "configure": "create ~/.config/himalaya/config.toml with an account (IMAP/SMTP or OAuth)",
        },
        "note": "Email setup is account onboarding, not code — once himalaya works in your shell, this skill works.",
    }


def _run(args, stdin=None):
    """Run himalaya with the given args; return (ok, stdout, stderr)."""
    try:
        p = subprocess.run(
            [HIMALAYA, *args],
            input=stdin,
            capture_output=True,
            text=True,
            timeout=60,
        )
        return p.returncode == 0, p.stdout, p.stderr
    except Exception as e:  # missing binary slipped through, timeout, etc.
        return False, "", str(e)


def _account_args(q):
    acct = q.get("account")
    return ["-a", acct] if acct else []


def _maybe_json(text):
    try:
        return json.loads(text)
    except Exception:
        return text.strip()


def action_list(q):
    folder = q.get("folder", "INBOX")
    limit = str(int(q.get("limit", 10)))
    args = _account_args(q) + ["envelope", "list", "-f", folder, "-s", limit, "-o", "json"]
    ok, out, err = _run(args)
    if not ok:
        return {"error": "list failed", "detail": err.strip()[:500]}
    return {"action": "list", "folder": folder, "envelopes": _maybe_json(out)}


def action_read(q):
    mid = q.get("id")
    if not mid:
        return {"error": "read requires 'id'"}
    args = _account_args(q) + ["message", "read", str(mid)]
    folder = q.get("folder")
    if folder:
        args += ["-f", folder]
    ok, out, err = _run(args)
    if not ok:
        return {"error": "read failed", "detail": err.strip()[:500]}
    return {"action": "read", "id": mid, "message": out.strip()}


def action_send(q):
    to = q.get("to")
    if not to:
        return {"error": "send requires 'to'"}
    subject = q.get("subject", "")
    body = q.get("body", "")
    headers = ["To: %s" % to, "Subject: %s" % subject]
    if q.get("cc"):
        headers.append("Cc: %s" % q["cc"])
    if q.get("bcc"):
        headers.append("Bcc: %s" % q["bcc"])
    mime = "\n".join(headers) + "\n\n" + body + "\n"
    ok, out, err = _run(_account_args(q) + ["message", "send"], stdin=mime)
    if not ok:
        return {"error": "send failed", "detail": err.strip()[:500]}
    return {"action": "send", "to": to, "subject": subject, "status": "sent"}


ACTIONS = {"list": action_list, "read": action_read, "send": action_send}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    missing = _need_himalaya()
    if missing:
        print(json.dumps(missing, indent=2))
        return 0

    action = (q.get("action") or "").lower()
    fn = ACTIONS.get(action)
    if not fn:
        print(json.dumps({
            "error": "unknown action %r" % action,
            "actions": list(ACTIONS),
            "example": {"action": "list", "folder": "INBOX", "limit": 10},
        }))
        return 0

    print(json.dumps(fn(q), indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
