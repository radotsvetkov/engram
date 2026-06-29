#!/usr/bin/env python3
"""flight_search — Engram seed skill (Process/python3, capability: Net).

Reads ONE JSON request object from stdin and writes ONE JSON result object to
stdout. Stdlib only (urllib) so it runs with no `pip install` under the skill
sandbox. This is the structured-API answer to "why can't Engram find flights":
consumer metasearch sites (Google Flights / Skyscanner / Ryanair) are JS SPAs
behind bot detection, so scraping them fails — a flight DATA API does not.

Request (stdin), all fields optional except origin/destination:
    {
      "origin": "HAM",            # IATA city/airport code
      "destination": "TNG",
      "depart": "2026-07-09",     # YYYY-MM-DD or YYYY-MM (month search)
      "return": "2026-07-21",     # omit for one-way
      "adults": 1,
      "currency": "eur",
      "direct": false,            # true = nonstop only
      "limit": 30,
      "provider": "auto"          # auto | travelpayouts | amadeus
    }

Credentials come from the daemon's environment (a Process skill inherits it):
    TRAVELPAYOUTS_TOKEN   free affiliate token  -> https://www.travelpayouts.com
    AMADEUS_CLIENT_ID + AMADEUS_CLIENT_SECRET   -> https://developers.amadeus.com
    AMADEUS_ENV=production  (default: test sandbox, which returns limited data)

With no credentials the skill still SUCCEEDS (exit 0) and returns an actionable
"how_to_fix" payload rather than crashing — so the agent can relay it.
"""

import json
import os
import sys
import urllib.parse
import urllib.request

TIMEOUT = 20
UA = "engram-flight-search/1"


def _http_json(url, data=None, headers=None, method=None):
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("User-Agent", UA)
    req.add_header("Accept", "application/json")
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
        return json.loads(resp.read().decode("utf-8", "replace"))


def _month(date):
    """Travelpayouts accepts YYYY-MM or YYYY-MM-DD; pass through either."""
    return date or ""


# ---------------------------------------------------------------------------
# Provider: Travelpayouts / Aviasales Data API v3 (free, cached cheapest fares)
# ---------------------------------------------------------------------------
def travelpayouts(q, token):
    base = "https://api.travelpayouts.com/aviasales/v3/prices_for_dates"
    params = {
        "origin": q["origin"],
        "destination": q["destination"],
        "currency": (q.get("currency") or "eur").lower(),
        "sorting": "price",
        "direct": "true" if q.get("direct") else "false",
        "limit": int(q.get("limit") or 30),
        "page": 1,
        "one_way": "false" if q.get("return") else "true",
        "unique": "false",
        "token": token,
    }
    if q.get("depart"):
        params["departure_at"] = _month(q["depart"])
    if q.get("return"):
        params["return_at"] = _month(q["return"])
    url = base + "?" + urllib.parse.urlencode(params)
    raw = _http_json(url)
    if not raw.get("success", True) and "data" not in raw:
        raise RuntimeError("travelpayouts error: %s" % json.dumps(raw)[:300])
    out = []
    for r in raw.get("data", []):
        link = r.get("link") or ""
        if link.startswith("/"):
            link = "https://www.aviasales.com" + link
        out.append(
            {
                "price": r.get("price"),
                "currency": params["currency"].upper(),
                "airline": r.get("airline"),
                "flight_number": r.get("flight_number"),
                "stops": r.get("transfers"),
                "direct": (r.get("transfers") == 0),
                "depart_at": r.get("departure_at"),
                "return_at": r.get("return_at"),
                "duration_min": r.get("duration"),
                "deep_link": link,
            }
        )
    return out


