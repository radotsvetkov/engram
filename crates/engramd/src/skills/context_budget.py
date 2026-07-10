#!/usr/bin/env python3
"""context_budget — Engram skill (no network). Check whether a prompt fits a context window.

Sums system prompt + history + next message + desired output tokens against the context
window. Each part accepts either a numeric '<part>_tokens' OR a '<part>' text string
(estimated as word_count*1.3). Reports the breakdown, whether it fits, utilization, and
tokens_to_trim when over budget.

Request (stdin): {"context_window": 128000, "system_prompt_tokens": 200, "history_tokens": 1500, "desired_output_tokens": 1000, "next_message": "hello there"}
Output (stdout): {context_window, breakdown, used_tokens, remaining_tokens, fits, utilization_pct, recommendation}
"""
import json, sys


def _resolve(q, base, default):
    tk = q.get(base + "_tokens")
    if tk is not None:
        if isinstance(tk, bool) or not isinstance(tk, (int, float)):
            return None, "'%s_tokens' must be a number" % base
        if tk < 0:
            return None, "'%s_tokens' must be non-negative" % base
        return int(round(tk)), None
    txt = q.get(base)
    if txt is not None:
        if not isinstance(txt, str):
            return None, "'%s' must be a string (text) or use '%s_tokens' (number)" % (base, base)
        return int(round(len(txt.split()) * 1.3)), None
    return default, None


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"context_window": 128000, "system_prompt_tokens": 200,
                        "history_tokens": 1500, "desired_output_tokens": 1000},
        })); return 0

    cw = q.get("context_window")
    if isinstance(cw, bool) or not isinstance(cw, (int, float)) or cw < 1:
        print(json.dumps({
            "error": "missing required field 'context_window' (positive number)",
            "example": {"context_window": 128000, "system_prompt_tokens": 200,
                        "history_tokens": 1500, "desired_output_tokens": 1000},
        })); return 0
    cw = int(cw)

    try:
        system, err = _resolve(q, "system_prompt", 0)
        if err:
            print(json.dumps({"error": err})); return 0
        history, err = _resolve(q, "history", 0)
        if err:
            print(json.dumps({"error": err})); return 0
        next_message, err = _resolve(q, "next_message", 0)
        if err:
            print(json.dumps({"error": err})); return 0
        desired, err = _resolve(q, "desired_output", 1000)
        if err:
            print(json.dumps({"error": err})); return 0

        used = system + history + next_message + desired
        remaining = cw - used
        fits = remaining >= 0
        util = round(100.0 * used / cw, 2)

        result = {
            "context_window": cw,
            "breakdown": {
                "system_prompt_tokens": system,
                "history_tokens": history,
                "next_message_tokens": next_message,
                "desired_output_tokens": desired,
            },
            "used_tokens": used,
            "remaining_tokens": remaining,
            "fits": fits,
            "utilization_pct": util,
        }
        if fits:
            result["recommendation"] = (
                "Fits: %d tokens to spare (%.1f%% of the window used)." % (remaining, util)
            )
        else:
            over = -remaining
            result["tokens_to_trim"] = over
            result["recommendation"] = (
                "Over budget by %d tokens. Trim history/system prompt or lower "
                "desired_output_tokens by at least %d tokens." % (over, over)
            )
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "context_budget failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
