#!/usr/bin/env python3
"""app_store_release_checklist — Engram skill (no network). Produce a
pre-submission release checklist for a mobile app.

Tailors items to iOS (App Store Connect), Android (Play Console), or both.
Items are grouped by phase (build / assets / compliance / store-listing /
testing / submit) and each carries a short `why`. Reference-only, stdlib.

Request (stdin): {"platform": "both"}
Output (stdout): {platform, checklist: {phase: [{item, why}]}}
"""
import json
import sys

_VALID = ("ios", "android", "both")

# Phase order preserved in output.
_PHASES = ["build", "assets", "compliance", "store-listing", "testing", "submit"]

_IOS = {
    "build": [
        {"item": "Bump version (CFBundleShortVersionString) and build number (CFBundleVersion)",
         "why": "App Store Connect rejects an upload whose build number is not higher than the last."},
        {"item": "Set the correct bundle ID and select a distribution provisioning profile / signing certificate",
         "why": "Archives must be signed with an App Store distribution profile matching the bundle ID or upload fails."},
        {"item": "Add Info.plist usage-description strings for every permission you request (e.g. NSCameraUsageDescription, NSPhotoLibraryUsageDescription, NSLocationWhenInUseUsageDescription)",
         "why": "iOS crashes on first access to a protected resource with no purpose string, and review rejects apps missing them."},
        {"item": "Archive a Release build for a generic iOS device and validate it in Xcode Organizer",
         "why": "Validation catches signing, entitlement, and asset problems before you waste an upload."},
    ],
    "assets": [
        {"item": "Provide all required app icon sizes in the asset catalog (including the 1024x1024 App Store icon)",
         "why": "A missing marketing icon or icon slot blocks the upload and the store listing."},
        {"item": "Add a launch screen (storyboard or SwiftUI launch screen)",
         "why": "Apple requires a launch screen; a static image or missing one can trigger rejection and looks broken on new device sizes."},
        {"item": "Capture screenshots for every required device class (6.7\" iPhone, 6.5\" iPhone, 12.9\" iPad if universal)",
         "why": "App Store Connect will not let you submit without screenshots for the required display sizes."},
    ],
    "compliance": [
        {"item": "Complete the App Privacy 'nutrition label' (data collection, tracking, linkage) in App Store Connect",
         "why": "The privacy details are mandatory before submission and must match what your app actually collects."},
        {"item": "Answer export-compliance / encryption questions (set ITSAppUsesNonExemptEncryption if applicable)",
         "why": "Every build must declare encryption usage; skipping it stalls the build in 'Missing Compliance'."},
        {"item": "Confirm you have a reachable privacy policy URL",
         "why": "A working privacy policy URL is required for the listing and for any app that collects data."},
    ],
    "store-listing": [
        {"item": "Fill in name, subtitle, description, keywords, support URL, and category",
         "why": "Incomplete metadata blocks submission and hurts discoverability."},
        {"item": "Set age rating via the content questionnaire",
         "why": "The rating is required and a mismatch with actual content triggers rejection."},
    ],
    "testing": [
        {"item": "Distribute the build via TestFlight and run a final smoke test on a real device",
         "why": "TestFlight surfaces crashes, permission prompts, and signing issues that simulators hide before public release."},
    ],
    "submit": [
        {"item": "Select the build, add release notes, choose manual or automatic release, and submit for review",
         "why": "Review is the final gate; choosing manual release lets you time the launch after approval."},
    ],
}

_ANDROID = {
    "build": [
        {"item": "Bump versionCode (integer) and versionName (display string) in build.gradle",
         "why": "Play Console rejects an AAB whose versionCode is not strictly greater than the highest already uploaded."},
        {"item": "Build a signed Android App Bundle (.aab) with your upload key; enroll in Play App Signing",
         "why": "Play distributes AABs (not APKs) and requires a signed bundle; Play App Signing manages the final release key."},
        {"item": "Meet the current target API level requirement (compileSdk/targetSdk) enforced by Play",
         "why": "Google blocks new app and update submissions that target an SDK below the yearly minimum."},
        {"item": "Enable and test ProGuard/R8 shrinking and keep a mapping.txt for crash de-obfuscation",
         "why": "Shrinking reduces size but can strip needed classes; the mapping file is needed to read release stack traces."},
    ],
    "assets": [
        {"item": "Provide an adaptive icon (foreground + background layers)",
         "why": "Modern Android launchers require an adaptive icon; a legacy-only icon looks wrong across device themes."},
        {"item": "Upload a 1024x500 feature graphic and phone/tablet screenshots",
         "why": "The feature graphic and at least the minimum screenshots are mandatory for the store listing."},
    ],
    "compliance": [
        {"item": "Complete the Data safety form (data collected/shared, security practices)",
         "why": "The Data safety section is mandatory and must accurately reflect your app's behavior or the release is rejected."},
        {"item": "Set the content rating via the IARC questionnaire and declare a privacy policy URL",
         "why": "A content rating and privacy policy are required before you can roll out to production."},
        {"item": "Complete app-access details and any required declarations (ads, permissions, target audience)",
         "why": "Missing declarations (e.g. sensitive permissions, ads presence) hold the review or cause rejection."},
    ],
    "store-listing": [
        {"item": "Fill in app title, short and full description, and select category",
         "why": "The main store listing must be complete before a production release can be created."},
    ],
    "testing": [
        {"item": "Roll the build out to an internal (or closed) testing track and verify install/upgrade on a real device",
         "why": "Testing tracks catch signing, upgrade, and crash issues before the update reaches production users."},
    ],
    "submit": [
        {"item": "Create a production release, add release notes, set the staged-rollout percentage, and submit for review",
         "why": "A staged rollout limits blast radius; review is the final gate before general availability."},
    ],
}


def _sections(platform):
    out = {}
    for phase in _PHASES:
        items = []
        if platform in ("ios", "both"):
            for it in _IOS.get(phase, []):
                entry = dict(it)
                if platform == "both":
                    entry["platform"] = "ios"
                items.append(entry)
        if platform in ("android", "both"):
            for it in _ANDROID.get(phase, []):
                entry = dict(it)
                if platform == "both":
                    entry["platform"] = "android"
                items.append(entry)
        if items:
            out[phase] = items
    return out


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"platform": "both"},
        }))
        return 0

    platform = q.get("platform")
    if not isinstance(platform, str):
        print(json.dumps({
            "error": "missing required field 'platform'",
            "allowed": list(_VALID),
            "example": {"platform": "both"},
        }))
        return 0
    platform = platform.strip().lower()
    if platform in ("ios+android", "android+ios", "all"):
        platform = "both"
    if platform not in _VALID:
        print(json.dumps({
            "error": "invalid 'platform': %r" % q.get("platform"),
            "allowed": list(_VALID),
            "example": {"platform": "both"},
        }))
        return 0

    try:
        checklist = _sections(platform)
        total = sum(len(v) for v in checklist.values())
        result = {
            "platform": platform,
            "phases": _PHASES,
            "item_count": total,
            "checklist": checklist,
            "notes": [
                "Ordering is a suggested flow: build -> assets -> compliance -> "
                "store-listing -> testing -> submit.",
                "Requirements evolve (target API levels, privacy forms); confirm "
                "current specifics in App Store Connect / Play Console before submitting.",
            ],
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "app_store_release_checklist failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
