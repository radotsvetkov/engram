#!/usr/bin/env python3
"""availability_calc — Engram skill (no network). SLA "nines" and downtime math.

Converts between availability percentage and allowed downtime, and composes the
availability of multi-component systems in series (all must be up) or parallel
(redundant, any one up). Uses a 30-day month and a 525600-minute year. Pure
arithmetic, stdlib only.

Request (stdin): {"action": "nines_to_downtime", "availability_pct": 99.9}
                 {"action": "downtime_to_nines", "downtime_minutes_per_year": 525.6}
                 {"action": "combine", "components": [99.9, 99.95], "mode": "series"}
Output (stdout): {action, availability_pct, downtime, ...}
"""
import json
import math
import sys

MIN_PER_DAY = 1440.0
MIN_PER_MONTH = 30.0 * 1440.0        # 43200
MIN_PER_YEAR = 525600.0


def _downtime_breakdown(avail_pct):
    unavail = 1.0 - avail_pct / 100.0
    if unavail < 0:
        unavail = 0.0
    return {
        "per_day_minutes": round(unavail * MIN_PER_DAY, 4),
        "per_month_minutes": round(unavail * MIN_PER_MONTH, 4),
        "per_year_minutes": round(unavail * MIN_PER_YEAR, 4),
        "per_year_hours": round(unavail * MIN_PER_YEAR / 60.0, 4),
    }


def _number_of_nines(avail_pct):
    unavail_frac = 1.0 - avail_pct / 100.0
    if unavail_frac <= 0:
        return None  # "perfect" — infinite nines
    return round(-math.log10(unavail_frac), 3)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"action": "nines_to_downtime", "availability_pct": 99.9},
        }))
        return 0

    action = str(q.get("action") or "").strip().lower()
    valid = ("nines_to_downtime", "downtime_to_nines", "combine")
    if action not in valid:
        print(json.dumps({
            "error": "missing/invalid 'action'; must be one of %s" % list(valid),
            "example": {"action": "nines_to_downtime", "availability_pct": 99.9},
        }))
        return 0

    try:
        if action == "nines_to_downtime":
            avail = q.get("availability_pct")
            if avail is None:
                print(json.dumps({
                    "error": "nines_to_downtime requires 'availability_pct'",
                    "example": {"action": "nines_to_downtime",
                                "availability_pct": 99.9},
                }))
                return 0
            avail = float(avail)
            if not (0 < avail <= 100):
                print(json.dumps({
                    "error": "'availability_pct' must be in (0, 100]",
                    "example": {"action": "nines_to_downtime",
                                "availability_pct": 99.9},
                }))
                return 0
            result = {
                "action": action,
                "availability_pct": avail,
                "number_of_nines": _number_of_nines(avail),
                "downtime": _downtime_breakdown(avail),
            }

        elif action == "downtime_to_nines":
            dt = q.get("downtime_minutes_per_year")
            if dt is None:
                print(json.dumps({
                    "error": "downtime_to_nines requires "
                             "'downtime_minutes_per_year'",
                    "example": {"action": "downtime_to_nines",
                                "downtime_minutes_per_year": 525.6},
                }))
                return 0
            dt = float(dt)
            if dt < 0 or dt > MIN_PER_YEAR:
                print(json.dumps({
                    "error": "'downtime_minutes_per_year' must be in [0, %g]"
                             % MIN_PER_YEAR,
                    "example": {"action": "downtime_to_nines",
                                "downtime_minutes_per_year": 525.6},
                }))
                return 0
            avail = (1.0 - dt / MIN_PER_YEAR) * 100.0
            result = {
                "action": action,
                "downtime_minutes_per_year": dt,
                "availability_pct": round(avail, 6),
                "number_of_nines": _number_of_nines(avail),
                "downtime": _downtime_breakdown(avail),
            }

        else:  # combine
            comps = q.get("components")
            mode = str(q.get("mode") or "series").strip().lower()
            if not isinstance(comps, list) or len(comps) == 0:
                print(json.dumps({
                    "error": "combine requires a non-empty 'components' list of "
                             "availability percentages",
                    "example": {"action": "combine",
                                "components": [99.9, 99.95], "mode": "series"},
                }))
                return 0
            if mode not in ("series", "parallel"):
                print(json.dumps({
                    "error": "'mode' must be 'series' or 'parallel'",
                    "example": {"action": "combine",
                                "components": [99.9, 99.95], "mode": "series"},
                }))
                return 0
            fracs = []
            for c in comps:
                cf = float(c) / 100.0
                if not (0 <= cf <= 1):
                    print(json.dumps({
                        "error": "each component must be a percentage in [0, 100]",
                        "example": {"action": "combine",
                                    "components": [99.9, 99.95]},
                    }))
                    return 0
                fracs.append(cf)

            if mode == "series":
                composite = 1.0
                for f in fracs:
                    composite *= f
                note = ("Series: every component must be up, so availability is "
                        "the product — adding components can only lower it.")
            else:  # parallel
                prod_down = 1.0
                for f in fracs:
                    prod_down *= (1.0 - f)
                composite = 1.0 - prod_down
                note = ("Parallel/redundant: system is up if any component is up, "
                        "so availability rises with more redundancy.")

            composite_pct = composite * 100.0
            result = {
                "action": action,
                "mode": mode,
                "components_pct": [float(c) for c in comps],
                "composite_availability_pct": round(composite_pct, 6),
                "composite_number_of_nines": _number_of_nines(composite_pct),
                "composite_downtime": _downtime_breakdown(composite_pct),
                "note": note,
            }

        print(json.dumps(result, indent=2, default=str))
        return 0
    except (TypeError, ValueError) as e:
        print(json.dumps({"error": "invalid numeric input: %s" % e}))
        return 0
    except Exception as e:
        print(json.dumps({"error": "availability_calc failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
