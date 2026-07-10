#!/usr/bin/env python3
"""classification_metrics — Engram skill (no network). Score classifier output.

Given true and predicted label sequences of equal length (string or int
labels), builds the confusion matrix and reports per-class precision, recall,
F1 and support, plus overall accuracy and macro-averaged precision/recall/F1.

Request (stdin): {"y_true": ["cat","dog","cat","bird"], "y_pred": ["cat","dog","dog","bird"]}
Output (stdout): {labels, confusion_matrix, per_class, accuracy, macro_*}
"""
import json, sys


def _f1(p, r):
    return 0.0 if (p + r) == 0 else 2 * p * r / (p + r)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e})); return 0

    ex = {"y_true": ["cat", "dog", "cat", "bird"], "y_pred": ["cat", "dog", "dog", "bird"]}
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": ex})); return 0

    def _labellist(v):
        return isinstance(v, list) and all(isinstance(t, (str, int)) and not isinstance(t, bool) for t in v)

    y_true = q.get("y_true")
    y_pred = q.get("y_pred")
    if not _labellist(y_true) or not _labellist(y_pred):
        print(json.dumps({"error": "'y_true' and 'y_pred' must be lists of string/int labels", "example": ex})); return 0
    if len(y_true) != len(y_pred):
        print(json.dumps({"error": "'y_true' and 'y_pred' must have equal length (got %d and %d)" % (len(y_true), len(y_pred)), "example": ex})); return 0
    if len(y_true) == 0:
        print(json.dumps({"error": "label sequences are empty", "example": ex})); return 0

    try:
        # Sort labels; keep as strings for stable JSON keys but sort by (type, value).
        uniq = set(y_true) | set(y_pred)
        labels = sorted(uniq, key=lambda x: (isinstance(x, str), x))
        lstr = [str(l) for l in labels]

        # Confusion matrix: cm[true][pred].
        cm = {a: {b: 0 for b in lstr} for a in lstr}
        for t, p in zip(y_true, y_pred):
            cm[str(t)][str(p)] += 1

        n = len(y_true)
        correct = sum(cm[a][a] for a in lstr)
        accuracy = correct / n

        per_class = {}
        for a in lstr:
            tp = cm[a][a]
            fp = sum(cm[b][a] for b in lstr if b != a)   # predicted a, actually other
            fn = sum(cm[a][b] for b in lstr if b != a)   # actually a, predicted other
            support = tp + fn
            precision = 0.0 if (tp + fp) == 0 else tp / (tp + fp)
            recall = 0.0 if (tp + fn) == 0 else tp / (tp + fn)
            per_class[a] = {
                "precision": round(precision, 6),
                "recall": round(recall, 6),
                "f1": round(_f1(precision, recall), 6),
                "support": support,
            }

        k = len(lstr)
        macro_p = sum(per_class[a]["precision"] for a in lstr) / k
        macro_r = sum(per_class[a]["recall"] for a in lstr) / k
        macro_f1 = sum(per_class[a]["f1"] for a in lstr) / k

        result = {
            "labels": lstr,
            "confusion_matrix": cm,
            "per_class": per_class,
            "accuracy": round(accuracy, 6),
            "macro_precision": round(macro_p, 6),
            "macro_recall": round(macro_r, 6),
            "macro_f1": round(macro_f1, 6),
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "classification_metrics failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
