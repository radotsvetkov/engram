#!/usr/bin/env python3
"""usability_test_script_gen — Engram skill (no network).

Generates a complete, ready-to-run MODERATED usability test script following
standard UX research practice: an intro script for the moderator to read,
warm-up questions, one structured task per feature under test (scenario-framed
prompt, success criteria, post-task questions), and closing wrap-up questions.

Request (stdin): {"product_name": str, "features_to_test": [str],
  "target_audience"?: str}
Output (stdout): {intro_script: str, warm_up_questions: [str],
  tasks: [{feature, task_prompt, success_criteria, post_task_questions}],
  wrap_up_questions: [str]}
"""
import json
import sys


def _intro_script(product_name, audience):
    audience_clause = (" as someone who fits our target group of %s" % audience) if audience else ""
    return (
        "Hi, and thank you for taking the time to join me today%s. My name is "
        "[moderator name], and I'll be walking you through this session. We're "
        "testing %s, and I want to be upfront: we're testing the product, not "
        "you — there are no right or wrong answers, and nothing you do will be "
        "wrong. As you work through a few tasks, I'd like you to think out loud "
        "— tell me what you're looking at, what you expect to happen, and what "
        "you're trying to do, even if it seems obvious. I'll be taking notes and, "
        "with your permission, recording this session so I can review it later "
        "— the recording is only for our internal research and won't be shared "
        "publicly. Is that okay with you? If at any point you want to stop, just "
        "let me know. Do you have any questions before we begin?"
    ) % (audience_clause, product_name)


def _warm_up_questions(product_name, audience):
    q1 = "Can you tell me a little about yourself and what you do day to day?"
    if audience:
        q2 = "How familiar are you with tools like %s, or with %s in general?" % (product_name, audience)
    else:
        q2 = "How familiar are you with %s or similar products?" % product_name
    q3 = "Walk me through the last time you had to do something %s is meant to help with — what did you use, and how did it go?" % product_name
    return [q1, q2, q3]


def _task_prompt(feature):
    goal = feature.strip().rstrip(".")
    if not goal:
        goal = "use this feature"
    return "Imagine you need to %s. Show me how you'd go about doing that." % goal


def _success_criteria(feature):
    return (
        "Participant locates and uses the '%s' functionality without moderator "
        "intervention, and confirms (verbally or by outcome) that they achieved "
        "what they set out to do." % feature
    )


def _post_task_questions():
    return [
        "On a scale of 1-5, how easy or difficult was that?",
        "What, if anything, was confusing?",
    ]


def _wrap_up_questions(product_name):
    return [
        "Overall, what were your first impressions of %s?" % product_name,
        "What did you like most about the experience? What did you like least?",
        "Is there anything that felt confusing, frustrating, or unexpected?",
        "On a scale of 1-10, how likely would you be to recommend %s to a colleague or friend, and why?" % product_name,
        "Is there anything else you'd like to share that we haven't covered?",
    ]


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"product_name": "Acme App",
                        "features_to_test": ["password reset", "export to PDF"],
                        "target_audience": "small business owners"},
        }))
        return 0

    product_name = q.get("product_name")
    features = q.get("features_to_test")

    if not product_name or not isinstance(product_name, str):
        print(json.dumps({
            "error": "'product_name' (string) is required",
            "example": {"product_name": "Acme App", "features_to_test": ["password reset"]},
        }))
        return 0

    if not features or not isinstance(features, list):
        print(json.dumps({
            "error": "'features_to_test' (non-empty list of strings) is required",
            "example": {"product_name": "Acme App", "features_to_test": ["password reset", "export to PDF"]},
        }))
        return 0

    features = [str(f).strip() for f in features if str(f).strip()]
    if not features:
        print(json.dumps({"error": "'features_to_test' must contain at least one non-empty string"}))
        return 0

    audience = q.get("target_audience")
    audience = str(audience).strip() if audience else None

    try:
        result = {
            "intro_script": _intro_script(product_name, audience),
            "warm_up_questions": _warm_up_questions(product_name, audience),
            "tasks": [
                {
                    "feature": f,
                    "task_prompt": _task_prompt(f),
                    "success_criteria": _success_criteria(f),
                    "post_task_questions": _post_task_questions(),
                }
                for f in features
            ],
            "wrap_up_questions": _wrap_up_questions(product_name),
        }
    except Exception as e:
        print(json.dumps({"error": "could not build usability test script: %s" % e}))
        return 0

    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
