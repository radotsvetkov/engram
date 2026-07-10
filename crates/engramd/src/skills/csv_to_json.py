#!/usr/bin/env python3
"""csv_to_json — Engram skill (no network). Convert CSV text into JSON rows.

Parses CSV text (stdlib `csv`, so quoted fields with embedded commas/newlines
work) into a JSON array of objects keyed by the header row, or arrays when
has_header is false. Ragged rows are padded/truncated to the header width.

Request (stdin): {"csv": "a,b\\n1,2", "delimiter"?: ",", "has_header"?: true}
Output (stdout): {rows, row_count, columns, ragged_rows_adjusted}
"""
import json, sys, csv, io


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"csv": "name,age\nAlice,30", "has_header": True},
        })); return 0

    text = q.get("csv")
    if not isinstance(text, str) or text.strip() == "":
        print(json.dumps({
            "error": "missing required field 'csv' (the CSV text as a string)",
            "example": {"csv": "name,age\nAlice,30\nBob,25", "delimiter": ",", "has_header": True},
        })); return 0

    delimiter = q.get("delimiter") or ","
    if not isinstance(delimiter, str) or len(delimiter) != 1:
        print(json.dumps({
            "error": "'delimiter' must be a single character",
            "example": {"csv": "a;b\n1;2", "delimiter": ";"},
        })); return 0

    has_header = q.get("has_header")
    if has_header is None:
        has_header = True

    try:
        reader = csv.reader(io.StringIO(text), delimiter=delimiter)
        all_rows = [row for row in reader]
    except Exception as e:
        print(json.dumps({"error": "could not parse CSV: %s" % e})); return 0

    # Drop fully empty leading rows.
    while all_rows and (len(all_rows[0]) == 0 or all(c == "" for c in all_rows[0])):
        all_rows.pop(0)

    if not all_rows:
        print(json.dumps({"error": "no data rows found in CSV"})); return 0

    try:
        ragged = 0
        if has_header:
            headers = [str(h) for h in all_rows[0]]
            width = len(headers)
            data_rows = all_rows[1:]
            rows = []
            for row in data_rows:
                if len(row) != width:
                    ragged += 1
                obj = {}
                for i, h in enumerate(headers):
                    key = h
                    # Disambiguate duplicate header names so no column is lost.
                    if key in obj:
                        n = 2
                        while ("%s_%d" % (h, n)) in obj:
                            n += 1
                        key = "%s_%d" % (h, n)
                    obj[key] = row[i] if i < len(row) else ""
                rows.append(obj)
            columns = headers
        else:
            # No header: emit list-of-lists, padded/truncated to the widest row.
            width = max(len(r) for r in all_rows)
            rows = []
            for row in all_rows:
                if len(row) != width:
                    ragged += 1
                padded = list(row) + [""] * (width - len(row))
                rows.append(padded[:width])
            columns = list(range(width))

        result = {
            "rows": rows,
            "row_count": len(rows),
            "columns": columns,
            "ragged_rows_adjusted": ragged,
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "csv_to_json failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
