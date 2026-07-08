#!/usr/bin/env python3
"""tam_sam_som — Engram skill (no network).

Computes Total/Serviceable/Obtainable market sizing (TAM/SAM/SOM), either
from a direct TAM figure or derived from population size times average
annual spend, plus a plain-English narrative summary.

Request (stdin): {"tam": 50000000000, "sam_pct_of_tam": 20, "som_pct_of_sam": 5}
              or: {"total_population": 10000000, "avg_annual_spend": 500, "sam_pct_of_tam": 20, "som_pct_of_sam": 5}
Output (stdout): {tam, sam, som, narrative}
"""
import json
import sys


def _money(v):
    return "${:,.2f}".format(v)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example_1 = {"tam": 50000000000, "sam_pct_of_tam": 20, "som_pct_of_sam": 5}
    example_2 = {"total_population": 10000000, "avg_annual_spend": 500, "sam_pct_of_tam": 20, "som_pct_of_sam": 5}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example_1": example_1, "example_2": example_2}))
        return 0

    sam_pct_of_tam = q.get("sam_pct_of_tam")
    som_pct_of_sam = q.get("som_pct_of_sam")

    if not isinstance(sam_pct_of_tam, (int, float)) or isinstance(sam_pct_of_tam, bool):
        print(json.dumps({
            "error": "missing or invalid required field 'sam_pct_of_tam' (percent, 0-100)",
            "example_1": example_1, "example_2": example_2,
        }))
        return 0

    if not isinstance(som_pct_of_sam, (int, float)) or isinstance(som_pct_of_sam, bool):
        print(json.dumps({
            "error": "missing or invalid required field 'som_pct_of_sam' (percent, 0-100)",
            "example_1": example_1, "example_2": example_2,
        }))
        return 0

    tam = q.get("tam")
    total_population = q.get("total_population")
    avg_annual_spend = q.get("avg_annual_spend")

    if isinstance(tam, (int, float)) and not isinstance(tam, bool):
        pass
    elif (
        isinstance(total_population, (int, float)) and not isinstance(total_population, bool)
        and isinstance(avg_annual_spend, (int, float)) and not isinstance(avg_annual_spend, bool)
    ):
        tam = total_population * avg_annual_spend
    else:
        print(json.dumps({
            "error": "provide either 'tam' directly, or both 'total_population' and 'avg_annual_spend'",
            "example_1": example_1, "example_2": example_2,
        }))
        return 0

    try:
        sam = tam * sam_pct_of_tam / 100.0
        som = sam * som_pct_of_sam / 100.0

        narrative = (
            "Your Total Addressable Market is %s. Your realistic Serviceable Addressable Market "
            "(%.1f%% of TAM) is %s. Your Serviceable Obtainable Market in year 1-3 (%.1f%% of SAM) is %s "
            "— that's the revenue ceiling a focused go-to-market can realistically capture."
            % (_money(tam), sam_pct_of_tam, _money(sam), som_pct_of_sam, _money(som))
        )

        result = {
            "tam": round(tam, 2),
            "sam": round(sam, 2),
            "som": round(som, 2),
            "sam_pct_of_tam": sam_pct_of_tam,
            "som_pct_of_sam": som_pct_of_sam,
            "narrative": narrative,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "tam_sam_som failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
