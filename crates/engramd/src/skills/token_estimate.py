#!/usr/bin/env python3
"""token_estimate — Engram skill (no network). Rough LLM token count via heuristic.

No tokenizer is available (stdlib only), so tokens are estimated as
max(word_count*1.3, char_count/4) — a rough approximation (~±20%), NOT an exact
tokenizer. Optionally reports the named model's context window and how much of it
the estimate would consume.

Request (stdin): {"text": "some text", "model": "claude-4"}
Output (stdout): {estimated_tokens, char_count, word_count, method, note, context_window?, pct_of_context?}
"""
import json, sys

CONTEXT_WINDOWS = {
    "gpt-4o": 128000,
    "claude-3.5": 200000,
    "claude-4": 200000,
    "llama-3-8b": 8192,
    "mistral-7b": 32768,
    "generic": 8192,
}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"text": "Hello, world!", "model": "claude-4"},
        })); return 0

    text = q.get("text")
    if text is None or not isinstance(text, str):
        print(json.dumps({
            "error": "missing required field 'text' (string)",
            "example": {"text": "Hello, world!", "model": "claude-4"},
        })); return 0

    try:
        char_count = len(text)
        word_count = len(text.split())
        estimated = round(max(word_count * 1.3, char_count / 4))
        result = {
            "estimated_tokens": estimated,
            "char_count": char_count,
            "word_count": word_count,
            "method": "max(word_count*1.3, char_count/4)",
            "note": "Rough heuristic (~±20%), NOT an exact tokenizer.",
        }
        model = q.get("model")
        if model is not None:
            if not isinstance(model, str):
                print(json.dumps({
                    "error": "'model' must be a string",
                    "known_models": sorted(CONTEXT_WINDOWS),
                })); return 0
            cw = CONTEXT_WINDOWS.get(model)
            result["model"] = model
            if cw is None:
                cw = CONTEXT_WINDOWS["generic"]
                result["model_note"] = "unknown model, using 'generic' context window (%d)" % cw
            result["context_window"] = cw
            result["pct_of_context"] = round(100.0 * estimated / cw, 2) if cw else None
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "token_estimate failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
