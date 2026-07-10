#!/usr/bin/env python3
"""ooda_loop — Engram skill (no network).

Produces an OODA (Observe-Orient-Decide-Act) SCAFFOLD for a situation:
2-3 guiding prompts per phase to move you quickly from raw signals to a
small reversible action. The prompts are templated scaffolding to answer,
not a decision made for you.

Request (stdin): {"situation": str}
Output (stdout): {situation, phases, note}
"""
import json
import sys

# (phase, [prompts]) — {situation} is filled in.
PHASES = [
    ("Observe", [
        "What data and signals are available about '{situation}' right now?",
        "What is changing fastest, and what feedback loops are in play?",
        "What information is missing or stale that you're implicitly assuming?",
    ]),
    ("Orient", [
        "What is your current mental model of '{situation}', and where did it come from?",
        "What biases, old analogies or outdated experience might distort that model?",
        "How would someone who disagrees with you read the same signals?",
    ]),
    ("Decide", [
        "What are the 2-3 realistic options for '{situation}' and their expected outcomes?",
        "Which option is most reversible if you're wrong?",
        "What would have to be true for each option to be the right call?",
    ]),
    ("Act", [
        "What is the smallest reversible next step you can take on '{situation}' now?",
        "How and when will you measure whether it worked?",
        "What signal would tell you to loop back and re-orient?",
    ]),
]


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"situation": "A competitor just undercut our pricing by 30%"}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    situation = q.get("situation")
    if not isinstance(situation, str) or not situation.strip():
        print(json.dumps({"error": "missing required field 'situation' (string)",
                          "example": example}))
        return 0
    situation = situation.strip()

    try:
        phases = [
            {"phase": name, "prompts": [p.format(situation=situation) for p in prompts]}
            for name, prompts in PHASES
        ]
        result = {
            "situation": situation,
            "phases": phases,
            "note": ("SCAFFOLD: OODA is a loop, not a checklist — cycle through it repeatedly. "
                     "The edge comes from tempo: looping faster than the problem (or adversary) "
                     "evolves, so decisions stay based on current reality. Prefer small "
                     "reversible acts that generate new observations over one big irreversible "
                     "bet."),
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "ooda_loop failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
