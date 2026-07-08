#!/usr/bin/env python3
"""drip_campaign_planner — Engram skill (no network). Templated multi-email drip sequences.

Provides a sensible, templated email sequence for common campaign types
(onboarding, nurture, win_back, abandoned_cart), each step with a send delay,
purpose, subject-line template, and a content prompt. The email count can be
adjusted with 'num_emails' — fewer than the base template truncates to the
highest-priority steps; more cycles through the base purposes with extended
delays. These are deterministic starting-point templates, not personalized
copy. Stdlib only.

Request (stdin): {"campaign_type": "onboarding", "num_emails": 5}
Output (stdout): {campaign_type, num_emails, sequence: [{email_number, send_delay, purpose, subject_line_template, key_content_prompt}]}
"""
import json
import sys

_CAMPAIGN_TYPES = ("onboarding", "nurture", "win_back", "abandoned_cart")

_MIN_EMAILS = 1
_MAX_EMAILS = 20

# Day-based templates: (days, purpose, subject_line_template, key_content_prompt)
_BASE = {
    "onboarding": [
        (0, "Welcome + quick win",
         "Welcome to {product} — let's get your first win",
         "Thank them for joining, restate the core value prop in one line, and point to one small action that delivers an immediate result."),
        (1, "Highlight the core feature",
         "Have you tried this yet?",
         "Introduce the single most important feature, show how to use it in under a minute, and link to a short walkthrough."),
        (3, "Social proof / case study",
         "How [Customer] got [result] with {product}",
         "Share a concise customer story or stat that reinforces they made the right choice."),
        (7, "Tips & best practices",
         "3 tips to get more out of {product}",
         "Offer practical tips and common-mistake fixes that deepen usage."),
        (14, "Check-in + upgrade/next-step CTA",
         "How's it going so far?",
         "Check in on their progress, offer support, and present one clear next step (upgrade, book a call, or unlock an advanced feature)."),
    ],
    "nurture": [
        (0, "Deliver the lead magnet / value",
         "Here's your [lead magnet]",
         "Deliver the promised resource immediately and set expectations for what's coming next."),
        (3, "Educational content on their pain point",
         "The #1 mistake people make with [pain point]",
         "Teach something genuinely useful about the specific problem they signed up to solve."),
        (7, "Case study / social proof",
         "How [Customer] solved [pain point]",
         "Show a relatable success story that maps to their situation."),
        (10, "Address common objections",
         "\"But what about...?\"",
         "Proactively answer the top 1-2 objections that stop people from converting."),
        (14, "Direct offer / CTA",
         "Ready to [desired outcome]?",
         "Make a direct, low-friction offer with a single clear CTA and a deadline if applicable."),
    ],
    "win_back": [
        (0, "We miss you + what's new",
         "We miss you, [Name]",
         "Acknowledge the gap, briefly note you've been improving, and invite them back with no pressure."),
        (3, "Highlight new features/value since they left",
         "Here's what's new since you've been away",
         "List 2-3 concrete improvements or features shipped since their last visit."),
        (7, "Incentive/discount",
         "A little something to come back to",
         "Offer a time-limited discount or bonus to lower the re-activation barrier."),
        (10, "Final reminder + urgency",
         "Last chance to grab [incentive]",
         "Remind them the offer expires soon and restate the value of returning."),
        (14, "Last-chance / farewell (removes low-intent contacts)",
         "Should we stop emailing you?",
         "Give a final easy opt-in to stay subscribed; anyone who doesn't act is suppressed to protect deliverability."),
    ],
    # Hour-based, kept short by design (a cart-abandonment funnel is inherently short).
    "abandoned_cart": [
        (1, "Reminder — you left something in your cart",
         "You left something behind",
         "Show the exact cart items with images and a single 'Complete your order' CTA — no discount yet."),
        (24, "Urgency — items in your cart are almost gone / social proof",
         "Your cart is about to expire",
         "Add urgency (limited stock/high demand) plus a touch of social proof (reviews/ratings) for the items."),
        (72, "Incentive — discount or free shipping to close the sale",
         "Here's 10% off to finish your order",
         "Offer a modest discount or free shipping as a final nudge to convert before giving up."),
    ],
}

_UNIT = {
    "onboarding": "days", "nurture": "days", "win_back": "days", "abandoned_cart": "hours",
}


def _delay_label(unit, value):
    if unit == "days":
        return "Day %d" % value
    if value == 1:
        return "1 hour"
    return "%d hours" % value


def _build_sequence(campaign_type, num_emails):
    base = _BASE[campaign_type]
    unit = _UNIT[campaign_type]
    n = len(base)

    if num_emails <= n:
        chosen = base[:num_emails]
    else:
        chosen = list(base)
        extra = num_emails - n
        last_value = base[-1][0]
        step = 48 if unit == "hours" else 7
        for k in range(1, extra + 1):
            src = base[(k - 1) % n]
            new_value = last_value + step * k
            chosen.append((
                new_value,
                src[1] + " (follow-up)",
                src[2],
                src[3],
            ))

    sequence = []
    for i, (value, purpose, subject, content) in enumerate(chosen, start=1):
        sequence.append({
            "email_number": i,
            "send_delay": _delay_label(unit, value),
            "purpose": purpose,
            "subject_line_template": subject,
            "key_content_prompt": content,
        })
    return sequence


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"campaign_type": "onboarding", "num_emails": 5}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    campaign_type = q.get("campaign_type")
    if not isinstance(campaign_type, str) or campaign_type.strip().lower() not in _CAMPAIGN_TYPES:
        print(json.dumps({
            "error": "'campaign_type' must be one of the supported types",
            "supported_campaign_types": list(_CAMPAIGN_TYPES),
            "example": example,
        }))
        return 0
    campaign_type = campaign_type.strip().lower()

    num_emails_raw = q.get("num_emails", 5)
    try:
        num_emails = int(num_emails_raw) if num_emails_raw is not None else 5
    except (TypeError, ValueError):
        print(json.dumps({"error": "'num_emails' must be an integer", "example": example}))
        return 0
    num_emails = max(_MIN_EMAILS, min(num_emails, _MAX_EMAILS))

    try:
        sequence = _build_sequence(campaign_type, num_emails)
        result = {
            "campaign_type": campaign_type,
            "num_emails": len(sequence),
            "sequence": sequence,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "drip_campaign_planner failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
