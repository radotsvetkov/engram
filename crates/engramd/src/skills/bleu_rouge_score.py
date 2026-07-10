#!/usr/bin/env python3
"""bleu_rouge_score — Engram skill (no network). BLEU and ROUGE-L text-generation eval.

Tokenizes to lowercase words. BLEU: modified n-gram precision for 1..n with a brevity
penalty and a geometric mean (a tiny epsilon avoids log(0)). ROUGE-L: longest common
subsequence -> precision, recall, and F1. Single-reference, rough eval metrics.

Request (stdin): {"candidate": "the cat sat", "reference": "the cat sat", "n": 4}
Output (stdout): {bleu, bleu_precisions, brevity_penalty, rouge_l:{precision,recall,f1}, ...}
"""
import json, sys, math
from collections import Counter


def _ngrams(tokens, k):
    if len(tokens) < k:
        return []
    return [tuple(tokens[i:i + k]) for i in range(len(tokens) - k + 1)]


def _modified_precision(cand, ref, k):
    cand_ng = _ngrams(cand, k)
    if not cand_ng:
        return 0.0
    cand_counts = Counter(cand_ng)
    ref_counts = Counter(_ngrams(ref, k))
    clipped = sum(min(c, ref_counts[ng]) for ng, c in cand_counts.items())
    total = sum(cand_counts.values())
    return clipped / total if total else 0.0


def _lcs_len(a, b):
    m, n = len(a), len(b)
    if m == 0 or n == 0:
        return 0
    prev = [0] * (n + 1)
    for i in range(1, m + 1):
        cur = [0] * (n + 1)
        ai = a[i - 1]
        for j in range(1, n + 1):
            if ai == b[j - 1]:
                cur[j] = prev[j - 1] + 1
            else:
                cur[j] = prev[j] if prev[j] >= cur[j - 1] else cur[j - 1]
        prev = cur
    return prev[n]


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"candidate": "the cat sat", "reference": "the cat sat on the mat", "n": 4},
        })); return 0

    candidate = q.get("candidate")
    reference = q.get("reference")
    if not isinstance(candidate, str) or not isinstance(reference, str):
        print(json.dumps({
            "error": "missing required fields 'candidate' and 'reference' (strings)",
            "example": {"candidate": "the cat sat", "reference": "the cat sat on the mat", "n": 4},
        })); return 0

    n = q.get("n", 4)
    if not isinstance(n, int) or isinstance(n, bool) or n < 1 or n > 10:
        print(json.dumps({"error": "'n' must be an integer in 1..10"})); return 0

    try:
        cand = candidate.lower().split()
        ref = reference.lower().split()
        c = len(cand)
        r = len(ref)

        if c == 0:
            bp = 0.0
        elif c > r:
            bp = 1.0
        else:
            bp = math.exp(1 - r / c)

        eps = 1e-9
        precisions = []
        log_sum = 0.0
        for k in range(1, n + 1):
            p = _modified_precision(cand, ref, k)
            precisions.append(round(p, 6))
            log_sum += math.log(p + eps)
        geo = math.exp(log_sum / n)
        bleu = bp * geo

        lcs = _lcs_len(cand, ref)
        rl_prec = lcs / c if c else 0.0
        rl_rec = lcs / r if r else 0.0
        rl_f1 = (2 * rl_prec * rl_rec / (rl_prec + rl_rec)) if (rl_prec + rl_rec) else 0.0

        result = {
            "bleu": round(bleu, 6),
            "bleu_precisions": precisions,
            "brevity_penalty": round(bp, 6),
            "n": n,
            "candidate_length": c,
            "reference_length": r,
            "rouge_l": {
                "precision": round(rl_prec, 6),
                "recall": round(rl_rec, 6),
                "f1": round(rl_f1, 6),
                "lcs": lcs,
            },
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "bleu_rouge_score failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
