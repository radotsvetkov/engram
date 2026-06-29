#!/usr/bin/env python3
"""csv_stats — Engram skill (no network). Per-column statistics from CSV text.

Parses CSV text (first row = headers) and computes per-column stats. Numeric
columns (all non-empty values parse as float) report min/max/mean/median/sum;
other columns report distinct count and the most common value.
Request: {"csv": "<csv text>", "delimiter"?: ","}.
Output: {"rows": N, "columns": [...], "stats": {header: {...}}}.
"""
import json, sys, csv, io, statistics
from collections import Counter


def _is_float(s):
    try:
        float(s)
        return True
    except (TypeError, ValueError):
        return False


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"csv": "a,b\n1,x\n2,y", "delimiter": ","},
        })); return 0

    text = q.get("csv")
    if not isinstance(text, str) or not text.strip():
        print(json.dumps({
            "error": "missing required field 'csv' (the CSV text as a string)",
            "example": {"csv": "name,age\nAlice,30\nBob,25", "delimiter": ","},
        })); return 0

    delimiter = q.get("delimiter") or ","
    if not isinstance(delimiter, str) or len(delimiter) != 1:
        print(json.dumps({
            "error": "'delimiter' must be a single character",
            "example": {"csv": "a;b\n1;2", "delimiter": ";"},
        })); return 0

    try:
        reader = csv.reader(io.StringIO(text), delimiter=delimiter)
        all_rows = [row for row in reader]
    except Exception as e:
        print(json.dumps({"error": "empty or unparseable CSV"})); return 0

    # Drop fully empty leading rows; first non-empty row = headers.
    while all_rows and (len(all_rows[0]) == 0 or all(c == "" for c in all_rows[0])):
        all_rows.pop(0)

    if not all_rows:
        print(json.dumps({"error": "empty or unparseable CSV"})); return 0

    try:
        headers = [str(h) for h in all_rows[0]]
        data_rows = all_rows[1:]

        if not headers:
            print(json.dumps({"error": "empty or unparseable CSV"})); return 0

        # Collect non-empty values per column index, robust to ragged rows.
        col_values = {i: [] for i in range(len(headers))}
        for row in data_rows:
            for i in range(len(headers)):
                val = row[i] if i < len(row) else ""
                if val is not None and str(val).strip() != "":
                    col_values[i].append(str(val).strip())

        stats = {}
        for i, header in enumerate(headers):
            values = col_values[i]
            count = len(values)
            if count > 0 and all(_is_float(v) for v in values):
                nums = [float(v) for v in values]
                col_stat = {
                    "type": "number",
                    "count": count,
                    "min": round(min(nums), 4),
                    "max": round(max(nums), 4),
                    "mean": round(statistics.fmean(nums), 4),
                    "median": round(statistics.median(nums), 4),
                    "sum": round(sum(nums), 4),
                }
            else:
                counter = Counter(values)
                top = counter.most_common(1)[0][0] if counter else None
                col_stat = {
                    "type": "text",
                    "count": count,
                    "unique": len(counter),
                    "top": top,
                }
            # Disambiguate duplicate header names so no column is lost.
            key = header
            if key in stats:
                n = 2
                while ("%s_%d" % (header, n)) in stats:
                    n += 1
                key = "%s_%d" % (header, n)
            stats[key] = col_stat

        result = {
            "rows": len(data_rows),
            "columns": headers,
            "stats": stats,
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "csv_stats failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
