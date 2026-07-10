#!/usr/bin/env python3
"""few_shot_builder — Engram skill (no network). Assemble a few-shot prompt.

Builds a completion-style prompt string or a chat 'messages' array from an instruction,
a list of {input, output} examples, and a final query. Always returns prompt_text; for
chat format it also returns a messages array. Reports example_count and estimated_tokens
(whole-prompt word_count*1.3).

Request (stdin): {"instruction": "Translate to French", "examples": [{"input": "hi", "output": "salut"}], "query": "bye", "format": "chat"}
Output (stdout): {format, example_count, estimated_tokens, prompt_text, messages?}
"""
import json, sys


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"instruction": "Translate to French",
                        "examples": [{"input": "hi", "output": "salut"}],
                        "query": "bye", "format": "chat"},
        })); return 0

    instruction = q.get("instruction")
    if instruction is None or not isinstance(instruction, str):
        print(json.dumps({
            "error": "missing required field 'instruction' (string)",
            "example": {"instruction": "Translate to French",
                        "examples": [{"input": "hi", "output": "salut"}], "query": "bye"},
        })); return 0

    examples = q.get("examples")
    if not isinstance(examples, list) or not examples:
        print(json.dumps({
            "error": "missing required field 'examples' (non-empty list of {input, output})",
            "example": {"examples": [{"input": "hi", "output": "salut"}]},
        })); return 0

    query = q.get("query")
    if query is None or not isinstance(query, str):
        print(json.dumps({
            "error": "missing required field 'query' (string)",
            "example": {"query": "bye"},
        })); return 0

    fmt = q.get("format", "completion")
    if fmt not in ("chat", "completion"):
        print(json.dumps({"error": "'format' must be 'chat' or 'completion'"})); return 0

    try:
        for ex in examples:
            if not isinstance(ex, dict) or "input" not in ex or "output" not in ex:
                print(json.dumps({
                    "error": "each example must be an object with 'input' and 'output'",
                    "example": {"examples": [{"input": "x", "output": "y"}]},
                })); return 0

        parts = [instruction, ""]
        for ex in examples:
            parts.append("Input: %s" % ex["input"])
            parts.append("Output: %s" % ex["output"])
            parts.append("")
        parts.append("Input: %s" % query)
        parts.append("Output:")
        prompt_text = "\n".join(parts)

        estimated = round(len(prompt_text.split()) * 1.3)
        result = {
            "format": fmt,
            "example_count": len(examples),
            "estimated_tokens": estimated,
            "prompt_text": prompt_text,
        }
        if fmt == "chat":
            messages = [{"role": "system", "content": instruction}]
            for ex in examples:
                messages.append({"role": "user", "content": str(ex["input"])})
                messages.append({"role": "assistant", "content": str(ex["output"])})
            messages.append({"role": "user", "content": query})
            result["messages"] = messages
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "few_shot_builder failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
