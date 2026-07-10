#!/usr/bin/env python3
"""db_normalization_check — Engram skill (no network). Analyse a relation
against 1NF / 2NF / 3NF using its columns, primary key, and functional
dependencies.

1NF is assumed (atomic values — noted, not verified). 2NF flags partial
dependencies (a non-key attribute determined by PART of a composite key). 3NF
flags transitive dependencies (non-key determining non-key). Heuristic — it
reasons only about the FDs you supply, so undeclared dependencies are invisible.

Request (stdin): {"columns": ["order_id","product_id","product_name","qty"],
  "primary_key": ["order_id","product_id"],
  "functional_dependencies": [{"determinant": ["product_id"], "dependent": ["product_name"]}]}
Output (stdout): {highest_normal_form_satisfied, assumptions, violations, notes}
"""
import json
import sys


def _as_str_list(v):
    if isinstance(v, list):
        return [str(x).strip() for x in v if str(x).strip()]
    if isinstance(v, str) and v.strip():
        return [v.strip()]
    return []


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"columns": ["order_id", "product_id", "product_name"],
                        "primary_key": ["order_id", "product_id"],
                        "functional_dependencies": [
                            {"determinant": ["product_id"], "dependent": ["product_name"]}]},
        }))
        return 0

    example = {
        "columns": ["order_id", "product_id", "product_name", "qty"],
        "primary_key": ["order_id", "product_id"],
        "functional_dependencies": [
            {"determinant": ["product_id"], "dependent": ["product_name"]}],
    }

    columns = _as_str_list(q.get("columns"))
    primary_key = _as_str_list(q.get("primary_key"))
    if not columns:
        print(json.dumps({"error": "missing required field 'columns' (list of column names)", "example": example}))
        return 0
    if not primary_key:
        print(json.dumps({"error": "missing required field 'primary_key' (list of PK column names)", "example": example}))
        return 0

    try:
        col_set = set(columns)
        pk_set = set(primary_key)
        notes = []

        # Structural sanity: every PK column should exist in columns.
        unknown_pk = [c for c in primary_key if c not in col_set]
        if unknown_pk:
            notes.append("primary_key columns not found in columns: %s" % ", ".join(unknown_pk))

        non_key = [c for c in columns if c not in pk_set]
        is_composite_pk = len(pk_set) > 1

        assumptions = [
            "1NF is assumed: all values are treated as atomic (single-valued, no "
            "repeating groups). This tool does not inspect actual data to verify atomicity.",
        ]

        raw_fds = q.get("functional_dependencies")
        fds = []
        if isinstance(raw_fds, list):
            for fd in raw_fds:
                if not isinstance(fd, dict):
                    continue
                det = _as_str_list(fd.get("determinant"))
                dep = _as_str_list(fd.get("dependent"))
                if det and dep:
                    fds.append((det, dep))

        violations = []

        if not fds:
            notes.append(
                "no functional_dependencies supplied — only structural 1NF/PK checks were "
                "performed. Provide FDs to detect 2NF (partial) and 3NF (transitive) violations.")
            highest = "1NF"
            result = {
                "highest_normal_form_satisfied": highest,
                "assumptions": assumptions,
                "violations": violations,
                "primary_key": primary_key,
                "non_key_attributes": non_key,
                "notes": notes,
            }
            print(json.dumps(result, indent=2, default=str))
            return 0

        # 2NF: for a composite PK, a non-prime attribute must not depend on a
        # PROPER SUBSET of the PK. (With a single-column PK, no partial dependency
        # is possible — 2NF is automatic given 1NF.)
        has_2nf_violation = False
        if is_composite_pk:
            for det, dep in fds:
                det_set = set(det)
                # determinant is a strict, non-empty subset of the PK
                if det_set and det_set < pk_set:
                    partial_deps = [d for d in dep if d in col_set and d not in pk_set]
                    if partial_deps:
                        has_2nf_violation = True
                        new_cols = sorted(det_set) + partial_deps
                        violations.append({
                            "normal_form": "2NF",
                            "fd": "%s -> %s" % (", ".join(det), ", ".join(dep)),
                            "explanation": (
                                "partial dependency: non-key attribute(s) %s depend on %s, "
                                "which is only PART of the composite primary key (%s). 2NF "
                                "requires every non-key attribute to depend on the WHOLE key."
                                % (", ".join(partial_deps), ", ".join(det), ", ".join(primary_key))),
                            "suggested_table": {
                                "columns": new_cols,
                                "primary_key": sorted(det_set),
                                "note": "move %s into a table keyed by %s" % (
                                    ", ".join(partial_deps), ", ".join(sorted(det_set))),
                            },
                        })
        else:
            notes.append("primary key is a single column, so no partial dependencies are "
                         "possible — 2NF holds automatically given 1NF.")

        # 3NF: no transitive dependency — a non-prime attribute must not be
        # determined by another non-prime attribute (determinant not a superkey).
        has_3nf_violation = False
        for det, dep in fds:
            det_set = set(det)
            # Non-prime determinant (none of it is the whole key / not a superkey here).
            if det_set and not (det_set >= pk_set) and any(d in det_set for d in non_key):
                trans_deps = [d for d in dep if d in col_set and d not in pk_set and d not in det_set]
                if trans_deps:
                    has_3nf_violation = True
                    new_cols = sorted(det_set) + trans_deps
                    violations.append({
                        "normal_form": "3NF",
                        "fd": "%s -> %s" % (", ".join(det), ", ".join(dep)),
                        "explanation": (
                            "transitive dependency: non-key attribute(s) %s depend on %s, which "
                            "is itself a non-key attribute (not a superkey). 3NF requires non-key "
                            "attributes to depend only on the key." % (", ".join(trans_deps), ", ".join(det))),
                        "suggested_table": {
                            "columns": new_cols,
                            "primary_key": sorted(det_set),
                            "note": "move %s into a table keyed by %s" % (
                                ", ".join(trans_deps), ", ".join(sorted(det_set))),
                        },
                    })

        if has_2nf_violation:
            highest = "1NF"
        elif has_3nf_violation:
            highest = "2NF"
        else:
            highest = "3NF"
            notes.append("no partial or transitive dependencies detected in the supplied FDs; "
                         "relation appears to satisfy 3NF (and is a BCNF-candidate if every "
                         "determinant is a superkey).")

        result = {
            "highest_normal_form_satisfied": highest,
            "assumptions": assumptions,
            "primary_key": primary_key,
            "non_key_attributes": non_key,
            "violations": violations,
            "notes": notes,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "db_normalization_check failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
