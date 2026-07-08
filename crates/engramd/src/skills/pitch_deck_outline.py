#!/usr/bin/env python3
"""pitch_deck_outline — Engram skill (no network).

Produces a fixed, best-practice ~11-slide fundraising pitch deck outline
(the well-known YC/Sequoia structure), with per-slide guidance. The Title
slide is personalized with your company name and one-liner if given, and a
stage note tailors emphasis if you say what stage you're raising at.

Request (stdin): {"company_name": str, "one_liner"?: str, "stage"?: str}
Output (stdout): {company_name, one_liner, stage, slides: [{slide_number,
  title, guidance}], stage_note?}
"""
import json
import sys

SLIDE_TITLES = [
    "Title",
    "Problem",
    "Solution",
    "Why Now",
    "Market Size (TAM/SAM/SOM)",
    "Product",
    "Business Model",
    "Traction",
    "Competition",
    "Team",
    "Ask & Use of Funds",
]

GUIDANCE = {
    "Problem": "Describe the pain point your customer has today — make it visceral and "
               "specific; who has this problem and how are they solving it now (or not)?",
    "Solution": "Show how your product solves the problem; keep it simple, demo-able, "
                "and tied directly back to the problem slide.",
    "Why Now": "Explain what has changed in the market, technology, or regulation that "
               "makes this the right moment for this solution to win.",
    "Market Size (TAM/SAM/SOM)": "Size the total addressable, serviceable addressable, "
               "and serviceable obtainable market with credible, bottoms-up numbers.",
    "Product": "Show the product itself — screenshots, demo, or architecture — and how "
               "it delivers the value proposition.",
    "Business Model": "Explain how you make money: pricing, unit economics, customer "
               "acquisition cost vs. lifetime value.",
    "Traction": "Show evidence of momentum — revenue, users, growth rate, key "
               "partnerships, or pilot results.",
    "Competition": "Map the competitive landscape and articulate your durable "
               "differentiation or unfair advantage.",
    "Team": "Show why this team is uniquely suited to win — relevant experience, prior "
               "exits, domain expertise.",
    "Ask & Use of Funds": "State how much you're raising, at what terms, and exactly "
               "what milestones the money will buy you.",
}


def _stage_note(stage):
    s = stage.strip().lower().replace("-", "").replace(" ", "").replace("_", "")
    if "preseed" in s:
        return ("At pre-seed, emphasize team, vision, and problem clarity over traction — "
                 "investors are betting on you and the insight, not on metrics yet.")
    if s == "seed" or ("seed" in s and "pre" not in s):
        return ("At seed, balance early traction/signal (waitlist, pilots, early revenue) "
                 "with a clear wedge into a large market.")
    if "seriesa" in s or s == "a":
        return ("At Series A, emphasize traction, growth metrics, and unit economics — "
                 "investors expect proof the business model works and is ready to scale.")
    return ("Tailor emphasis to your stage: earlier stages lean on vision and team, later "
            "stages lean on traction and metrics.")


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"company_name": "Acme", "one_liner": "Instant invoicing for freelancers", "stage": "seed"}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    company_name = q.get("company_name")
    if not isinstance(company_name, str) or not company_name.strip():
        print(json.dumps({"error": "missing required field 'company_name' (string)", "example": example}))
        return 0
    company_name = company_name.strip()

    one_liner = q.get("one_liner")
    one_liner = one_liner.strip() if isinstance(one_liner, str) and one_liner.strip() else None

    stage = q.get("stage")
    stage = stage.strip() if isinstance(stage, str) and stage.strip() else None

    try:
        if one_liner:
            title_guidance = ("Lead with '%s — %s'. Include founder names and contact info; "
                               "this slide should be legible at a glance." % (company_name, one_liner))
        else:
            title_guidance = ("Lead with '%s' and a one-line description of what you do. Include "
                               "founder names and contact info." % company_name)

        slides = []
        for i, title in enumerate(SLIDE_TITLES, start=1):
            guidance = title_guidance if title == "Title" else GUIDANCE[title]
            slides.append({"slide_number": i, "title": title, "guidance": guidance})

        result = {
            "company_name": company_name,
            "one_liner": one_liner,
            "stage": stage,
            "slides": slides,
        }
        if stage:
            result["stage_note"] = _stage_note(stage)

        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "pitch_deck_outline failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
