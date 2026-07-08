#!/usr/bin/env python3
"""pricing_psychology — Engram skill (no network). Charm vs. prestige price variants.

Given a price, computes three "charm" price candidates (rounded down to end
in .99/.95/.97) and one "prestige" price candidate (rounded up to a clean,
round whole number), each with a one-line rationale, plus a note on when
each strategy tends to work better. Deterministic arithmetic — no external
data. Stdlib only.

Request (stdin): {"price": 20}
Output (stdout): {original_price, charm_options: [{price, rationale}], prestige_option: {price, rationale}, note}
"""
import json
import math
import sys

_CHARM_SUFFIXES = (0.99, 0.95, 0.97)
_CHARM_RATIONALE = "charm pricing exploits left-digit bias, making the price feel meaningfully cheaper"
_PRESTIGE_RATIONALE = "whole, round numbers signal quality/luxury and avoid appearing 'discount-y'"
_NOTE = (
    "charm pricing (e.g. $X.99) tends to outperform for value/discount "
    "positioning, while whole, round numbers tend to outperform for "
    "premium/luxury positioning — a well-established, widely-replicated "
    "finding in pricing research, though the exact effect size varies by "
    "category and audience."
)


def _charm_options(price):
    base_int = math.floor(price)
    options = []
    for suffix in _CHARM_SUFFIXES:
        candidate = round(base_int + suffix, 2)
        if candidate >= price and base_int >= 1:
            candidate = round(base_int - 1 + suffix, 2)
        options.append({"price": candidate, "rationale": _CHARM_RATIONALE})
    return options


def _prestige_price(price):
    if price != math.floor(price):
        return int(math.ceil(price))
    p_int = int(price)
    if p_int < 20:
        step = 5
    elif p_int < 100:
        step = 10
    elif p_int < 1000:
        step = 50
    else:
        step = 100
    return int(math.ceil((p_int + 1) / step) * step)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {"price": 19.99}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    price = q.get("price")
    if not isinstance(price, (int, float)) or isinstance(price, bool):
        print(json.dumps({
            "error": "missing or invalid required field 'price' (number)",
            "example": example,
        }))
        return 0
    price = float(price)

    if price <= 0:
        print(json.dumps({"error": "'price' must be greater than 0", "example": example}))
        return 0

    try:
        charm_options = _charm_options(price)
        prestige_option = {"price": _prestige_price(price), "rationale": _PRESTIGE_RATIONALE}

        result = {
            "original_price": price,
            "charm_options": charm_options,
            "prestige_option": prestige_option,
            "note": _NOTE,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "pricing_psychology failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
