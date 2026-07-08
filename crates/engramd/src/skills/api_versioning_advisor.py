#!/usr/bin/env python3
"""api_versioning_advisor — Engram skill (no network). Heuristically
classify a described API change as a semver MAJOR/MINOR/PATCH bump via
keyword matching, and lay out the two common REST API versioning
strategies (URL path vs. header-based) with their tradeoffs.

Request (stdin): {"change_description": str, "current_version"?: str = "1.0.0"}
Output (stdout): {classification, confidence, matched_keywords,
                   recommended_next_version, versioning_strategies, note}
"""
import json
import re
import sys

_MAJOR_KEYWORDS = [
    "remove", "removed", "rename", "renamed", "change the type", "changed the type",
    "required field", "breaking", "incompatible",
]
_MINOR_KEYWORDS = [
    "add", "added", "new endpoint", "new field", "optional", "deprecate", "deprecated",
]
_PATCH_KEYWORDS = [
    "fix", "fixed", "bug", "typo", "performance", "correct", "corrected",
]

_SEMVER_RE = re.compile(r"^(\d+)\.(\d+)\.(\d+)(?:[-+].*)?$")

_VERSIONING_STRATEGIES = {
    "url_path": {
        "example": "/v2/resource",
        "pros": "simple, very visible in logs/docs/browser bar, easy to route at the "
                "infra/proxy layer, trivially testable with curl or a browser",
        "cons": "couples the version to routing/URL structure, encourages duplicating "
                "whole route trees per version, resource identity (the URL) changes across versions",
    },
    "header_based": {
        "example": "Accept: application/vnd.api+json;version=2",
        "pros": "keeps URLs stable and version-agnostic (better for resource identity/caching "
                "semantics), keeps the version orthogonal to routing",
        "cons": "less discoverable (invisible when just looking at a URL), harder to test "
                "manually (can't just paste a URL in a browser), easy for clients to forget "
                "to set the header",
    },
}


def _find_keyword_matches(text_lower, keywords):
    matched = []
    for kw in keywords:
        if kw in text_lower:
            matched.append(kw)
    return matched


def _classify(change_description):
    text_lower = change_description.lower()

    major_matches = _find_keyword_matches(text_lower, _MAJOR_KEYWORDS)
    minor_matches = _find_keyword_matches(text_lower, _MINOR_KEYWORDS)
    patch_matches = _find_keyword_matches(text_lower, _PATCH_KEYWORDS)

    # Precedence: a breaking-change signal always wins (safest default), then
    # additive, then fix-only. This mirrors real-world practice: if a change
    # both adds a field AND removes one, it's still MAJOR overall.
    if major_matches:
        return "major", "high", major_matches
    if minor_matches:
        return "minor", "high", minor_matches
    if patch_matches:
        return "patch", "high", patch_matches
    return "minor", "low", []


def _bump_semver(version_str, classification):
    m = _SEMVER_RE.match(version_str.strip())
    if not m:
        return None
    major, minor, patch = (int(x) for x in m.groups())
    if classification == "major":
        return "%d.0.0" % (major + 1)
    if classification == "minor":
        return "%d.%d.0" % (major, minor + 1)
    return "%d.%d.%d" % (major, minor, patch + 1)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"change_description": "Removed the 'legacy_id' field from the response",
                                      "current_version": "1.4.2"}}))
        return 0

    change_description = q.get("change_description")
    if not isinstance(change_description, str) or not change_description.strip():
        print(json.dumps({
            "error": "provide non-empty 'change_description'",
            "example": {"change_description": "Added a new optional 'metadata' field to the response",
                        "current_version": "1.4.2"},
        }))
        return 0

    current_version = q.get("current_version") or "1.0.0"
    if not isinstance(current_version, str):
        print(json.dumps({"error": "'current_version' must be a string, e.g. '1.4.2'"}))
        return 0

    try:
        classification, confidence, matched_keywords = _classify(change_description)
        recommended_next_version = _bump_semver(current_version, classification)
    except Exception as e:
        print(json.dumps({"error": "classification failed: %s" % e}))
        return 1

    result = {
        "classification": classification,
        "confidence": confidence,
        "matched_keywords": matched_keywords,
        "current_version": current_version,
        "recommended_next_version": recommended_next_version,
        "versioning_strategies": _VERSIONING_STRATEGIES,
    }
    if confidence == "low":
        result["note"] = (
            "no clear breaking/additive/fix keyword signal was found in 'change_description' — "
            "defaulted to MINOR; please clarify the change (e.g. does it remove/rename/change "
            "the type of anything existing?) for a more confident classification"
        )
    if recommended_next_version is None:
        result["version_note"] = "'current_version' is not a valid semver string (expected MAJOR.MINOR.PATCH) — no bump computed"
    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
