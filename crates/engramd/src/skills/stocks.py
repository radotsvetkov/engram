#!/usr/bin/env python3
"""stocks — Engram skill (needs ALPHAVANTAGE_KEY). Real-time stock quote.

Looks up the latest price for a ticker via Alpha Vantage's GLOBAL_QUOTE endpoint.
Request (stdin): {"symbol": "AAPL"}
Output (stdout): {symbol, price, change, change_percent, volume, latest_day}.
With no ALPHAVANTAGE_KEY it returns an actionable {"error", "how_to_fix"} and
exits 0 (free key: https://www.alphavantage.co/support/#api-key).
"""
import json
import os
import sys
import urllib.parse
import urllib.request

TIMEOUT = 20
UA = "engram-stocks/1"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    symbol = q.get("symbol") or q.get("ticker") or q.get("s")
    if not symbol or not str(symbol).strip():
        print(json.dumps({
            "error": "symbol is required",
            "example": {"symbol": "AAPL"},
        }))
        return 0
    symbol = str(symbol).strip().upper()

    key = os.environ.get("ALPHAVANTAGE_KEY")
    if not key:
        print(json.dumps({
            "error": "no Alpha Vantage key configured",
            "how_to_fix": {
                "env": "ALPHAVANTAGE_KEY",
                "signup": "https://www.alphavantage.co/support/#api-key",
            },
        }))
        return 0

    url = "https://www.alphavantage.co/query?" + urllib.parse.urlencode({
        "function": "GLOBAL_QUOTE",
        "symbol": symbol,
        "apikey": key,
    })

    try:
        req = urllib.request.Request(url, headers={
            "User-Agent": UA,
            "Accept": "application/json",
        })
        with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
            data = json.loads(resp.read().decode("utf-8", "replace"))

        # Alpha Vantage signals rate limits / errors via these keys, not HTTP status.
        note = data.get("Note") or data.get("Information") or data.get("Error Message")
        quote = data.get("Global Quote") or data.get("globalQuote") or {}

        if not quote or not (quote.get("05. price") or quote.get("01. symbol")):
            err = "no quote for %s (or rate limited — free tier is 25/day)" % symbol
            out = {"error": err}
            if note:
                out["detail"] = note
            print(json.dumps(out))
            return 0

        result = {
            "symbol": quote.get("01. symbol", symbol),
            "price": quote.get("05. price"),
            "change": quote.get("09. change"),
            "change_percent": quote.get("10. change percent"),
            "volume": quote.get("06. volume"),
            "latest_day": quote.get("07. latest trading day"),
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "stocks failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
