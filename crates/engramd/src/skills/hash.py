#!/usr/bin/env python3
"""hash — Engram skill (no network). Hash + encode/decode text, pure compute.

Reads a JSON object {text, decode?} on stdin. If "decode" is one of
{"base64","hex","url"}, it decodes "text" with that codec and returns {decoded}.
Otherwise it returns {hashes:{md5,sha1,sha256,sha512}, encodings:{base64,hex,url}}
all computed from text.encode("utf-8"). Errors are reported as {"error":...}.
"""
import json, sys, hashlib, base64, binascii
import urllib.parse


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"text": "hello world"},
        })); return 0

    text = q.get("text")
    if text is None or not isinstance(text, str):
        print(json.dumps({
            "error": "missing required field 'text' (string)",
            "example": {"text": "hello world"},
            "how_to_fix": "Pass {\"text\":\"...\"}. Add \"decode\":\"base64\"|\"hex\"|\"url\" to decode instead of hashing.",
        })); return 0

    decode = q.get("decode")

    try:
        # --- decode mode ---
        if decode is not None:
            mode = str(decode).strip().lower()
            if mode not in ("base64", "hex", "url"):
                print(json.dumps({
                    "error": "unsupported decode mode: %r" % decode,
                    "how_to_fix": "decode must be one of: base64, hex, url",
                    "example": {"text": "aGVsbG8=", "decode": "base64"},
                })); return 0

            if mode == "base64":
                try:
                    raw = base64.b64decode(text, validate=True)
                except (binascii.Error, ValueError) as de:
                    print(json.dumps({
                        "error": "invalid base64 input: %s" % de,
                        "example": {"text": "aGVsbG8gd29ybGQ=", "decode": "base64"},
                    })); return 0
                decoded = raw.decode("utf-8", errors="replace")
            elif mode == "hex":
                cleaned = "".join(text.split())
                try:
                    raw = bytes.fromhex(cleaned)
                except ValueError as de:
                    print(json.dumps({
                        "error": "invalid hex input: %s" % de,
                        "example": {"text": "68656c6c6f", "decode": "hex"},
                    })); return 0
                decoded = raw.decode("utf-8", errors="replace")
            else:  # url
                decoded = urllib.parse.unquote(text)

            print(json.dumps({"decoded": decoded, "mode": mode}, indent=2, default=str)); return 0

        # --- hash + encode mode ---
        data = text.encode("utf-8")
        result = {
            "hashes": {
                "md5": hashlib.md5(data).hexdigest(),
                "sha1": hashlib.sha1(data).hexdigest(),
                "sha256": hashlib.sha256(data).hexdigest(),
                "sha512": hashlib.sha512(data).hexdigest(),
            },
            "encodings": {
                "base64": base64.b64encode(data).decode("ascii"),
                "hex": data.hex(),
                "url": urllib.parse.quote(text),
            },
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "hash failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
