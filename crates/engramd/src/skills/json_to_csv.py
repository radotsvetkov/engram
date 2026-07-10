#!/usr/bin/env python3
"""json_to_csv — Engram skill (no network). Convert JSON records into CSV text.

Accepts a JSON array of objects (or a JSON string of one) and emits CSV text
with a header row. Columns default to the union of all keys in first-seen order
(or the caller's `columns`). Fields with the delimiter/quote/newline are quoted
by stdlib `csv`; nested values are flattened to their compact JSON string.

Request (stdin): {"json": [{...}], "delimiter"?: ",", "columns"?: ["a","b"]}
Output (stdout): {csv, row_count, columns}
"""
import json, sys, csv, io


def _cell(v):
    if v is None:
        return ""
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, (dict, list)):
        return json.dumps(v, separators=(",", ":"), default=str)
    return str(v)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"json": [{"a": 1, "b": 2}]},
        })); return 0

    if "json" not in q:
        print(json.dumps({
            "error": "missing required field 'json' (a JSON array of objects, or a JSON string of one)",
            "example": {"json": [{"name": "Alice", "age": 30}], "delimiter": ","},
        })); return 0

    raw = q.get("json")
    if isinstance(raw, str):
        try:
            data = json.loads(raw)
        except Exception as e:
            print(json.dumps({"error": "invalid JSON in 'json': %s" % e})); return 0
    else:
        data = raw

    # Accept a single object as a one-row table.
    if isinstance(data, dict):
        data = [data]
    if not isinstance(data, list):
        print(json.dumps({
            "error": "'json' must be an array of objects (or a single object)",
            "example": {"json": [{"a": 1}, {"a": 2, "b": 3}]},
        })); return 0

    delimiter = q.get("delimiter") or ","
    if not isinstance(delimiter, str) or len(delimiter) != 1:
        print(json.dumps({
            "error": "'delimiter' must be a single character",
            "example": {"json": [{"a": 1}], "delimiter": ";"},
        })); return 0

    try:
        columns = q.get("columns")
        if columns is not None:
            if not (isinstance(columns, list) and all(isinstance(c, str) for c in columns)):
                print(json.dumps({
                    "error": "'columns' must be a list of strings",
                    "example": {"json": [{"a": 1, "b": 2}], "columns": ["a", "b"]},
                })); return 0
        else:
            # Union of keys, preserving first-seen order.
            columns = []
            seen = set()
            for item in data:
                if isinstance(item, dict):
                    for k in item.keys():
                        if k not in seen:
                            seen.add(k)
                            columns.append(k)
            if not columns:
                print(json.dumps({"error": "no object keys found; provide 'columns' or objects with keys"})); return 0

        out = io.StringIO()
        writer = csv.writer(out, delimiter=delimiter, lineterminator="\n")
        writer.writerow(columns)
        for item in data:
            if isinstance(item, dict):
                writer.writerow([_cell(item.get(c)) for c in columns])
            elif isinstance(item, list):
                # Positional mapping onto columns.
                row = [_cell(item[i]) if i < len(item) else "" for i in range(len(columns))]
                writer.writerow(row)
            else:
                # Scalar row: place in first column.
                writer.writerow([_cell(item)] + [""] * (len(columns) - 1))

        result = {
            "csv": out.getvalue(),
            "row_count": len(data),
            "columns": columns,
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "json_to_csv failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
