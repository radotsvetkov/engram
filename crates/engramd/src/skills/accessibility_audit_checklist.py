#!/usr/bin/env python3
"""accessibility_audit_checklist — Engram skill (no network). Score a page/app against a 12-item WCAG-inspired checklist.

This is a checklist self-assessment, NOT a live page scanner — you report
which elements are already present and it compares that against a canonical
12-item WCAG-inspired checklist (alt text, contrast, keyboard nav, etc.),
returning what's present, what's missing, a completeness percentage, and up
to 3 prioritized recommendations (in a fixed importance order) for what to
fix first. Unrecognized element ids are ignored with a note. Stdlib only.

Request (stdin): {"present_elements": ["alt_text_on_images", "keyboard_navigable"]}
Output (stdout): {present, missing, completeness_pct, priority_recommendations: [{element, reason}], note?}
"""
import json
import sys

_CANONICAL = [
    "alt_text_on_images", "sufficient_color_contrast", "keyboard_navigable",
    "visible_focus_indicators", "semantic_html_headings", "form_labels_associated",
    "aria_landmarks", "skip_to_content_link", "resizable_text_no_loss",
    "captions_on_video", "no_flashing_content", "descriptive_link_text",
]

_PRIORITY_ORDER = [
    "keyboard_navigable", "sufficient_color_contrast", "alt_text_on_images",
    "form_labels_associated", "semantic_html_headings",
]

_REASONS = {
    "alt_text_on_images": "screen-reader users get no information about non-text content without alt text, making images effectively invisible to them",
    "sufficient_color_contrast": "low-contrast text is unreadable for users with low vision or color blindness, and hard to read for everyone in bright light",
    "keyboard_navigable": "screen-reader and motor-impaired users who can't use a mouse are completely blocked without this",
    "visible_focus_indicators": "keyboard users lose track of where they are on the page without a visible focus outline, making navigation guesswork",
    "semantic_html_headings": "screen readers rely on heading structure to let users jump between sections — without it, navigating a long page is painfully linear",
    "form_labels_associated": "unlabeled form fields are unusable for screen-reader users, who have no way to know what a given input expects",
    "aria_landmarks": "landmarks (nav, main, header, etc.) let screen-reader users jump directly to a page region instead of tabbing through everything",
    "skip_to_content_link": "without a skip link, keyboard and screen-reader users must tab through the entire navigation on every single page load",
    "resizable_text_no_loss": "users with low vision who zoom or increase text size lose content or functionality if the layout breaks instead of reflowing",
    "captions_on_video": "deaf and hard-of-hearing users (and anyone in a sound-off environment) can't access video content without captions",
    "no_flashing_content": "flashing content faster than 3 times per second can trigger seizures in users with photosensitive epilepsy",
    "descriptive_link_text": "vague link text like 'click here' is meaningless out of context for screen-reader users who navigate by scanning a list of links",
}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"present_elements": ["alt_text_on_images", "keyboard_navigable", "semantic_html_headings"]}

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
        print(json.dumps({"error": "accessibility_audit_checklist failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
