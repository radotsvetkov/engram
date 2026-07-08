#!/usr/bin/env python3
"""jwt_decode — Engram skill (no network). Decode a JWT's header and payload.

This ONLY decodes the base64url-encoded header/payload — it does NOT verify
the signature, so it can never prove the token is authentic or untampered.
Useful for inspecting claims (exp/iat/nbf/sub/...) of a token you already trust.

Request (stdin): {"token": "eyJhbGciOi...header.payload.signature"}
Output (stdout): {header, payload, is_expired, expires_at, warning}
"""
import base64
import datetime
import json
import sys


def _b64url_decode(segment, label):
    padded = segment + "=" * (-len(segment) % 4)
    try:
        raw = base64.urlsafe_b64decode(padded.encode("ascii"))
    except Exception as e:
        raise ValueError("could not base64-decode %s: %s" % (label, e))
    try:
        return json.loads(raw.decode("utf-8"))
    except Exception as e:
        raise ValueError("could not parse %s as JSON: %s" % (label, e))


def _iso(ts):
    return datetime.datetime.fromtimestamp(ts, datetime.timezone.utc).isoformat()


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"token": "header.payload.signature"}}))
        return 0

    token = (q.get("token") or "").strip()
    if not token:
        print(json.dumps({"error": "provide a 'token'",
                          "example": {"token": "header.payload.signature"}}))
        return 0

    parts = token.split(".")
    if len(parts) != 3:
        print(json.dumps({
            "error": "not a valid JWT structure (expected 3 dot-separated parts)",
            "example": {"token": "header.payload.signature"},
        }))
        return 0

    try:
        header_b64, payload_b64, _sig_b64 = parts
        try:
            header = _b64url_decode(header_b64, "header")
        except ValueError as e:
            print(json.dumps({"error": str(e)}))
            return 0
        try:
            payload = _b64url_decode(payload_b64, "payload")
        except ValueError as e:
            print(json.dumps({"error": str(e)}))
            return 0

        is_expired = None
        expires_at = None
        issued_at = None
        not_before = None

        if isinstance(payload, dict):
            exp = payload.get("exp")
            if isinstance(exp, (int, float)):
                is_expired = datetime.datetime.now(datetime.timezone.utc).timestamp() > exp
                expires_at = _iso(exp)
            iat = payload.get("iat")
            if isinstance(iat, (int, float)):
                issued_at = _iso(iat)
            nbf = payload.get("nbf")
            if isinstance(nbf, (int, float)):
                not_before = _iso(nbf)

        result = {
            "header": header,
            "payload": payload,
            "is_expired": is_expired,
            "expires_at": expires_at,
            "issued_at": issued_at,
            "not_before": not_before,
            "warning": "signature NOT verified — this only decodes the token's "
                       "contents, it does not prove authenticity",
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "jwt_decode failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
