#!/usr/bin/env python3
"""fastapi_endpoint_scaffold — Engram skill (no network). Generate a FastAPI
APIRouter boilerplate file for a resource, with a Pydantic model stub and one
route function per requested HTTP method.

Request (stdin): {"resource": "items", "methods": ["GET", "POST", "PUT", "DELETE"]}
Output (stdout): {filename, code}
"""
import json
import re
import sys

_SUPPORTED = ["GET", "POST", "PUT", "PATCH", "DELETE"]


def _to_pascal_case(name):
    parts = re.split(r"[^A-Za-z0-9]+", name.strip())
    return "".join(p[:1].upper() + p[1:] for p in parts if p)


def _singularize(word):
    if word.endswith("ies") and len(word) > 3:
        return word[:-3] + "y"
    if word.endswith("ses") and len(word) > 3:
        return word[:-2]
    if word.endswith("s") and not word.endswith("ss") and len(word) > 1:
        return word[:-1]
    return word


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"resource": "items", "methods": ["GET", "POST"]},
        }))
        return 0

    resource = q.get("resource")
    if not isinstance(resource, str) or not resource.strip():
        print(json.dumps({
            "error": "missing required field 'resource' (non-empty string)",
            "example": {"resource": "items", "methods": ["GET", "POST", "PUT", "DELETE"]},
        }))
        return 0
    resource = resource.strip().strip("/")
    if not re.match(r"^[A-Za-z0-9_-]+$", resource):
        print(json.dumps({"error": "'resource' must be a simple path segment (letters, numbers, -, _), got %r" % resource}))
        return 0

    methods = q.get("methods")
    if methods is None:
        methods = ["GET", "POST", "PUT", "DELETE"]
    if not isinstance(methods, list) or not methods:
        print(json.dumps({
            "error": "'methods' must be a non-empty list of strings if provided",
            "supported_methods": _SUPPORTED,
        }))
        return 0

    normalized = []
    unknown = []
    for m in methods:
        if not isinstance(m, str):
            unknown.append(m)
            continue
        um = m.strip().upper()
        if um in _SUPPORTED:
            if um not in normalized:
                normalized.append(um)
        else:
            unknown.append(m)

    if unknown:
        print(json.dumps({
            "error": "unsupported method(s): %s" % ", ".join(str(u) for u in unknown),
            "supported_methods": _SUPPORTED,
        }))
        return 0

    try:
        model_name = _to_pascal_case(_singularize(resource)) or "Item"
        lines = []
        lines.append("from fastapi import APIRouter, HTTPException")
        lines.append("from pydantic import BaseModel")
        lines.append("from typing import Optional, List")
        lines.append("")
        lines.append("router = APIRouter()")
        lines.append("")
        lines.append("")
        lines.append("class %s(BaseModel):" % model_name)
        lines.append("    # TODO: define the real fields for %s" % model_name)
        lines.append("    id: Optional[int] = None")
        lines.append("")
        lines.append("")

        for method in normalized:
            if method == "GET":
                lines.append("@router.get(\"/%s\", response_model=List[%s])" % (resource, model_name))
                lines.append("def list_%s():" % resource)
                lines.append("    # TODO: list all %s" % resource)
                lines.append("    raise HTTPException(status_code=501, detail=\"not implemented\")")
                lines.append("")
                lines.append("")
                lines.append("@router.get(\"/%s/{item_id}\", response_model=%s)" % (resource, model_name))
                lines.append("def get_%s(item_id: int):" % model_name.lower())
                lines.append("    # TODO: get a single %s by id" % model_name.lower())
                lines.append("    raise HTTPException(status_code=501, detail=\"not implemented\")")
                lines.append("")
                lines.append("")
            elif method == "POST":
                lines.append("@router.post(\"/%s\", response_model=%s, status_code=201)" % (resource, model_name))
                lines.append("def create_%s(payload: %s):" % (model_name.lower(), model_name))
                lines.append("    # TODO: create a new %s" % model_name.lower())
                lines.append("    raise HTTPException(status_code=501, detail=\"not implemented\")")
                lines.append("")
                lines.append("")
            elif method == "PUT":
                lines.append("@router.put(\"/%s/{item_id}\", response_model=%s)" % (resource, model_name))
                lines.append("def replace_%s(item_id: int, payload: %s):" % (model_name.lower(), model_name))
                lines.append("    # TODO: replace a %s by id" % model_name.lower())
                lines.append("    raise HTTPException(status_code=501, detail=\"not implemented\")")
                lines.append("")
                lines.append("")
            elif method == "PATCH":
                lines.append("@router.patch(\"/%s/{item_id}\", response_model=%s)" % (resource, model_name))
                lines.append("def update_%s(item_id: int, payload: %s):" % (model_name.lower(), model_name))
                lines.append("    # TODO: partially update a %s by id" % model_name.lower())
                lines.append("    raise HTTPException(status_code=501, detail=\"not implemented\")")
                lines.append("")
                lines.append("")
            elif method == "DELETE":
                lines.append("@router.delete(\"/%s/{item_id}\", status_code=204)" % resource)
                lines.append("def delete_%s(item_id: int):" % model_name.lower())
                lines.append("    # TODO: delete a %s by id" % model_name.lower())
                lines.append("    raise HTTPException(status_code=501, detail=\"not implemented\")")
                lines.append("")
                lines.append("")

        code = "\n".join(lines).rstrip() + "\n"
        filename = "%s_router.py" % resource
        print(json.dumps({"filename": filename, "code": code}, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "fastapi_endpoint_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
