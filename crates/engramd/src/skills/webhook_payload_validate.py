#!/usr/bin/env python3
"""webhook_payload_validate — Engram skill (no network). Verify an inbound
webhook's HMAC signature AND check it isn't a replay of an old request.

This complements (but is distinct from) the `hmac_sign` skill in this codebase:
hmac_sign does generic raw HMAC sign/verify. This skill is specifically a
webhook-security helper — it additionally checks signature freshness against a
timestamp, the classic replay-attack defense used by Stripe/GitHub-style
webhook verification schemes (a validly-signed payload should still be
REJECTED if it's too old, in case it was captured and resent by an attacker).

Request (stdin): {
    "payload": "<raw request body as received>",
    "signature": "<hex digest from the webhook's signature header>",
    "secret": "<shared webhook signing secret>",
    "timestamp": 1735689600,       # optional: unix seconds, usually from a signature header
    "algorithm": "sha256",          # optional, default sha256
    "max_age_seconds": 300          # optional, default 300 (5 minutes)
}
Output (stdout): {
    "signature_valid": bool,
    "age_seconds": float|null,       # null if no timestamp given
    "within_window": bool|null,      # null if no timestamp given
    "overall_valid": bool,           # true only if signature_valid AND (no timestamp OR within_window)
    "algorithm": "sha256"
}
The "secret" is never echoed back.
"""
import hashlib
import hmac
import json
import sys
import time

_EXAMPLE = {
    "payload": '{"event":"payment.succeeded"}',
    "signature": "<hex-digest-from-webhook-header>",
    "secret": "whsec_...",
    "timestamp": 1735689600,
    "algorithm": "sha256",
    "max_age_seconds": 300,
}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE}))
        return 0

    payload = q.get("payload")
    signature = q.get("signature")
    secret = q.get("secret")

    missing = [k for k, v in (("payload", payload), ("signature", signature), ("secret", secret))
               if not isinstance(v, str) or not v]
    if missing:
        print(json.dumps({
            "error": "missing/invalid required field(s): %s" % ", ".join(missing),
            "example": _EXAMPLE,
        }))
        return 0

    algorithm_raw = q.get("algorithm", "sha256")
    if not isinstance(algorithm_raw, str) or not algorithm_raw.strip():
        print(json.dumps({"error": "'algorithm' must be a non-empty string", "example": _EXAMPLE}))
        return 0
    algorithm = algorithm_raw.strip().lower()
    if not hasattr(hashlib, algorithm):
        print(json.dumps({
            "error": "unsupported algorithm %r" % algorithm_raw,
            "supported_examples": ["sha256", "sha1", "sha512"],
            "example": _EXAMPLE,
        }))
        return 0

    timestamp = q.get("timestamp")
    if timestamp is not None and not isinstance(timestamp, (int, float)):
        print(json.dumps({"error": "'timestamp' must be a number (unix seconds) if provided", "example": _EXAMPLE}))
        return 0

    max_age_seconds = q.get("max_age_seconds", 300)
    if not isinstance(max_age_seconds, (int, float)) or max_age_seconds <= 0:
        print(json.dumps({"error": "'max_age_seconds' must be a positive number if provided", "example": _EXAMPLE}))
        return 0

    try:
        digestmod = getattr(hashlib, algorithm)
        expected_signature = hmac.new(secret.encode("utf-8"), payload.encode("utf-8"), digestmod).hexdigest()
        signature_valid = hmac.compare_digest(expected_signature, signature)

        age_seconds = None
        within_window = None
        if timestamp is not None:
            age_seconds = abs(time.time() - timestamp)
            within_window = age_seconds <= max_age_seconds

        overall_valid = bool(signature_valid and (timestamp is None or within_window))

        result = {
            "signature_valid": signature_valid,
            "age_seconds": age_seconds,
            "within_window": within_window,
            "overall_valid": overall_valid,
            "algorithm": algorithm,
        }
        if timestamp is not None:
            result["note"] = (
                "Even a validly-signed payload should be rejected if within_window is false — an "
                "attacker who captured a genuine, correctly-signed webhook request can replay it "
                "verbatim; checking timestamp freshness (as Stripe/GitHub-style schemes do) is what "
                "prevents that. A signature alone does not prove the request is fresh."
            )
    except Exception as e:
        print(json.dumps({"error": "could not validate webhook payload: %s" % e}))
        return 0

    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
