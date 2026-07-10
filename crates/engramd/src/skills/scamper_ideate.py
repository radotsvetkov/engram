#!/usr/bin/env python3
"""scamper_ideate — Engram skill (no network).

Runs the SCAMPER innovation framework over a subject: for each of the seven
lenses (Substitute, Combine, Adapt, Modify/Magnify, Put to another use,
Eliminate, Reverse/Rearrange) it generates targeted ideation PROMPTS applied
to the subject. These are divergent-thinking scaffolding, meant to spark
ideas that you later narrow down.

Request (stdin): {"subject": str}
Output (stdout): {subject, lenses, note}
"""
import json
import sys

# (lens, letter, [prompt templates]) — {subject} is filled in.
LENSES = [
    ("Substitute", "S", [
        "What material, step, component or person in '{subject}' could be swapped for something "
        "cheaper, faster or better?",
        "What rule or ingredient in '{subject}' could be replaced with an alternative?",
    ]),
    ("Combine", "C", [
        "What could you merge with '{subject}' — another product, feature, step or audience — to "
        "create more value?",
        "What if you combined '{subject}' with something from a totally different industry?",
    ]),
    ("Adapt", "A", [
        "What existing solution from elsewhere could you adapt to '{subject}'?",
        "What has worked in a similar context that '{subject}' could borrow?",
    ]),
    ("Modify/Magnify", "M", [
        "What in '{subject}' could you magnify, exaggerate or make far bigger/smaller?",
        "What attribute of '{subject}' (shape, frequency, speed, scale) could you change?",
    ]),
    ("Put to another use", "P", [
        "Who else could use '{subject}', or what other problem could it solve unchanged?",
        "What would '{subject}' be worth to a completely different market or use case?",
    ]),
    ("Eliminate", "E", [
        "What step, feature or cost in '{subject}' could you remove entirely without losing the "
        "core value?",
        "What would a stripped-down, minimal version of '{subject}' look like?",
    ]),
    ("Reverse/Rearrange", "R", [
        "What if you reversed the order, roles or flow in '{subject}'?",
        "How could you rearrange or reorganize the parts of '{subject}' for a better result?",
    ]),
]


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"subject": "our customer onboarding flow"}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    subject = q.get("subject")
    if not isinstance(subject, str) or not subject.strip():
        print(json.dumps({"error": "missing required field 'subject' (string)", "example": example}))
        return 0
    subject = subject.strip()

    try:
        lenses = [
            {
                "lens": name,
                "letter": letter,
                "prompts": [p.format(subject=subject) for p in prompts],
            }
            for name, letter, prompts in LENSES
        ]
        result = {
            "subject": subject,
            "lenses": lenses,
            "note": ("SCAMPER is a DIVERGENT ideation tool — aim for quantity of ideas per lens, "
                     "defer judgment, and don't filter yet. Once you have a wide set, switch to "
                     "convergent selection (e.g. score the best ideas with a decision_matrix)."),
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "scamper_ideate failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
