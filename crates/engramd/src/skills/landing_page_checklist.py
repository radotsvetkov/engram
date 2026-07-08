#!/usr/bin/env python3
"""landing_page_checklist — Engram skill (no network). Score a landing page against a 14-item checklist.

Compares the elements you report as present against a canonical 14-item
conversion checklist (headline, CTA, social proof, mobile responsiveness,
etc.), returning what's present, what's missing, a completeness percentage,
and up to 3 prioritized recommendations (in a fixed importance order) for
what to fix first. Unrecognized element ids are ignored with a note.
Stdlib only.

Request (stdin): {"present_elements": ["clear_headline", "hero_image", "mobile_responsive"]}
Output (stdout): {present, missing, completeness_pct, priority_recommendations: [{element, reason}], note?}
"""
import json
import sys

_CANONICAL = [
    "clear_headline", "subheadline", "hero_image", "single_cta", "social_proof",
    "trust_badges", "mobile_responsive", "fast_load", "above_fold_cta",
    "contact_form", "urgency_scarcity", "testimonials", "faq", "clear_value_prop",
]

_PRIORITY_ORDER = [
    "clear_value_prop", "single_cta", "social_proof", "mobile_responsive", "above_fold_cta",
]

_REASONS = {
    "clear_headline": "the headline is the first thing read — an unclear one causes instant drop-off",
    "subheadline": "the subheadline reinforces the headline with detail, helping visitors self-qualify before reading on",
    "hero_image": "a relevant hero image builds context and emotional connection faster than text alone",
    "single_cta": "multiple competing CTAs split attention and reduce the conversion rate for any one action",
    "social_proof": "social proof (reviews, logos, numbers) reduces perceived risk and builds trust before asking for a commitment",
    "trust_badges": "trust badges (security, certifications, guarantees) reduce purchase anxiety at the moment of decision",
    "mobile_responsive": "most traffic is mobile — a non-responsive layout breaks the experience for the majority of visitors",
    "fast_load": "slow load times directly increase bounce rate — every extra second costs conversions",
    "above_fold_cta": "putting the primary CTA above the fold captures high-intent visitors who never scroll",
    "contact_form": "a contact form gives lower-intent visitors an easy way to engage instead of leaving",
    "urgency_scarcity": "urgency/scarcity cues (limited time or stock) nudge undecided visitors toward acting now",
    "testimonials": "testimonials add a relatable, human voice to social proof, increasing credibility",
    "faq": "an FAQ section pre-empts objections that would otherwise stall the decision",
    "clear_value_prop": "without a clear value proposition, visitors can't tell what's in it for them within seconds — the top driver of bounce",
}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"present_elements": ["clear_headline", "hero_image", "mobile_responsive"]}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    present_elements = q.get("present_elements")
    if not isinstance(present_elements, list) or not all(isinstance(x, str) for x in present_elements):
        print(json.dumps({
            "error": "missing required field 'present_elements' (list of strings)",
            "recognized_elements": _CANONICAL,
            "example": example,
        }))
        return 0

    try:
        given_set = set(present_elements)
        canonical_set = set(_CANONICAL)

        present = [e for e in _CANONICAL if e in given_set]
        missing = [e for e in _CANONICAL if e not in given_set]
        unrecognized = sorted(set(x for x in present_elements if x not in canonical_set))
        completeness_pct = round(len(present) / len(_CANONICAL) * 100, 1)

        missing_set = set(missing)
        priority_ids = [k for k in _PRIORITY_ORDER if k in missing_set]
        for k in _CANONICAL:
            if k in missing_set and k not in priority_ids:
                priority_ids.append(k)
        priority_ids = priority_ids[:3]
        priority_recommendations = [{"element": k, "reason": _REASONS[k]} for k in priority_ids]

        result = {
            "present": present,
            "missing": missing,
            "completeness_pct": completeness_pct,
            "priority_recommendations": priority_recommendations,
        }
        if unrecognized:
            result["note"] = "ignored unrecognized element(s): %s" % ", ".join(unrecognized)

        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "landing_page_checklist failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
