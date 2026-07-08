#!/usr/bin/env python3
"""north_star_metric — Engram skill (no network).

With no candidate given, explains the North Star Metric framework: the 4
criteria a good NSM should meet, plus example NSMs by business type. Given a
candidate_metric (and optional description), scores it against those same 4
criteria with a keyword-heuristic pass/warn/fail per criterion.

Request (stdin): {"candidate_metric"?: str, "description"?: str}
Output (stdout): no candidate -> {criteria, examples}; with candidate ->
  {candidate_metric, description, scores: [{criterion, verdict, note}], verdict}
"""
import json
import sys

CRITERIA = [
    {"id": 1, "criterion": "Reflects customer value received",
     "explanation": "It should measure the value customers actually get, not just money "
                     "flowing to the company."},
    {"id": 2, "criterion": "Leading indicator of revenue/business success",
     "explanation": "It should predict future revenue/growth, not just report it after "
                     "the fact."},
    {"id": 3, "criterion": "Understandable by everyone in the company",
     "explanation": "Anyone, from engineering to sales, should be able to grasp it "
                     "without a glossary."},
    {"id": 4, "criterion": "Actionable — teams can influence it",
     "explanation": "Teams should be able to move the number through their own work, "
                     "not just watch it happen."},
]

EXAMPLES = [
    {"company_type": "Marketplace (e.g. Airbnb, Uber)", "example_metric": "Weekly transacting buyers"},
    {"company_type": "Social / content platform (e.g. Instagram, YouTube)",
     "example_metric": "Weekly active users creating content"},
    {"company_type": "SaaS / productivity (e.g. Slack, Asana)",
     "example_metric": "Weekly active teams completing a core workflow"},
    {"company_type": "E-commerce", "example_metric": "Monthly repeat purchase rate"},
]

REVENUE_WORDS = {"revenue", "mrr", "arr", "sales", "profit", "income"}
VALUE_WORDS = {"value", "satisfaction", "success", "outcome", "completed", "active",
               "engaged", "retention", "habit", "benefit", "help", "solve", "solved"}
FREQUENCY_WORDS = {"weekly", "daily", "monthly", "active", "frequency", "recurring",
                    "habit", "repeat"}
MACRO_PHRASES = {"stock price", "market share", "gdp", "valuation"}
ACTIONABLE_WORDS = {"users", "customers", "teams", "complete", "creating", "purchase",
                     "transacting", "sessions", "tasks", "workflow", "active"}
COMPLEX_WORDS = {"ratio", "coefficient", "logarithm", "index"}


def _score(metric, description):
    text = ("%s %s" % (metric, description or "")).lower()
    scores = []

    has_revenue = any(w in text for w in REVENUE_WORDS)
    has_value = any(w in text for w in VALUE_WORDS)
    if has_revenue and not has_value:
        scores.append({"criterion": CRITERIA[0]["criterion"], "verdict": "fail",
                        "note": "reads as company revenue, not customer value received; "
                                "reframe around the value customers get."})
    elif has_value:
        scores.append({"criterion": CRITERIA[0]["criterion"], "verdict": "pass", "note": ""})
    else:
        scores.append({"criterion": CRITERIA[0]["criterion"], "verdict": "warn",
                        "note": "unclear whether this reflects customer value — clarify "
                                "the customer outcome it represents."})

    if any(w in text for w in FREQUENCY_WORDS):
        scores.append({"criterion": CRITERIA[1]["criterion"], "verdict": "pass", "note": ""})
    else:
        scores.append({"criterion": CRITERIA[1]["criterion"], "verdict": "warn",
                        "note": "consider whether this predicts future revenue/growth, "
                                "or only reports it after the fact."})

    word_count = len(metric.split())
    has_complex = any(w in text for w in COMPLEX_WORDS)
    if word_count <= 6 and not has_complex:
        scores.append({"criterion": CRITERIA[2]["criterion"], "verdict": "pass", "note": ""})
    else:
        scores.append({"criterion": CRITERIA[2]["criterion"], "verdict": "warn",
                        "note": "simplify the name/definition so any employee can grasp "
                                "it at a glance."})

    if any(p in text for p in MACRO_PHRASES):
        scores.append({"criterion": CRITERIA[3]["criterion"], "verdict": "fail",
                        "note": "this looks macro/uncontrollable — teams can't move it "
                                "through their own work."})
    elif any(w in text for w in ACTIONABLE_WORDS):
        scores.append({"criterion": CRITERIA[3]["criterion"], "verdict": "pass", "note": ""})
    else:
        scores.append({"criterion": CRITERIA[3]["criterion"], "verdict": "warn",
                        "note": "clarify which team activities move this number."})

    verdicts = [s["verdict"] for s in scores]
    if "fail" in verdicts:
        overall = "needs rework"
    elif "warn" in verdicts:
        overall = "solid but refine"
    else:
        overall = "strong candidate"
    return scores, overall


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"candidate_metric": "Weekly active teams", "description": "Teams completing core workflow"},
        }))
        return 0

    candidate_metric = q.get("candidate_metric")
    candidate_metric = candidate_metric.strip() if isinstance(candidate_metric, str) and candidate_metric.strip() else None
    description = q.get("description")
    description = description.strip() if isinstance(description, str) and description.strip() else None

    try:
        if not candidate_metric:
            result = {"criteria": CRITERIA, "examples": EXAMPLES}
            print(json.dumps(result, indent=2, default=str))
            return 0

        scores, overall = _score(candidate_metric, description)
        result = {
            "candidate_metric": candidate_metric,
            "description": description,
            "scores": scores,
            "verdict": overall,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "north_star_metric failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
