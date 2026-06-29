#!/usr/bin/env python3
"""crypto — Engram skill (keyless). Current cryptocurrency prices.

Uses CoinGecko's free, keyless API. Stdlib only.

Request (stdin): {"ids": ["bitcoin", "eth"], "vs": "usd"}
  - accepts common tickers (btc, eth, sol, ...) or CoinGecko ids.
Output (stdout): {vs, prices: {id: {price, change_24h}}}
"""
import json
import sys
import urllib.parse
import urllib.request

TIMEOUT = 20
ALIAS = {
    "btc": "bitcoin", "eth": "ethereum", "sol": "solana", "ada": "cardano",
    "xrp": "ripple", "doge": "dogecoin", "dot": "polkadot", "ltc": "litecoin",
    "bnb": "binancecoin", "matic": "matic-network", "avax": "avalanche-2",
    "link": "chainlink", "usdt": "tether", "usdc": "usd-coin", "ton": "the-open-network",
}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    raw = q.get("ids") or q.get("id") or ["bitcoin", "ethereum"]
    if isinstance(raw, str):
        raw = [s.strip() for s in raw.replace(",", " ").split() if s.strip()]
    ids = [ALIAS.get(str(x).lower().strip(), str(x).lower().strip()) for x in raw]
    vs = (q.get("vs") or q.get("currency") or "usd").lower()
    url = "https://api.coingecko.com/api/v3/simple/price?" + urllib.parse.urlencode(
        {"ids": ",".join(ids), "vs_currencies": vs, "include_24hr_change": "true"})
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "engram-crypto/1"})
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            data = json.loads(r.read().decode("utf-8", "replace"))
    except Exception as e:
        print(json.dumps({"error": "price lookup failed: %s" % e}))
        return 1
    if not data:
        print(json.dumps({"error": "no prices found", "tried": ids,
                          "hint": "use a CoinGecko id like 'bitcoin' or a ticker like 'btc'"}))
        return 0
    prices = {}
    for cid, v in data.items():
        prices[cid] = {"price": v.get(vs), "change_24h_pct": round(v.get(vs + "_24h_change", 0) or 0, 2)}
    print(json.dumps({"vs": vs.upper(), "prices": prices}, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
