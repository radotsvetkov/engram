#!/usr/bin/env python3
"""swot_analysis — Engram skill (no network).

Builds a SWOT analysis and cross-references the quadrants into a TOWS matrix
(SO/WO/ST/WT strategies). Strategies are templated scaffolding for you to
refine, not generated insight — they only appear where both source
quadrants have at least one item, capped at 6 pairs per category.

Request (stdin): {"strengths"?: [str], "weaknesses"?: [str],
  "opportunities"?: [str], "threats"?: [str]}
Output (stdout): {strengths, weaknesses, opportunities, threats, tows_matrix}
"""
import json
import sys

QUADRANTS = [
    ("strengths",
     "What advantages do you have? What do you do well? What unique "
     "resources can you draw on that others can't?"),
    ("weaknesses",
     "What could you improve? Where do you have fewer resources than "
     "others? What are others likely to see as weaknesses?"),
    ("opportunities",
     "What opportunities are open to you? What trends could you take "
     "advantage of? How can you turn strengths into opportunities?"),
    ("threats",
     "What threats could harm you? What is your competition doing? What "
     "threats do your weaknesses expose you to?"),
]


SINGULAR = {
    "strengths": "strength",
    "weaknesses": "weakness",
    "opportunities": "opportunity",
    "threats": "threat",
}


def _pairs(a, b, cap_a=2, cap_b=3):
    out = []
    for x in a[:cap_a]:
        for y in b[:cap_b]:
            out.append((x, y))
    return out


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"strengths": ["Strong brand"], "opportunities": ["New market opening"]},
        }))
        return 0

    result = {}
    values = {}
    for key, prompt in QUADRANTS:
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
        values[key] = items
        if items:
            result[key] = {"status": "filled", "items": items}
        else:
            result[key] = {"status": "empty", "prompt": prompt}

    tows = {}

    def add_category(cat_key, label, a_key, b_key, template):
        a, b = values[a_key], values[b_key]
        if a and b:
            strategies = [template.format(**{SINGULAR[a_key]: x, SINGULAR[b_key]: y})
                          for x, y in _pairs(a, b)]
            tows[cat_key] = {"label": label, "strategies": strategies, "count": len(strategies)}
        else:
            tows[cat_key] = {
                "note": "provide both %s and %s to generate %s strategies" % (a_key, b_key, label)
            }

    add_category("SO_strategies", "SO", "strengths", "opportunities",
                  "Leverage '{strength}' to capture '{opportunity}'")
    add_category("WO_strategies", "WO", "weaknesses", "opportunities",
                  "Overcome '{weakness}' by leveraging '{opportunity}'")
    add_category("ST_strategies", "ST", "strengths", "threats",
                  "Use '{strength}' to defend against '{threat}'")
    add_category("WT_strategies", "WT", "weaknesses", "threats",
                  "Minimize '{weakness}' to reduce exposure to '{threat}'")

    result["tows_matrix"] = tows
    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
