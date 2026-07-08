#!/usr/bin/env python3
"""jobs_to_be_done_canvas — Engram skill (no network).

Builds a Jobs-to-be-Done (JTBD) canvas across the classic six dimensions:
struggling_moment, desired_outcome, current_solution, functional_job,
emotional_job, social_job. Any dimension you don't provide comes back with a
guiding prompt specific to that dimension so you know what to fill in next.
When both struggling_moment and functional_job are filled, a canonical JTBD
statement is generated using the standard "When I ... I want to ... so I can
..." template. Stdlib only.

Request (stdin): {"struggling_moment"?: str, "desired_outcome"?: str,
  "current_solution"?: str, "functional_job"?: str, "emotional_job"?: str,
  "social_job"?: str}
Output (stdout): {struggling_moment, desired_outcome, current_solution,
  functional_job, emotional_job, social_job, completeness_pct, jtbd_statement}
"""
import json
import sys

DIMENSIONS = [
    ("struggling_moment",
     "Describe the specific moment/context where the customer realizes they "
     "need to make progress — what triggered it?"),
    ("desired_outcome",
     "What does success look like once the job is done? How would the "
     "customer measure progress?"),
    ("current_solution",
     "What is the customer using today (a product, a workaround, or doing "
     "nothing) to get this job done, and where does it fall short?"),
    ("functional_job",
     "What practical task is the customer trying to get done?"),
    ("emotional_job",
     "How does the customer want to FEEL (or avoid feeling) while getting "
     "this done?"),
    ("social_job",
     "How does the customer want to be PERCEIVED by others through this?"),
]

_DIM_KEYS = [k for k, _ in DIMENSIONS]


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {
        "struggling_moment": "our team keeps missing deadlines because status updates are scattered across tools",
        "functional_job": "get a single, trustworthy view of project status",
        "desired_outcome": "spend less time chasing updates and more time unblocking work",
    }

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    for key, _ in DIMENSIONS:
        val = q.get(key)
        if val is not None and not isinstance(val, str):
            print(json.dumps({
                "error": "'%s' must be a string if provided" % key,
                "example": example,
            }))
            return 0

    try:
        values = {}
        result = {}
        filled_count = 0
        for key, prompt in DIMENSIONS:
            raw = q.get(key)
            text = raw.strip() if isinstance(raw, str) else ""
            values[key] = text
            if text:
                filled_count += 1
                result[key] = {"status": "filled", "value": text}
            else:
                result[key] = {"status": "empty", "prompt": prompt}

        completeness_pct = round(filled_count / len(DIMENSIONS) * 100, 1)

        jtbd_statement = None
        if values["struggling_moment"] and values["functional_job"]:
            outcome = values["desired_outcome"] or "make real progress"
            jtbd_statement = "When I %s, I want to %s, so I can %s." % (
                values["struggling_moment"], values["functional_job"], outcome,
            )

        result["completeness_pct"] = completeness_pct
        result["jtbd_statement"] = jtbd_statement

        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "jobs_to_be_done_canvas failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
