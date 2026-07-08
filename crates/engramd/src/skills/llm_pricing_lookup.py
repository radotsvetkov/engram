#!/usr/bin/env python3
"""llm_pricing_lookup — Engram skill (no network). Look up rough,
order-of-magnitude LLM API pricing patterns from a small static reference
table. Pricing changes constantly, so every response carries a prominent
disclaimer to verify against the provider's official page.

Request (stdin): {"provider": "anthropic", "model": "sonnet"}
Output (stdout): {disclaimer, provider, models} or {disclaimer, providers}
"""
import json
import sys

_DISCLAIMER = (
    "LLM pricing changes frequently — verify current numbers on the provider's "
    "official pricing page before using these for cost estimates."
)

# Rough, plausible order-of-magnitude figures (USD per 1M tokens) reflecting
# typical frontier/mid/small tier pricing patterns. NOT guaranteed current.
_TABLE = {
    "anthropic": {
        "claude-opus": {
            "tier": "frontier",
            "price_per_million_input_tokens": 15.0,
            "price_per_million_output_tokens": 75.0,
        },
        "claude-sonnet": {
            "tier": "mid",
            "price_per_million_input_tokens": 3.0,
            "price_per_million_output_tokens": 15.0,
        },
        "claude-haiku": {
            "tier": "small/fast",
            "price_per_million_input_tokens": 0.25,
            "price_per_million_output_tokens": 1.25,
        },
    },
    "openai": {
        "gpt-4o": {
            "tier": "frontier",
            "price_per_million_input_tokens": 5.0,
            "price_per_million_output_tokens": 15.0,
        },
        "gpt-4o-mini": {
            "tier": "small/fast",
            "price_per_million_input_tokens": 0.15,
            "price_per_million_output_tokens": 0.60,
        },
        "o1": {
            "tier": "frontier reasoning",
            "price_per_million_input_tokens": 15.0,
            "price_per_million_output_tokens": 60.0,
        },
    },
    "google": {
        "gemini-1.5-pro": {
            "tier": "frontier",
            "price_per_million_input_tokens": 3.5,
            "price_per_million_output_tokens": 10.5,
        },
        "gemini-1.5-flash": {
            "tier": "small/fast",
            "price_per_million_input_tokens": 0.075,
            "price_per_million_output_tokens": 0.30,
        },
    },
}


def _fuzzy_match_provider(name):
    name_l = name.strip().lower()
    for key in _TABLE:
        if name_l == key or name_l in key or key in name_l:
            return key
    return None


def _fuzzy_match_models(models_dict, model_query):
    q = model_query.strip().lower()
    return {k: v for k, v in models_dict.items() if q in k.lower()}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"provider": "anthropic", "model": "sonnet"},
        }))
        return 0

    provider = q.get("provider")
    model = q.get("model")
    if provider is not None and not isinstance(provider, str):
        print(json.dumps({"error": "'provider' must be a string if provided"}))
        return 0
    if model is not None and not isinstance(model, str):
        print(json.dumps({"error": "'model' must be a string if provided"}))
        return 0

    try:
        if not provider:
            print(json.dumps({
                "disclaimer": _DISCLAIMER,
                "providers": _TABLE,
            }, indent=2, default=str))
            return 0

        matched = _fuzzy_match_provider(provider)
        if not matched:
            print(json.dumps({
                "error": "unrecognized provider %r" % provider,
                "supported_providers": sorted(_TABLE),
            }))
            return 0

        models_for_provider = _TABLE[matched]
        if not model:
            print(json.dumps({
                "disclaimer": _DISCLAIMER,
                "provider": matched,
                "models": models_for_provider,
            }, indent=2, default=str))
            return 0

        filtered = _fuzzy_match_models(models_for_provider, model)
        if not filtered:
            print(json.dumps({
                "disclaimer": _DISCLAIMER,
                "provider": matched,
                "note": "no exact match for model %r under %r — showing the full provider list" % (model, matched),
                "models": models_for_provider,
            }, indent=2, default=str))
            return 0

        print(json.dumps({
            "disclaimer": _DISCLAIMER,
            "provider": matched,
            "models": filtered,
        }, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "llm_pricing_lookup failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
