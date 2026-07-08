#!/usr/bin/env python3
"""five_whys — Engram skill (no network).

Walks the Five Whys root-cause chain. Feed it the problem and however many
"why" answers you've collected so far (0-5); it returns the next prompt to
ask, or — once you have 5 — the root-cause candidate plus a check for
whether it's actually a system/process cause rather than blaming a person.

Request (stdin): {"problem": str, "whys"?: [str]}
Output (stdout): incomplete -> {problem, chain, whys_so_far, next_prompt,
  complete: false}; complete -> {problem, chain, root_cause_candidate,
  blame_flag, note, complete: true}
"""
import json
import sys

BLAME_PHRASES = ["someone", "they forgot", "human error", "somebody forgot", "his fault", "her fault"]


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"problem": "The production deploy failed", "whys": ["The build script crashed"]}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    problem = q.get("problem")
    if not isinstance(problem, str) or not problem.strip():
        print(json.dumps({"error": "missing required field 'problem' (string)", "example": example}))
        return 0
    problem = problem.strip()

    whys = q.get("whys", [])
    if whys is None:
        whys = []
    if not isinstance(whys, list):
        print(json.dumps({"error": "'whys' must be a list of strings", "example": example}))
        return 0

    try:
        why_texts = [str(w).strip() for w in whys if str(w).strip()]
        chain = [{"n": i + 1, "answer": w} for i, w in enumerate(why_texts)]

        if len(why_texts) < 5:
            last = why_texts[-1] if why_texts else problem
            next_prompt = "Why #%d: Why does '%s' happen?" % (len(why_texts) + 1, last)
            result = {
                "problem": problem,
                "chain": chain,
                "whys_so_far": len(why_texts),
                "next_prompt": next_prompt,
                "complete": False,
            }
        else:
            first_five = why_texts[:5]
            chain = [{"n": i + 1, "answer": w} for i, w in enumerate(first_five)]
            root_cause_candidate = first_five[4]
            low = root_cause_candidate.lower()
            blame_flag = any(p in low for p in BLAME_PHRASES)
            if blame_flag:
                note = ("This looks like it blames a person rather than a process — dig one "
                        "level deeper into the system or process that allowed the error to happen.")
            else:
                note = "Root cause candidate identified. Validate that it's specific and actionable."
            result = {
                "problem": problem,
                "chain": chain,
                "root_cause_candidate": root_cause_candidate,
                "blame_flag": blame_flag,
                "note": note,
                "complete": True,
            }
            if len(why_texts) > 5:
                result["additional_whys_ignored"] = len(why_texts) - 5

        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "five_whys failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
