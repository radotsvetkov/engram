#!/usr/bin/env python3
"""jsonschema_validate — Engram skill (no network). Validate data against a JSON Schema subset.

Implements a pragmatic, DOCUMENTED SUBSET of JSON Schema draft-07 in pure Python
(no jsonschema lib): type (incl. arrays of types), required, properties,
additionalProperties (bool), items, enum, const, minimum/maximum,
exclusiveMinimum/exclusiveMaximum, minLength/maxLength, pattern, minItems/
maxItems, uniqueItems, with recursion into nested objects/arrays. It collects
ALL errors, not just the first. Unsupported keywords are ignored.

Request (stdin): {"data": <any>, "schema": {...}}
Output (stdout): {valid, errors: [{path, message}]}
"""
import json, sys, re


def _type_ok(value, t):
    if t == "string":
        return isinstance(value, str)
    if t == "integer":
        return isinstance(value, int) and not isinstance(value, bool)
    if t == "number":
        return isinstance(value, (int, float)) and not isinstance(value, bool)
    if t == "boolean":
        return isinstance(value, bool)
    if t == "array":
        return isinstance(value, list)
    if t == "object":
        return isinstance(value, dict)
    if t == "null":
        return value is None
    return True  # unknown type keyword: don't fail


def _canonical(v):
    return json.dumps(v, sort_keys=True, separators=(",", ":"), default=str)


def _validate(value, schema, path, errors):
    if not isinstance(schema, dict):
        return

    # type
    if "type" in schema:
        types = schema["type"]
        types = types if isinstance(types, list) else [types]
        if not any(_type_ok(value, t) for t in types):
            errors.append({"path": path, "message": "expected type %s, got %s" % (
                "/".join(str(t) for t in types), type(value).__name__)})

    # enum
    if "enum" in schema:
        allowed = schema["enum"]
        if isinstance(allowed, list):
            cv = _canonical(value)
            if not any(cv == _canonical(a) for a in allowed):
                errors.append({"path": path, "message": "value not in enum %s" % json.dumps(allowed, default=str)})

    # const
    if "const" in schema:
        if _canonical(value) != _canonical(schema["const"]):
            errors.append({"path": path, "message": "value must equal const %s" % json.dumps(schema["const"], default=str)})

    # numeric constraints (skip booleans, which are not real numbers here)
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        if "minimum" in schema and value < schema["minimum"]:
            errors.append({"path": path, "message": "%s < minimum %s" % (value, schema["minimum"])})
        if "maximum" in schema and value > schema["maximum"]:
            errors.append({"path": path, "message": "%s > maximum %s" % (value, schema["maximum"])})
        if "exclusiveMinimum" in schema and value <= schema["exclusiveMinimum"]:
            errors.append({"path": path, "message": "%s <= exclusiveMinimum %s" % (value, schema["exclusiveMinimum"])})
        if "exclusiveMaximum" in schema and value >= schema["exclusiveMaximum"]:
            errors.append({"path": path, "message": "%s >= exclusiveMaximum %s" % (value, schema["exclusiveMaximum"])})

    # string constraints
    if isinstance(value, str):
        if "minLength" in schema and len(value) < schema["minLength"]:
            errors.append({"path": path, "message": "length %d < minLength %d" % (len(value), schema["minLength"])})
        if "maxLength" in schema and len(value) > schema["maxLength"]:
            errors.append({"path": path, "message": "length %d > maxLength %d" % (len(value), schema["maxLength"])})
        if "pattern" in schema:
            try:
                if re.search(schema["pattern"], value) is None:
                    errors.append({"path": path, "message": "does not match pattern %s" % schema["pattern"]})
            except re.error as e:
                errors.append({"path": path, "message": "invalid pattern in schema: %s" % e})

    # array constraints
    if isinstance(value, list):
        if "minItems" in schema and len(value) < schema["minItems"]:
            errors.append({"path": path, "message": "%d items < minItems %d" % (len(value), schema["minItems"])})
        if "maxItems" in schema and len(value) > schema["maxItems"]:
            errors.append({"path": path, "message": "%d items > maxItems %d" % (len(value), schema["maxItems"])})
        if schema.get("uniqueItems") is True:
            seen = set()
            dup = False
            for item in value:
                c = _canonical(item)
                if c in seen:
                    dup = True
                    break
                seen.add(c)
            if dup:
                errors.append({"path": path, "message": "items are not unique"})
        if "items" in schema and isinstance(schema["items"], dict):
            for i, item in enumerate(value):
                _validate(item, schema["items"], "%s[%d]" % (path, i), errors)

    # object constraints
    if isinstance(value, dict):
        props = schema.get("properties")
        props = props if isinstance(props, dict) else {}
        for req in schema.get("required", []) or []:
            if req not in value:
                errors.append({"path": "%s.%s" % (path, req) if path else req, "message": "required property missing"})
        for k, v in value.items():
            child_path = "%s.%s" % (path, k) if path else k
            if k in props:
                _validate(v, props[k], child_path, errors)
            else:
                ap = schema.get("additionalProperties", True)
                if ap is False:
                    errors.append({"path": child_path, "message": "additional property not allowed"})
                elif isinstance(ap, dict):
                    _validate(v, ap, child_path, errors)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"data": {"a": 1}, "schema": {"type": "object", "required": ["a"]}},
        })); return 0

    if "schema" not in q:
        print(json.dumps({
            "error": "missing required field 'schema'",
            "example": {"data": {"name": "Al"}, "schema": {"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}},
        })); return 0

    schema = q.get("schema")
    if not isinstance(schema, dict):
        print(json.dumps({
            "error": "'schema' must be a JSON object",
            "example": {"data": 5, "schema": {"type": "integer", "minimum": 0}},
        })); return 0

    data = q.get("data")  # any, may be null/absent
    try:
        errors = []
        _validate(data, schema, "", errors)
        result = {"valid": len(errors) == 0, "errors": errors}
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "jsonschema_validate failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
