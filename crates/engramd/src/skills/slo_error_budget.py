#!/usr/bin/env python3
"""slo_error_budget — Engram skill (no network). SLO error-budget math.

Given an SLO target and a window, computes the allowed downtime and (if you
supply request counts) the allowed failures, how much of the error budget is
consumed, how much remains, and a health status. This is standard SRE
error-budget arithmetic. Stdlib only.

Request (stdin): {"slo_target_pct": 99.9, "window_days": 30,
                  "total_requests": 1000000, "failed_requests": 500}
Output (stdout): {slo_target_pct, window_days, error_budget_pct,
                  allowed_downtime_minutes, allowed_failures, budget_consumed_pct,
                  budget_remaining_pct, status}
"""
import json
import sys


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"slo_target_pct": 99.9, "window_days": 30,
                        "total_requests": 1000000, "failed_requests": 500},
        }))
        return 0

    target = q.get("slo_target_pct")
    if target is None:
        print(json.dumps({
            "error": "missing required field: slo_target_pct",
            "example": {"slo_target_pct": 99.9, "window_days": 30},
        }))
        return 0

    try:
        target = float(target)
        if not (0 < target <= 100):
            print(json.dumps({
                "error": "'slo_target_pct' must be in (0, 100]",
                "example": {"slo_target_pct": 99.9, "window_days": 30},
            }))
            return 0

        window_days = float(q.get("window_days") or 30)
        if window_days <= 0:
            window_days = 30.0

        error_budget_frac = 1.0 - target / 100.0
        error_budget_pct = error_budget_frac * 100.0
        allowed_downtime_minutes = window_days * 1440.0 * error_budget_frac

        result = {
            "slo_target_pct": target,
            "window_days": window_days,
            "error_budget_pct": round(error_budget_pct, 6),
            "allowed_downtime_minutes": round(allowed_downtime_minutes, 4),
            "allowed_downtime_hours": round(allowed_downtime_minutes / 60.0, 4),
        }

        total = q.get("total_requests")
        failed = q.get("failed_requests")
        current_burn_rate = q.get("current_burn_rate")

        if total is not None:
            total = float(total)
            if total < 0:
                raise ValueError("'total_requests' must be non-negative")
            allowed_failures = total * error_budget_frac
            result["allowed_failures"] = round(allowed_failures, 4)

            if failed is not None:
                failed = float(failed)
                if failed < 0:
                    raise ValueError("'failed_requests' must be non-negative")
                result["failed_requests"] = failed
                if allowed_failures > 0:
                    consumed = failed / allowed_failures * 100.0
                else:
                    # zero error budget: any failure exhausts it
                    consumed = 0.0 if failed == 0 else float("inf")
                remaining = 100.0 - consumed
                result["budget_consumed_pct"] = (
                    round(consumed, 4) if consumed != float("inf") else "inf")
                result["budget_remaining_pct"] = round(remaining, 4)
                result["current_success_pct"] = (
                    round((1.0 - failed / total) * 100.0, 6) if total > 0 else None)

                if consumed >= 100.0:
                    status = "exhausted"
                elif consumed >= 75.0:
                    status = "at risk"
                else:
                    status = "healthy"
                result["status"] = status

        if current_burn_rate is not None:
            burn = float(current_burn_rate)
            result["current_burn_rate"] = burn
            # burn rate = how fast budget is being spent vs. sustainable (1x)
            if burn >= 10:
                bnote = "burn_rate >= 10x: page now — budget gone in hours."
            elif burn > 1:
                bnote = "burn_rate > 1x: spending faster than sustainable."
            elif burn == 1:
                bnote = "burn_rate == 1x: budget spent exactly over the window."
            else:
                bnote = "burn_rate < 1x: spending slower than budget allows."
            result["burn_rate_note"] = bnote

        result["note"] = ("error_budget = 1 - SLO. allowed_downtime = "
                          "window_days * 1440 * error_budget. Spend the budget "
                          "deliberately on releases and risk; freeze changes when "
                          "it's exhausted.")

        print(json.dumps(result, indent=2, default=str))
        return 0
    except (TypeError, ValueError) as e:
        print(json.dumps({"error": "invalid numeric input: %s" % e}))
        return 0
    except Exception as e:
        print(json.dumps({"error": "slo_error_budget failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
