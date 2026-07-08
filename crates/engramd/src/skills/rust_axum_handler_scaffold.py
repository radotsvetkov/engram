#!/usr/bin/env python3
"""rust_axum_handler_scaffold — Engram skill (no network). Generate a Rust
axum (0.7-style) handler module boilerplate for a resource: one async
handler function per requested HTTP method plus a Router builder wiring
them to the resource path.

Request (stdin): {"resource": "widgets", "methods": ["GET", "POST"]}
Output (stdout): {filename, code}
"""
import json
import re
import sys

_SUPPORTED = ["GET", "POST", "PUT", "PATCH", "DELETE"]

# method -> (axum routing fn name, handler fn name suffix, description)
_VERB_MAP = {
    "GET": ("get", "list", "list all {resource}"),
    "POST": ("post", "create", "create a new {resource}"),
    "PUT": ("put", "update", "replace a {resource}"),
    "PATCH": ("patch", "update", "partially update a {resource}"),
    "DELETE": ("delete", "delete", "delete a {resource}"),
}


def _singularize(word):
    if word.endswith("ies") and len(word) > 3:
        return word[:-3] + "y"
    if word.endswith("ses") and len(word) > 3:
        return word[:-2]
    if word.endswith("s") and not word.endswith("ss"):
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
            "example": {"resource": "widgets", "methods": ["GET", "POST"]},
        }))
        return 0

    raw_resource = q.get("resource")
    if not isinstance(raw_resource, str) or not raw_resource.strip():
        print(json.dumps({
            "error": "missing required field 'resource' (non-empty string)",
            "example": {"resource": "widgets", "methods": ["GET", "POST"]},
        }))
        return 0
    resource = raw_resource.strip().strip("/")
    if not re.match(r"^[A-Za-z0-9_-]+$", resource):
        print(json.dumps({"error": "'resource' must be a simple path segment (letters, numbers, -, _), got %r" % resource}))
        return 0

    methods = q.get("methods")
    if methods is None:
        methods = ["GET", "POST"]
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
        slug = re.sub(r"[^A-Za-z0-9]+", "_", resource).strip("_").lower()
        singular = _singularize(slug)

        # Build handler fn names, avoiding duplicate fn names when PUT and
        # PATCH both map to "update_<resource>" — keep the first, reuse for
        # routing but only emit the function body once.
        handler_fns = {}  # fn_name -> (verb_fn, [methods using it], description)
        order = []
        for method in normalized:
            verb_fn, name_suffix, desc_tpl = _VERB_MAP[method]
            noun = slug if name_suffix == "list" else singular
            fn_name = "%s_%s" % (name_suffix, noun)
            if fn_name not in handler_fns:
                handler_fns[fn_name] = []
                order.append(fn_name)
            handler_fns[fn_name].append((method, verb_fn, desc_tpl.format(resource=slug)))

        routing_fns_used = sorted({_VERB_MAP[m][0] for m in normalized})
        any_item_route = any(_VERB_MAP[m][0] in ("put", "patch", "delete") for m in normalized)

        lines = []
        lines.append("use axum::{")
        if any_item_route:
            lines.append("    extract::Path,")
        lines.append("    http::StatusCode,")
        lines.append("    response::IntoResponse,")
        lines.append("    routing::{%s}," % ", ".join(routing_fns_used))
        lines.append("    Router,")
        lines.append("};")
        lines.append("")

        for fn_name in order:
            entries = handler_fns[fn_name]
            method, verb_fn, desc = entries[0]
            needs_path = verb_fn in ("put", "patch", "delete")
            lines.append("// %s" % desc)
            if needs_path:
                lines.append("async fn %s(Path(id): Path<String>) -> impl IntoResponse {" % fn_name)
                lines.append("    // TODO: %s (using `id`)" % desc)
                lines.append("    let _ = id;")
            else:
                lines.append("async fn %s() -> impl IntoResponse {" % fn_name)
                lines.append("    // TODO: %s" % desc)
            lines.append("    StatusCode::OK")
            lines.append("}")
            lines.append("")

        lines.append("pub fn router() -> Router {")
        lines.append("    Router::new()")

        collection_parts = []
        item_parts = []
        for method in normalized:
            verb_fn, name_suffix, _desc = _VERB_MAP[method]
            noun = slug if name_suffix == "list" else singular
            fn_name = "%s_%s" % (name_suffix, noun)
            if verb_fn in ("put", "patch", "delete"):
                item_parts.append("%s(%s)" % (verb_fn, fn_name))
            else:
                collection_parts.append("%s(%s)" % (verb_fn, fn_name))

        if collection_parts:
            lines.append("        .route(\"/%s\", %s)" % (slug, ".".join(collection_parts)))
        if item_parts:
            lines.append("        .route(\"/%s/:id\", %s)" % (slug, ".".join(item_parts)))
        lines.append("}")
        lines.append("")

        code = "\n".join(lines)
        filename = "%s.rs" % slug
        print(json.dumps({"filename": filename, "code": code}, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "rust_axum_handler_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
