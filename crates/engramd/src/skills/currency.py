#!/usr/bin/env python3
"""currency — Engram skill (keyless). Convert money / get exchange rates.

Uses open.er-api.com (free, no key). Stdlib only.

Request (stdin): {"from": "USD", "to": "EUR", "amount": 100}
  - omit "to" to get all rates for the base currency.
Output (stdout): {from, to, rate, amount, converted} or {base, rates}.
"""
import json
import sys
import urllib.request

TIMEOUT = 20


def _get(url):
    req = urllib.request.Request(url, headers={"User-Agent": "engram-currency/1"})
    with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
        return json.loads(r.read().decode("utf-8", "replace"))


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    base = (q.get("from") or q.get("base") or "USD").strip().upper()
    to = (q.get("to") or "").strip().upper()
    try:
        amount = float(q.get("amount", 1) or 1)
    except Exception:
        amount = 1.0
    try:
        data = _get("https://open.er-api.com/v6/latest/" + base)
    except Exception as e:
        print(json.dumps({"error": "rate lookup failed: %s" % e}))
        return 1
    if data.get("result") != "success" or "rates" not in data:
        print(json.dumps({"error": "unknown base currency %r" % base}))
        return 0
    rates = data["rates"]
    if not to:
        print(json.dumps({"base": base, "updated": data.get("time_last_update_utc"),
                          "rates": rates}, indent=2, default=str))
        return 0
    if to not in rates:
        print(json.dumps({"error": "unknown target currency %r" % to}))
        return 0
    rate = rates[to]
    print(json.dumps({
        "from": base, "to": to, "rate": rate, "amount": amount,
        "converted": round(amount * rate, 4),
        "summary": "%s %s = %s %s (1 %s = %s %s)" % (amount, base, round(amount * rate, 2), to, base, rate, to),
    }, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
