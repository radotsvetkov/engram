#!/usr/bin/env python3
"""translate — Engram skill (keyless). Translate text between languages.

Keyless via MyMemory (api.mymemory.translated.net); uses DeepL instead when
DEEPL_API_KEY is set. Request (stdin): {"text": "...", "to": "fr", "from": "en"}.
'to'/'from' are ISO language codes (fr/de/es/...); 'from' defaults to "en".
Output (stdout): {translated, from, to, provider, match?}.
"""
import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request

TIMEOUT = 20


def _deepl(text, src, tgt, key):
    """Translate via DeepL free API. Returns (translated, detected_source)."""
    data = {"auth_key": key, "text": text, "target_lang": tgt.upper()}
    if src:
        data["source_lang"] = src.upper()
    body = urllib.parse.urlencode(data).encode("utf-8")
    req = urllib.request.Request(
        "https://api-free.deepl.com/v2/translate",
        data=body,
        headers={
            "User-Agent": "engram-translate/1",
            "Content-Type": "application/x-www-form-urlencoded",
        },
    )
    with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
        resp = json.loads(r.read().decode("utf-8", "replace"))
    trans = (resp.get("translations") or [{}])[0]
    return trans.get("text", ""), (trans.get("detected_source_language") or "").lower()


def _mymemory(text, src, tgt):
    """Translate via keyless MyMemory API. Returns (translated, match)."""
    qs = urllib.parse.urlencode({"q": text, "langpair": src + "|" + tgt})
    url = "https://api.mymemory.translated.net/get?" + qs
    req = urllib.request.Request(url, headers={"User-Agent": "engram-translate/1"})
    with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
        resp = json.loads(r.read().decode("utf-8", "replace"))
    rd = resp.get("responseData") or {}
    return rd.get("translatedText", ""), rd.get("match")


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    text = (q.get("text") or "").strip()
    tgt = (q.get("to") or "").strip()
    if not text or not tgt:
        print(json.dumps({
            "error": "provide 'text' and 'to' (ISO lang code)",
            "example": {"text": "Hello, world", "to": "fr", "from": "en"},
        }))
        return 0
    src = (q.get("from") or "en").strip() or "en"

    key = os.environ.get("DEEPL_API_KEY")
    try:
        if key:
            translated, detected = _deepl(text, src, tgt, key)
            out = {
                "translated": translated,
                "from": detected or src,
                "to": tgt,
                "provider": "deepl",
            }
        else:
            translated, match = _mymemory(text, src, tgt)
            out = {
                "translated": translated,
                "from": src,
                "to": tgt,
                "provider": "mymemory",
            }
            if match is not None:
                out["match"] = match
        if not translated:
            out["warning"] = "empty translation returned"
        print(json.dumps(out, indent=2, default=str))
        return 0
    except urllib.error.HTTPError as e:
        prov = "deepl" if key else "mymemory"
        detail = ""
        try:
            detail = e.read().decode("utf-8", "replace")[:200]
        except Exception:
            pass
        print(json.dumps({"error": "translate failed (%s HTTP %s): %s" % (prov, e.code, detail)}))
        return 0
    except Exception as e:
        print(json.dumps({"error": "translate failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
