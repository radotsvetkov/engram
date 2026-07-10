#!/usr/bin/env python3
"""pivot_table — Engram skill (no network). Group-and-aggregate rows into a pivot.

Groups a list of row-objects by an `index` field (and optionally cross-tabs by a
`columns` field), aggregating the `values` field with aggfunc (sum/count/mean/
min/max/median). Non-numeric values are skipped for numeric aggs (and noted).
count works on any field. Missing index/values fields are skipped and noted.

Request (stdin): {"rows": [obj], "index": "region", "columns"?: "product",
                  "values": "sales", "aggfunc"?: "sum"}
Output (stdout): {pivot, row_totals?, grand_total?, aggfunc, skipped, notes}
"""
import json, sys, statistics


def _num(v):
    if isinstance(v, bool):
        return None
    if isinstance(v, (int, float)):
        return v
    if isinstance(v, str):
        try:
            return float(v)
        except ValueError:
            return None
    return None


def _agg(values, fn):
    """values: raw cell values collected for a group. Returns (result, skipped)."""
    if fn == "count":
        return len(values), 0
    nums = []
    skipped = 0
    for v in values:
        n = _num(v)
        if n is None:
            skipped += 1
        else:
            nums.append(n)
    if not nums:
        return None, skipped
    if fn == "sum":
        r = sum(nums)
    elif fn == "mean":
        r = statistics.fmean(nums)
    elif fn == "min":
        r = min(nums)
    elif fn == "max":
        r = max(nums)
    elif fn == "median":
        r = statistics.median(nums)
    else:
        r = sum(nums)
    if isinstance(r, float):
        r = round(r, 6)
    return r, skipped


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    ex = {"rows": [{"region": "N", "product": "A", "sales": 10},
                   {"region": "N", "product": "B", "sales": 5},
                   {"region": "S", "product": "A", "sales": 7}],
          "index": "region", "columns": "product", "values": "sales", "aggfunc": "sum"}

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": ex})); return 0

    rows = q.get("rows")
    if not isinstance(rows, list):
        print(json.dumps({"error": "missing required field 'rows' (a list of objects)", "example": ex})); return 0

    index = q.get("index")
    if not isinstance(index, str) or index == "":
        print(json.dumps({"error": "missing required field 'index' (a field name to group by)", "example": ex})); return 0

    values = q.get("values")
    columns = q.get("columns")
    aggfunc = (q.get("aggfunc") or "sum")
    if not isinstance(aggfunc, str):
        aggfunc = "sum"
    aggfunc = aggfunc.lower().strip()
    if aggfunc not in ("sum", "count", "mean", "min", "max", "median"):
        print(json.dumps({
            "error": "unknown aggfunc: %s" % aggfunc,
            "how_to_fix": "use one of: sum, count, mean, min, max, median",
        })); return 0

    # 'values' required except for count (count can use any present field / row existence).
    if aggfunc != "count" and (not isinstance(values, str) or values == ""):
        print(json.dumps({
            "error": "field 'values' is required for aggfunc '%s'" % aggfunc,
            "example": ex,
        })); return 0

    try:
        notes = []
        skipped_missing_index = 0
        # groups[index_val][col_val] = [raw cell values]
        groups = {}
        col_order = []
        idx_order = []
        for row in rows:
            if not isinstance(row, dict):
                continue
            if index not in row:
                skipped_missing_index += 1
                continue
            iv = str(row[index])
            if iv not in groups:
                groups[iv] = {}
                idx_order.append(iv)
            if isinstance(columns, str) and columns != "":
                cv = str(row[columns]) if columns in row else "(missing)"
            else:
                cv = "value"
            if cv not in col_order:
                col_order.append(cv)
            # cell value to aggregate
            if aggfunc == "count" and (not isinstance(values, str) or values == ""):
                cell = 1  # count rows
            else:
                cell = row.get(values)
            groups[iv].setdefault(cv, []).append(cell)

        if skipped_missing_index:
            notes.append("%d row(s) skipped: missing index field '%s'" % (skipped_missing_index, index))

        total_skipped_nonnumeric = 0
        pivot = {}
        row_totals = {}
        numeric_totals_ok = aggfunc in ("sum", "count")
        for iv in idx_order:
            pivot[iv] = {}
            for cv in col_order:
                cell_vals = groups[iv].get(cv)
                if cell_vals is None:
                    continue
                res, sk = _agg(cell_vals, aggfunc)
                total_skipped_nonnumeric += sk
                pivot[iv][cv] = res
            if numeric_totals_ok:
                rt, _ = _agg([v for cvals in groups[iv].values() for v in cvals], aggfunc)
                row_totals[iv] = rt

        result = {"pivot": pivot, "aggfunc": aggfunc}
        if numeric_totals_ok:
            result["row_totals"] = row_totals
            gt, _ = _agg([v for iv in idx_order for cvals in groups[iv].values() for v in cvals], aggfunc)
            result["grand_total"] = gt

        if total_skipped_nonnumeric:
            notes.append("%d non-numeric value(s) skipped for aggfunc '%s'" % (total_skipped_nonnumeric, aggfunc))
        result["skipped_nonnumeric"] = total_skipped_nonnumeric
        result["notes"] = notes
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "pivot_table failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
