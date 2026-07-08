#!/usr/bin/env python3
"""hmac_sign — Engram skill (no network). Sign or verify a message with HMAC.

Computes an HMAC signature (sha256 by default; sha1/sha512/md5 also supported)
over a message using a shared secret, or verifies a message + signature pair
using a constant-time comparison (hmac.compare_digest). Stdlib only (hmac,
hashlib). The secret is NEVER echoed back in the output.

Request (stdin):
    sign:   {"message": "...", "secret": "...", "algorithm": "sha256"}
    verify: {"message": "...", "secret": "...", "algorithm": "sha256", "signature": "<hex>"}
    "action" ("sign" | "verify") is optional: inferred as "verify" when a
    "signature" is supplied, else "sign".
Output (stdout):
    sign:   {"signature": "<hex>", "algorithm": "sha256"}
    verify: {"valid": true/false, "algorithm": "sha256"}
    (a "warning" key is added when algorithm is sha1 or md5)
"""
import hashlib
import hmac
import json
import sys

ALGORITHMS = ("sha256", "sha1", "sha512", "md5")
WEAK_ALGORITHMS = ("sha1", "md5")
WEAK_WARNING = ("sha1/md5 are weak for new designs — prefer sha256 or better, though some "
                "legacy systems (e.g. older webhook signature schemes) still require them")

_EXAMPLE = {"action": "sign", "message": "hello", "secret": "s3cr3t", "algorithm": "sha256"}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE}))
        return 0

    message = q.get("message")
    secret = q.get("secret")
    signature = q.get("signature")

    action_raw = q.get("action")
    if action_raw is None:
        action = "verify" if isinstance(signature, str) and signature else "sign"
    elif isinstance(action_raw, str):
        action = action_raw.strip().lower()
    else:
        print(json.dumps({
            "error": "'action' must be a string ('sign' or 'verify')",
            "example": _EXAMPLE,
        }))
        return 0

    if action not in ("sign", "verify"):
        print(json.dumps({
            "error": "unknown action %r" % action,
            "actions": ["sign", "verify"],
            "example": _EXAMPLE,
        }))
        return 0

    algorithm_raw = q.get("algorithm")
    if algorithm_raw is None:
        algorithm = "sha256"
    elif isinstance(algorithm_raw, str):
        algorithm = algorithm_raw.strip().lower()
    else:
        print(json.dumps({
            "error": "'algorithm' must be a string",
            "supported": list(ALGORITHMS),
            "example": _EXAMPLE,
        }))
        return 0

    if algorithm not in ALGORITHMS:
        print(json.dumps({
            "error": "unsupported algorithm %r" % algorithm,
            "supported": list(ALGORITHMS),
            "example": _EXAMPLE,
        }))
        return 0

    if not isinstance(message, str) or not message:
        print(json.dumps({
            "error": "provide 'message' (non-empty string)",
            "example": dict(_EXAMPLE, action=action),
        }))
        return 0

    if not isinstance(secret, str) or not secret:
        print(json.dumps({
            "error": "provide 'secret' (non-empty string) — the shared HMAC key",
            "example": dict(_EXAMPLE, action=action),
        }))
        return 0

    if action == "verify" and (not isinstance(signature, str) or not signature):
        print(json.dumps({
            "error": "verify requires 'signature' (hex string) to check the message against",
            "example": {"action": "verify", "message": "hello", "secret": "s3cr3t",
                        "algorithm": "sha256", "signature": "<hex-digest>"},
        }))
        return 0

    try:
        digestmod = getattr(hashlib, algorithm)
        computed = hmac.new(secret.encode("utf-8"), message.encode("utf-8"), digestmod).hexdigest()
    except Exception as e:
        print(json.dumps({"error": "could not compute HMAC: %s" % e}))
        return 0

    if action == "sign":
        result = {"signature": computed, "algorithm": algorithm}
    else:
        result = {"valid": hmac.compare_digest(computed, signature), "algorithm": algorithm}

    if algorithm in WEAK_ALGORITHMS:
        result["warning"] = WEAK_WARNING

    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
