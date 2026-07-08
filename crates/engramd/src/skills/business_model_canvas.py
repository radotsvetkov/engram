#!/usr/bin/env python3
"""business_model_canvas — Engram skill (no network).

Builds an Osterwalder Business Model Canvas from whatever blocks you already
have. Blocks you provide are echoed back as filled; blocks you omit come back
with guiding questions so you know what to fill in next.

Request (stdin): {"key_partners"?: [str], "key_activities"?: [str],
  "key_resources"?: [str], "value_propositions"?: [str],
  "customer_relationships"?: [str], "channels"?: [str],
  "customer_segments"?: [str], "cost_structure"?: [str],
  "revenue_streams"?: [str]}
Output (stdout): {<block>: {status, items|prompt}, ..., completeness_pct}
"""
import json
import sys

BLOCKS = [
    ("key_partners",
     "Who are your key partners and suppliers? Which key resources are you "
     "acquiring from partners? Which key activities do partners perform?"),
    ("key_activities",
     "What key activities does your value proposition require? Your "
     "distribution channels? Customer relationships? Revenue streams?"),
    ("key_resources",
     "What key resources does your value proposition require? Your "
     "distribution channels? Customer relationships? Revenue streams?"),
    ("value_propositions",
     "What value do you deliver to the customer? Which customer problems "
     "are you helping to solve? Which needs are you satisfying?"),
    ("customer_relationships",
     "What type of relationship does each customer segment expect you to "
     "establish? How costly are they? How are they integrated with the "
     "rest of your business model?"),
    ("channels",
     "Through which channels do your customer segments want to be "
     "reached? How are you reaching them now? Which channels work best "
     "and are most cost-efficient?"),
    ("customer_segments",
     "For whom are you creating value? Who are your most important "
     "customers? Are they a mass market, niche market, segmented, "
     "diversified, or multi-sided platform?"),
    ("cost_structure",
     "What are the most important costs inherent in your business model? "
     "Which key resources and activities are most expensive?"),
    ("revenue_streams",
     "For what value are your customers really willing to pay? For what "
     "do they currently pay? How are they currently paying? How would "
     "they prefer to pay?"),
]


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"value_propositions": ["Save time for busy founders"]},
        }))
        return 0

    result = {}
    filled = 0
    for key, prompt in BLOCKS:
        raw = q.get(key)
        if raw is None:
            raw = []
        if not isinstance(raw, list):
            print(json.dumps({
                "error": "'%s' must be a list of strings" % key,
                "example": {key: ["example item"]},
            }))
            return 0
        items = [str(x).strip() for x in raw if str(x).strip()]
        if items:
            filled += 1
            result[key] = {"status": "filled", "items": items}
        else:
            result[key] = {"status": "empty", "prompt": prompt}

    result["completeness_pct"] = round(filled / len(BLOCKS) * 100, 1)
    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
