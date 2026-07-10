#!/usr/bin/env python3
"""first_principles — Engram skill (no network).

Builds a first-principles reasoning SCAFFOLD for a problem: surface current
assumptions, challenge each one ("is this fundamentally true or just
convention?"), identify the irreducible fundamentals that CAN'T change, then
rebuild a solution from only those. Prompts are scaffolding to work through,
not answers.

Request (stdin): {"problem": str, "assumptions"?: [str]}
Output (stdout): {problem, steps, note}
"""
import json
import sys


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"problem": "Batteries are too expensive for grid storage",
               "assumptions": ["We must buy finished cells from suppliers"]}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    problem = q.get("problem")
    if not isinstance(problem, str) or not problem.strip():
        print(json.dumps({"error": "missing required field 'problem' (string)", "example": example}))
        return 0
    problem = problem.strip()

    assumptions = q.get("assumptions", [])
    if assumptions is None:
        assumptions = []
    if not isinstance(assumptions, list):
        print(json.dumps({"error": "'assumptions' must be a list of strings", "example": example}))
        return 0
    assumptions = [str(a).strip() for a in assumptions if str(a).strip()]

    try:
        challenge_t = ("Is '%s' fundamentally true, or just convention/analogy/inertia? "
                       "What is the actual evidence? What breaks if it's false?")
        challenges = [{"assumption": a, "challenge_prompt": challenge_t % a} for a in assumptions]

        steps = [
            {
                "step": 1,
                "name": "List assumptions",
                "prompt": ("Write down every assumption you're currently making about '%s' — "
                           "especially the ones that feel too obvious to state." % problem),
                "given_assumptions": assumptions,
            },
            {
                "step": 2,
                "name": "Challenge each assumption",
                "prompt": "For each assumption, ask whether it is a fundamental truth or an "
                          "inherited convention.",
                "challenges": challenges,
            },
            {
                "step": 3,
                "name": "Identify the fundamentals",
                "prompt": ("What are the irreducible fundamentals of '%s' — the physics, "
                           "economics, math or hard constraints that genuinely CANNOT change? "
                           "Reduce until you hit bedrock." % problem),
                "sub_prompts": [
                    "What is true here regardless of how it's currently done?",
                    "What is the theoretical floor (cost/time/energy) set by those constraints?",
                    "Which 'constraints' are actually just current practice in disguise?",
                ],
            },
            {
                "step": 4,
                "name": "Rebuild from fundamentals",
                "prompt": ("Given ONLY those fundamentals, how would you solve '%s' from scratch, "
                           "as if nothing had been built before?" % problem),
                "sub_prompts": [
                    "What solution does the bedrock actually permit that convention rules out?",
                    "Where is the gap between today's approach and the theoretical floor?",
                ],
            },
        ]

        result = {
            "problem": problem,
            "steps": steps,
            "note": ("SCAFFOLD: first-principles thinking (Aristotle's 'first basis'; Musk's "
                     "'boil things down to fundamental truths and reason up') deliberately "
                     "breaks reasoning-by-analogy — solving by copying what others do. Work each "
                     "step honestly; the value is in challenging assumptions you didn't know you "
                     "held."),
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "first_principles failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
