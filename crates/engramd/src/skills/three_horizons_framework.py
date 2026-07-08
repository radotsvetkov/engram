#!/usr/bin/env python3
"""three_horizons_framework — Engram skill (no network).

Classifies a list of initiatives into McKinsey's Three Horizons of Growth:
Horizon 1 (core business — extend & defend), Horizon 2 (building emerging
business — medium-term bets), Horizon 3 (creating genuinely new/viable
options — long-shot, high-risk bets). Classification is a transparent
heuristic over `time_to_revenue_months` and `risk_level` (plus a small
keyword scan of `description` for H3 innovation signals), not a model
judgment — each initiative's `reasoning` field explains exactly which rule
fired. Also reports the count-based portfolio split against the classic
70/20/10 (H1/H2/H3) rule of thumb. Stdlib only.

Request (stdin): {"initiatives": [{"name": str, "description"?: str,
  "time_to_revenue_months"?: number, "risk_level"?: "low"|"medium"|"high"}]}
Output (stdout): {horizon_1, horizon_2, horizon_3, portfolio_balance_note}
"""
import json
import sys

_H3_KEYWORDS = ["new market", "emerging", "experimental", "unproven"]

_VALID_RISK = {"low", "medium", "high"}


def _classify(item):
    name = item["name"]
    description = (item.get("description") or "")
    desc_lower = description.lower()
    ttr = item.get("time_to_revenue_months")
    risk = item.get("risk_level")
    risk = risk.strip().lower() if isinstance(risk, str) else None
    if risk not in _VALID_RISK:
        risk = None

    has_ttr = isinstance(ttr, (int, float)) and not isinstance(ttr, bool)

    # Horizon 1: core business, extend/defend.
    if has_ttr and ttr <= 6:
        return "horizon_1", (
            "time_to_revenue_months=%s (<= 6 months) signals a near-term, "
            "core-business extension." % ttr
        )
    if risk == "low":
        return "horizon_1", "risk_level='low' signals a known, defensible core-business bet."

    # Horizon 3: genuinely new/emerging options.
    if has_ttr and ttr > 24:
        return "horizon_3", (
            "time_to_revenue_months=%s (> 24 months) signals a long-horizon, "
            "speculative option." % ttr
        )
    if risk == "high":
        hit = next((kw for kw in _H3_KEYWORDS if kw in desc_lower), None)
        if hit:
            return "horizon_3", (
                "risk_level='high' combined with innovation-signal keyword "
                "'%s' in the description signals a genuinely new, unproven bet." % hit
            )

    # Horizon 2: default — building emerging business, medium-term.
    reason_bits = []
    if has_ttr:
        reason_bits.append("time_to_revenue_months=%s (between 6 and 24)" % ttr)
    if risk:
        reason_bits.append("risk_level='%s'" % risk)
    if not reason_bits:
        reason_bits.append("no strong Horizon 1 or Horizon 3 signal from the fields given")
    return "horizon_2", (
        "No strong Horizon 1 or Horizon 3 signal (%s) — defaults to Horizon 2, "
        "a medium-term emerging-business bet." % "; ".join(reason_bits)
    )


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {
        "initiatives": [
            {"name": "Add SSO to core product", "risk_level": "low", "time_to_revenue_months": 3},
            {"name": "Launch adjacent product line", "time_to_revenue_months": 12, "risk_level": "medium"},
            {"name": "Explore an emerging new market", "risk_level": "high",
             "description": "experimental, unproven new market bet", "time_to_revenue_months": 30},
        ]
    }

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    initiatives = q.get("initiatives")
    if not isinstance(initiatives, list) or not initiatives:
        print(json.dumps({
            "error": "missing required field 'initiatives' (non-empty list of objects with at least 'name')",
            "example": example,
        }))
        return 0

    cleaned = []
    for i, item in enumerate(initiatives):
        if not isinstance(item, dict) or not str(item.get("name") or "").strip():
            print(json.dumps({
                "error": "initiatives[%d] must be an object with a non-empty 'name'" % i,
                "example": example,
            }))
            return 0
        entry = dict(item)
        entry["name"] = str(item["name"]).strip()
        cleaned.append(entry)

    try:
        buckets = {"horizon_1": [], "horizon_2": [], "horizon_3": []}
        for item in cleaned:
            horizon, reasoning = _classify(item)
            buckets[horizon].append({"name": item["name"], "reasoning": reasoning})

        total = len(cleaned)
        h1_pct = round(len(buckets["horizon_1"]) / total * 100, 1)
        h2_pct = round(len(buckets["horizon_2"]) / total * 100, 1)
        h3_pct = round(len(buckets["horizon_3"]) / total * 100, 1)

        split_summary = (
            "Portfolio split by initiative count: Horizon 1 %.1f%%, Horizon 2 %.1f%%, "
            "Horizon 3 %.1f%% (classic rule of thumb is roughly 70/20/10)." % (h1_pct, h2_pct, h3_pct)
        )
        if h1_pct > 70 and (h2_pct + h3_pct) < 30:
            balance_note = split_summary + (
                " Your portfolio may be too focused on defending the core — consider "
                "more H2/H3 bets for future growth."
            )
        elif h3_pct > 20 or h1_pct < 50:
            balance_note = split_summary + (
                " Too many H3/too few H1 bets risks starving the core business that "
                "funds everything else."
            )
        else:
            balance_note = split_summary + " This is roughly in line with a healthy, balanced portfolio."

        result = {
            "horizon_1": buckets["horizon_1"],
            "horizon_2": buckets["horizon_2"],
            "horizon_3": buckets["horizon_3"],
            "portfolio_balance_note": balance_note,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "three_horizons_framework failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