# ---------------------------------------------------------------------------
# Provider: Amadeus Self-Service (OAuth2 client_credentials, real-time offers)
# ---------------------------------------------------------------------------
def amadeus(q, client_id, client_secret):
    host = (
        "https://api.amadeus.com"
        if os.environ.get("AMADEUS_ENV", "test").lower() == "production"
        else "https://test.api.amadeus.com"
    )
    body = urllib.parse.urlencode(
        {
            "grant_type": "client_credentials",
            "client_id": client_id,
            "client_secret": client_secret,
        }
    ).encode()
    tok = _http_json(
        host + "/v1/security/oauth2/token",
        data=body,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
        method="POST",
    )
    access = tok["access_token"]
    params = {
        "originLocationCode": q["origin"],
        "destinationLocationCode": q["destination"],
        "adults": int(q.get("adults") or 1),
        "currencyCode": (q.get("currency") or "EUR").upper(),
        "max": int(q.get("limit") or 20),
        "nonStop": "true" if q.get("direct") else "false",
    }
    if q.get("depart"):
        params["departureDate"] = q["depart"]
    if q.get("return"):
        params["returnDate"] = q["return"]
    url = host + "/v2/shopping/flight-offers?" + urllib.parse.urlencode(params)
    raw = _http_json(url, headers={"Authorization": "Bearer " + access})
    out = []
    for offer in raw.get("data", []):
        price = offer.get("price", {})
        itins = offer.get("itineraries", [])
        first = itins[0]["segments"] if itins else []
        carriers = sorted({s["carrierCode"] for it in itins for s in it["segments"]})
        out.append(
            {
                "price": float(price.get("grandTotal", price.get("total", 0)) or 0),
                "currency": price.get("currency", params["currencyCode"]),
                "airline": ",".join(carriers),
                "stops": (len(first) - 1) if first else None,
                "direct": (len(first) == 1) if first else None,
                "depart_at": first[0]["departure"]["at"] if first else None,
                "return_at": (
                    itins[1]["segments"][0]["departure"]["at"]
                    if len(itins) > 1 and itins[1]["segments"]
                    else None
                ),
                "deep_link": "https://www.amadeus.com",
            }
        )
    out.sort(key=lambda r: r.get("price") or 1e12)
    return out


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not q.get("origin") or not q.get("destination"):
        print(
            json.dumps(
                {
                    "error": "origin and destination (IATA codes) are required",
                    "example": {"origin": "HAM", "destination": "TNG",
                                "depart": "2026-07-09", "return": "2026-07-21"},
                }
            )
        )
        return 0

    pref = (q.get("provider") or "auto").lower()
    tp = os.environ.get("TRAVELPAYOUTS_TOKEN") or os.environ.get("AVIASALES_TOKEN")
    am_id = os.environ.get("AMADEUS_CLIENT_ID")
    am_sec = os.environ.get("AMADEUS_CLIENT_SECRET")

    order = []
    if pref == "travelpayouts":
        order = ["travelpayouts"]
    elif pref == "amadeus":
        order = ["amadeus"]
    else:  # auto: free cached fares first, then real-time if configured
        if tp:
            order.append("travelpayouts")
        if am_id and am_sec:
            order.append("amadeus")

    if not order:
        print(
            json.dumps(
                {
                    "error": "no flight provider credentials configured",
                    "how_to_fix": {
                        "travelpayouts (free, recommended first)": {
                            "env": "TRAVELPAYOUTS_TOKEN",
                            "signup": "https://www.travelpayouts.com/ (Tools > Data API)",
                        },
                        "amadeus (free dev tier, real-time)": {
                            "env": "AMADEUS_CLIENT_ID + AMADEUS_CLIENT_SECRET",
                            "signup": "https://developers.amadeus.com/register",
                        },
                    },
                    "note": "Set one of these in the Engram daemon's environment, then rerun.",
                },
                indent=2,
            )
        )
        return 0

    last_err = None
    for prov in order:
        try:
            if prov == "travelpayouts":
                results = travelpayouts(q, tp)
            else:
                results = amadeus(q, am_id, am_sec)
            results = [r for r in results if r.get("price")]
            results.sort(key=lambda r: r.get("price") or 1e12)
            cheapest = results[0] if results else None
            direct = [r for r in results if r.get("direct")]
            summary = _summary(q, prov, results, cheapest, direct)
            print(
                json.dumps(
                    {
                        "query": q,
                        "provider": prov,
                        "count": len(results),
                        "cheapest": cheapest,
                        "cheapest_direct": direct[0] if direct else None,
                        "results": results[: int(q.get("limit") or 30)],
                        "summary": summary,
                    },
                    indent=2,
                    default=str,
                )
            )
            return 0
        except Exception as e:  # try the next provider, remember the error
            last_err = "%s: %s" % (prov, e)

    print(json.dumps({"error": "all providers failed", "detail": last_err}))
    return 1


def _summary(q, prov, results, cheapest, direct):
    route = "%s->%s" % (q["origin"], q["destination"])
    if not results:
        return "No fares found for %s on %s (provider: %s)." % (
            route, q.get("depart", "flexible"), prov)
    parts = ["Cheapest %s: %s %s" % (route, cheapest["price"], cheapest["currency"])]
    if cheapest.get("airline"):
        parts.append("on %s" % cheapest["airline"])
    parts.append("(%s stop[s])" % cheapest.get("stops"))
    if direct:
        parts.append("| cheapest NONSTOP: %s %s" % (direct[0]["price"], direct[0]["currency"]))
    else:
        parts.append("| no nonstop option found")
    parts.append("| provider=%s" % prov)
    return " ".join(str(p) for p in parts)


if __name__ == "__main__":
    sys.exit(main())
