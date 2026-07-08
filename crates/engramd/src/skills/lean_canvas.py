#!/usr/bin/env python3
"""lean_canvas — Engram skill (no network).

Builds an Ash Maurya Lean Canvas (the startup-focused cousin of the Business
Model Canvas) from whatever blocks you already have. Blocks you provide are
echoed back as filled; blocks you omit come back with guiding questions.

Request (stdin): {"problem"?: [str], "customer_segments"?: [str],
  "unique_value_proposition"?: [str], "solution"?: [str], "channels"?: [str],
  "revenue_streams"?: [str], "cost_structure"?: [str], "key_metrics"?: [str],
  "unfair_advantage"?: [str]}
Output (stdout): {<block>: {status, items|prompt}, ..., completeness_pct}
"""
import json
import sys

BLOCKS = [
    ("problem",
     "List your top 1-3 problems. What existing alternatives do people use "
     "today to solve these problems?"),
    ("customer_segments",
     "List your target customers and users. Who are your early adopters — "
     "the ideal customers who need this most and are easiest to reach?"),
    ("unique_value_proposition",
     "A single, clear, compelling message that states why you are "
     "different and worth paying attention to."),
    ("solution",
     "Outline a possible solution for each of your top problems — keep it "
     "minimal, this is not the full feature list."),
    ("channels",
     "List your path to customers, both inbound (content, SEO, referral) "
     "and outbound (sales, ads, partnerships)."),
    ("revenue_streams",
     "List your revenue model, pricing, life-time value, and gross "
     "margin."),
    ("cost_structure",
     "List your customer acquisition costs, distribution costs, hosting, "
     "people — separate fixed costs from variable costs."),
    ("key_metrics",
     "List the key numbers that tell you how your business is doing — the "
     "handful of activities you actually measure."),
    ("unfair_advantage",
     "Something that cannot be easily copied or bought — not simply a "
     "feature or a big head start."),
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
            "example": {"problem": ["No easy way to track team OKRs"]},
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
