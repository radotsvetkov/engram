#!/usr/bin/env python3
"""task_decompose — Engram skill (no network).

Breaks a goal into a MECE-style decomposition SCAFFOLD: 3-5 top-level
workstream prompts (each with 2-3 sub-prompts) that guide you to define
acceptance criteria, phases, dependencies, blockers and resources. The
prompts are templated scaffolding for you to fill in, not final subtasks.

Request (stdin): {"goal": str, "max_depth"?: int (default 2)}
Output (stdout): {goal, max_depth, tree, checklist, count, note}
"""
import json
import sys

# Each workstream: (title, top-level prompt, [sub-prompts]) — {goal} is filled in.
WORKSTREAMS = [
    (
        "Definition of done",
        "What must be TRUE for '{goal}' to be considered done? (define acceptance criteria)",
        [
            "What is the single measurable outcome that proves success?",
            "What is explicitly OUT of scope?",
            "Who signs off that it is complete?",
        ],
    ),
    (
        "Major phases",
        "What are the major phases or milestones for '{goal}'?",
        [
            "What is the first shippable/testable increment?",
            "What is the rough sequence of phases?",
        ],
    ),
    (
        "Dependencies & prerequisites",
        "What must exist or be true BEFORE work on '{goal}' can start?",
        [
            "What upstream inputs, approvals or data are required?",
            "Which other teams or systems does this depend on?",
        ],
    ),
    (
        "Risks & blockers",
        "What could block or derail '{goal}'?",
        [
            "What is the single biggest unknown or assumption?",
            "What would you do if the riskiest assumption is false?",
        ],
    ),
    (
        "People & resources",
        "Who and what is needed to deliver '{goal}'?",
        [
            "Who owns each workstream?",
            "What tools, budget or access are required?",
        ],
    ),
]


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"goal": "Launch the v2 API", "max_depth": 2}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    goal = q.get("goal")
    if not isinstance(goal, str) or not goal.strip():
        print(json.dumps({"error": "missing required field 'goal' (string)", "example": example}))
        return 0
    goal = goal.strip()

    max_depth = q.get("max_depth", 2)
    try:
        max_depth = int(max_depth)
    except (TypeError, ValueError):
        print(json.dumps({"error": "'max_depth' must be an integer", "example": example}))
        return 0
    if max_depth < 1:
        max_depth = 1
    if max_depth > 3:
        max_depth = 3

    try:
        tree = []
        checklist = []
        for i, (title, prompt_t, subs) in enumerate(WORKSTREAMS, 1):
            prompt = prompt_t.format(goal=goal)
            node = {"id": "%d" % i, "workstream": title, "prompt": prompt}
            checklist.append(prompt)
            if max_depth >= 2:
                children = []
                for j, sub_t in enumerate(subs, 1):
                    sub = sub_t.format(goal=goal)
                    children.append({"id": "%d.%d" % (i, j), "prompt": sub})
                    checklist.append(sub)
                node["children"] = children
            tree.append(node)

        result = {
            "goal": goal,
            "max_depth": max_depth,
            "tree": tree,
            "checklist": checklist,
            "count": len(checklist),
            "note": ("SCAFFOLD ONLY: these are templated decomposition prompts, not final "
                     "subtasks. Answer each to turn the goal into a concrete, MECE work "
                     "breakdown (mutually exclusive, collectively exhaustive)."),
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "task_decompose failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
