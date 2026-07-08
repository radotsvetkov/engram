#!/usr/bin/env python3
"""core_web_vitals_grade — Engram skill (no network). Grade Core Web Vitals metrics.

Grades given metrics against Google's published 2024+ thresholds: LCP, INP,
and CLS are the three official Core Web Vitals (INP replaced FID as the
responsiveness metric in March 2024); FCP and TTFB are supplementary,
non-official metrics graded for extra context. At least one metric must be
given. Stdlib only, static reference thresholds.

Request (stdin): {"lcp_ms": 2100, "inp_ms": 150, "cls": 0.05, "fcp_ms": 1200, "ttfb_ms": 400}
Output (stdout): {metrics: {<key>: {label, value, grade, core_web_vital, note}}, core_web_vitals_pass, core_web_vitals_note?}
"""
import json
import sys

_METRICS = {
    "lcp_ms": {
        "label": "LCP", "good": 2500, "needs_improvement": 4000, "core": True,
        "note": "Largest Contentful Paint (ms) — loading performance. Official Core Web Vital.",
    },
    "inp_ms": {
        "label": "INP", "good": 200, "needs_improvement": 500, "core": True,
        "note": "Interaction to Next Paint (ms) — replaced FID as the official Core Web Vital for responsiveness in March 2024.",
    },
    "cls": {
        "label": "CLS", "good": 0.1, "needs_improvement": 0.25, "core": True,
        "note": "Cumulative Layout Shift (unitless score) — visual stability. Official Core Web Vital.",
    },
    "fcp_ms": {
        "label": "FCP", "good": 1800, "needs_improvement": 3000, "core": False,
        "note": "First Contentful Paint (ms) — supplementary metric, not an official Core Web Vital.",
    },
    "ttfb_ms": {
        "label": "TTFB", "good": 800, "needs_improvement": 1800, "core": False,
        "note": "Time to First Byte (ms) — supplementary metric, not an official Core Web Vital.",
    },
}

_CORE_KEYS = ["lcp_ms", "inp_ms", "cls"]


def _grade(value, good, needs_improvement):
    if value <= good:
        return "good"
    if value <= needs_improvement:
        return "needs_improvement"
    return "poor"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"lcp_ms": 2100, "inp_ms": 150, "cls": 0.05, "fcp_ms": 1200, "ttfb_ms": 400}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    given = {k: q[k] for k in _METRICS if k in q and q[k] is not None}
    if not given:
        print(json.dumps({
            "error": "provide at least one metric",
            "valid_metrics": list(_METRICS.keys()),
            "example": example,
        }))
        return 0

    for k, v in given.items():
        if not isinstance(v, (int, float)) or isinstance(v, bool):
            print(json.dumps({"error": "'%s' must be a number" % k, "example": example}))
            return 0

    try:
        metrics_out = {}
        for k, v in given.items():
            m = _METRICS[k]
            g = _grade(float(v), m["good"], m["needs_improvement"])
            metrics_out[k] = {
                "label": m["label"],
                "value": v,
                "grade": g,
                "core_web_vital": m["core"],
                "note": m["note"],
            }

        missing_core = [k for k in _CORE_KEYS if k not in given]
        result = {"metrics": metrics_out}
        if missing_core:
            result["core_web_vitals_pass"] = None
            result["core_web_vitals_note"] = (
                "cannot determine pass/fail: missing %s (all three of LCP, INP, "
                "and CLS are required)" % ", ".join(_METRICS[k]["label"] for k in missing_core)
            )
        else:
            result["core_web_vitals_pass"] = all(metrics_out[k]["grade"] == "good" for k in _CORE_KEYS)

        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "core_web_vitals_grade failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
