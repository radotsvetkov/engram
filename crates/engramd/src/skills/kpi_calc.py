#!/usr/bin/env python3
"""kpi_calc — Engram skill (no network).

Computes one of four common business KPIs, selected via the "metric" field:
churn_rate, mrr_growth, nps (Net Promoter Score), or gross_margin.

Request (stdin): {"metric": "churn_rate", "customers_start": 1000, "customers_lost": 50}
              or: {"metric": "mrr_growth", "mrr_start": 10000, "mrr_end": 12000}
              or: {"metric": "nps", "promoters": 60, "passives": 30, "detractors": 10}
              or: {"metric": "nps", "scores": [9, 10, 7, 3, 8]}
              or: {"metric": "gross_margin", "revenue": 100000, "cogs": 40000}
Output (stdout): metric-specific fields, e.g. {metric, churn_rate_pct, retention_rate_pct}
"""
import json
import sys

EXAMPLES = {
    "churn_rate": {"metric": "churn_rate", "customers_start": 1000, "customers_lost": 50},
    "mrr_growth": {"metric": "mrr_growth", "mrr_start": 10000, "mrr_end": 12000},
    "nps": {"metric": "nps", "promoters": 60, "passives": 30, "detractors": 10, "_or": {"metric": "nps", "scores": [9, 10, 7, 3, 8]}},
    "gross_margin": {"metric": "gross_margin", "revenue": 100000, "cogs": 40000},
}


def _is_num(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "supported_metrics": EXAMPLES,
        }))
        return 0

    metric = q.get("metric")
    if metric not in EXAMPLES:
        print(json.dumps({
            "error": "missing or unknown 'metric'; supported metrics are: churn_rate, mrr_growth, nps, gross_margin",
            "supported_metrics": EXAMPLES,
        }))
        return 0

    try:
        if metric == "churn_rate":
            customers_start = q.get("customers_start")
            customers_lost = q.get("customers_lost")
            if not _is_num(customers_start) or not _is_num(customers_lost):
                print(json.dumps({
                    "error": "'churn_rate' requires numeric 'customers_start' and 'customers_lost'",
                    "example": EXAMPLES["churn_rate"],
                }))
                return 0
            if customers_start == 0:
                print(json.dumps({"error": "'customers_start' must not be 0"}))
                return 0
            churn_rate_pct = customers_lost / customers_start * 100.0
            result = {
                "metric": metric,
                "customers_start": customers_start,
                "customers_lost": customers_lost,
                "churn_rate_pct": round(churn_rate_pct, 2),
                "retention_rate_pct": round(100.0 - churn_rate_pct, 2),
            }

        elif metric == "mrr_growth":
            mrr_start = q.get("mrr_start")
            mrr_end = q.get("mrr_end")
            if not _is_num(mrr_start) or not _is_num(mrr_end):
                print(json.dumps({
                    "error": "'mrr_growth' requires numeric 'mrr_start' and 'mrr_end'",
                    "example": EXAMPLES["mrr_growth"],
                }))
                return 0
            if mrr_start == 0:
                print(json.dumps({"error": "'mrr_start' must not be 0"}))
                return 0
            mrr_growth_pct = (mrr_end - mrr_start) / mrr_start * 100.0
            result = {
                "metric": metric,
                "mrr_start": mrr_start,
                "mrr_end": mrr_end,
                "mrr_growth_pct": round(mrr_growth_pct, 2),
            }

        elif metric == "nps":
            scores = q.get("scores")
            if isinstance(scores, list) and len(scores) > 0:
                if not all(_is_num(s) and 0 <= s <= 10 for s in scores):
                    print(json.dumps({
                        "error": "'scores' must be a list of numbers between 0 and 10",
                        "example": EXAMPLES["nps"]["_or"],
                    }))
                    return 0
                promoters = sum(1 for s in scores if s >= 9)
                detractors = sum(1 for s in scores if s <= 6)
                passives = sum(1 for s in scores if 7 <= s <= 8)
            else:
                promoters = q.get("promoters")
                passives = q.get("passives")
                detractors = q.get("detractors")
                if not _is_num(promoters) or not _is_num(passives) or not _is_num(detractors):
                    print(json.dumps({
                        "error": "'nps' requires either numeric 'promoters'/'passives'/'detractors', or a 'scores' list (0-10)",
                        "example": EXAMPLES["nps"],
                        "example_alt": EXAMPLES["nps"]["_or"],
                    }))
                    return 0

            total = promoters + passives + detractors
            if total == 0:
                print(json.dumps({"error": "total respondents (promoters + passives + detractors) must not be 0"}))
                return 0

            nps = (promoters / total - detractors / total) * 100.0
            result = {
                "metric": metric,
                "promoters": promoters,
                "passives": passives,
                "detractors": detractors,
                "total": total,
                "nps": round(nps, 2),
            }

        else:  # gross_margin
            revenue = q.get("revenue")
            cogs = q.get("cogs")
            if not _is_num(revenue) or not _is_num(cogs):
                print(json.dumps({
                    "error": "'gross_margin' requires numeric 'revenue' and 'cogs'",
                    "example": EXAMPLES["gross_margin"],
                }))
                return 0
            if revenue == 0:
                print(json.dumps({"error": "'revenue' must not be 0"}))
                return 0
            gross_margin_pct = (revenue - cogs) / revenue * 100.0
            result = {
                "metric": metric,
                "revenue": revenue,
                "cogs": cogs,
                "gross_margin_pct": round(gross_margin_pct, 2),
            }

        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "kpi_calc failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
