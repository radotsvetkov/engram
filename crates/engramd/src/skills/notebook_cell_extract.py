#!/usr/bin/env python3
"""notebook_cell_extract — Engram skill (no network). Parse the raw text of a
Jupyter .ipynb file (notebooks are plain JSON) and extract a compact summary
of each cell's type, source, execution count, and output shape — without
dumping potentially huge/binary output content.

Request (stdin): {"notebook": "{\\"cells\\": [...], \\"nbformat\\": 4}"}
Output (stdout): {cell_count, code_cell_count, markdown_cell_count, cells}
"""
import json
import sys


def _join_source(source):
    if isinstance(source, list):
        return "".join(str(s) for s in source)
    if isinstance(source, str):
        return source
    return str(source) if source is not None else ""


def _summarize_outputs(outputs):
    if not isinstance(outputs, list) or not outputs:
        return "no output"
    types = []
    for o in outputs:
        if isinstance(o, dict):
            types.append(str(o.get("output_type", "unknown")))
        else:
            types.append("unknown")
    return "%d output(s): %s" % (len(outputs), types)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"notebook": "<raw .ipynb file text>"},
        }))
        return 0

    notebook_text = q.get("notebook")
    if not isinstance(notebook_text, str) or not notebook_text.strip():
        print(json.dumps({
            "error": "missing required field 'notebook' (raw .ipynb file text, as a string)",
            "example": {"notebook": "{\"cells\": [], \"nbformat\": 4}"},
        }))
        return 0

    try:
        nb = json.loads(notebook_text)
    except Exception as e:
        print(json.dumps({"error": "'notebook' is not valid JSON: %s" % e}))
        return 0

    if not isinstance(nb, dict) or "cells" not in nb:
        print(json.dumps({"error": "not a valid Jupyter notebook — missing 'cells'"}))
        return 0

    cells_raw = nb.get("cells")
    if not isinstance(cells_raw, list):
        print(json.dumps({"error": "not a valid Jupyter notebook — 'cells' is not a list"}))
        return 0

    try:
        cells = []
        code_count = 0
        markdown_count = 0
        for c in cells_raw:
            if not isinstance(c, dict):
                continue
            cell_type = c.get("cell_type", "unknown")
            source = _join_source(c.get("source"))
            if cell_type == "code":
                code_count += 1
                entry = {
                    "cell_type": cell_type,
                    "source": source,
                    "execution_count": c.get("execution_count"),
                    "outputs_summary": _summarize_outputs(c.get("outputs")),
                }
            else:
                if cell_type == "markdown":
                    markdown_count += 1
                entry = {
                    "cell_type": cell_type,
                    "source": source,
                    "execution_count": None,
                    "outputs_summary": None,
                }
            cells.append(entry)

        result = {
            "cell_count": len(cells),
            "code_cell_count": code_count,
            "markdown_cell_count": markdown_count,
            "cells": cells,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "notebook_cell_extract failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
