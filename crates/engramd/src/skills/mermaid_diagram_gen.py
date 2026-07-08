#!/usr/bin/env python3
"""mermaid_diagram_gen — Engram skill (no network).

Generates real, syntactically-valid Mermaid.js diagram source text for one of
five diagram types (flowchart, sequence, entity-relationship, class, and an
"approximated" C4 system-context diagram, since Mermaid has no native C4
support without a plugin — we fake it honestly with a styled flowchart and say
so in the output). Mermaid renders natively in GitHub, GitLab, Notion, and
many docs tools, so the output text is directly pasteable and useful.

Request (stdin): {"diagram_type": "flowchart"|"sequence"|"er"|"class"|"c4_context",
  "nodes"?: [...], "edges"?: [...], "title"?: str}
  - flowchart nodes: [{id, label, shape?: "rect"|"diamond"|"round"}]
    edges: [{from, to, label?}]
  - sequence nodes (participants): [{id, label?}]
    edges (messages, in order): [{from, to, label, type?: "sync"|"async"}]
  - er nodes (entities): [{id, attributes?: [str]}]
    edges: [{from, to, relationship?: "one-to-many"|"one-to-one"|"many-to-many", label?}]
  - class nodes: [{id, attributes?: [str], methods?: [str]}]
    edges: [{from, to, relationship?: "inheritance"|"composition"|"association"}]
  - c4_context nodes: [{id, label, type?: "person"|"system"|"external_system"}]
    edges: [{from, to, label?}]
Output (stdout): {mermaid_code: str, diagram_type: str}
"""
import json
import sys

DIAGRAM_TYPES = ("flowchart", "sequence", "er", "class", "c4_context")


def _sanitize_id(raw):
    """Mermaid node/participant ids should be simple tokens."""
    s = str(raw).strip()
    return s if s else "n"


def _gen_flowchart(nodes, edges, title):
    lines = []
    if title:
        lines.append("%%{init: {'theme': 'default'}}%%")
    lines.append("flowchart TD")
    for n in nodes:
        nid = _sanitize_id(n.get("id"))
        label = str(n.get("label", nid))
        shape = n.get("shape") or "rect"
        if shape == "diamond":
            lines.append('    %s{"%s"}' % (nid, label))
        elif shape == "round":
            lines.append('    %s("%s")' % (nid, label))
        else:
            lines.append('    %s["%s"]' % (nid, label))
    for e in edges:
        frm = _sanitize_id(e.get("from"))
        to = _sanitize_id(e.get("to"))
        label = e.get("label")
        if label:
            lines.append("    %s -->|%s| %s" % (frm, label, to))
        else:
            lines.append("    %s --> %s" % (frm, to))
    return "\n".join(lines)


def _gen_sequence(nodes, edges, title):
    lines = ["sequenceDiagram"]
    if title:
        lines.append("    title %s" % title)
    for n in nodes:
        nid = _sanitize_id(n.get("id"))
        label = n.get("label")
        if label:
            lines.append("    participant %s as %s" % (nid, label))
        else:
            lines.append("    participant %s" % nid)
    for e in edges:
        frm = _sanitize_id(e.get("from"))
        to = _sanitize_id(e.get("to"))
        label = str(e.get("label", ""))
        msg_type = e.get("type") or "sync"
        if msg_type == "async":
            lines.append("    %s-->>-%s: %s" % (frm, to, label))
        else:
            lines.append("    %s->>+%s: %s" % (frm, to, label))
    return "\n".join(lines)


_ER_CARDINALITY = {
    "one-to-many": "||--o{",
    "one-to-one": "||--||",
    "many-to-many": "}o--o{",
}


def _gen_er(nodes, edges, title):
    lines = ["erDiagram"]
    for n in nodes:
        nid = _sanitize_id(n.get("id"))
        attrs = n.get("attributes") or []
        if attrs:
            lines.append("    %s {" % nid)
            for a in attrs:
                lines.append("        string %s" % str(a).replace(" ", "_"))
            lines.append("    }")
        else:
            lines.append("    %s {" % nid)
            lines.append("    }")
    for e in edges:
        frm = _sanitize_id(e.get("from"))
        to = _sanitize_id(e.get("to"))
        rel = e.get("relationship") or "one-to-many"
        notation = _ER_CARDINALITY.get(rel, _ER_CARDINALITY["one-to-many"])
        label = e.get("label")
        if label:
            lines.append('    %s %s %s : "%s"' % (frm, notation, to, label))
        else:
            lines.append('    %s %s %s : "relates_to"' % (frm, notation, to))
    return "\n".join(lines)


_CLASS_ARROWS = {
    "inheritance": "<|--",
    "composition": "*--",
    "association": "--",
}


def _gen_class(nodes, edges, title):
    lines = ["classDiagram"]
    for n in nodes:
        nid = _sanitize_id(n.get("id"))
        attrs = n.get("attributes") or []
        methods = n.get("methods") or []
        lines.append("    class %s {" % nid)
        for a in attrs:
            lines.append("        +%s" % a)
        for m in methods:
            m = str(m)
            if not m.endswith(")"):
                m = m + "()"
            lines.append("        +%s" % m)
        lines.append("    }")
    for e in edges:
        frm = _sanitize_id(e.get("from"))
        to = _sanitize_id(e.get("to"))
        rel = e.get("relationship") or "association"
        arrow = _CLASS_ARROWS.get(rel, _CLASS_ARROWS["association"])
        # Inheritance in Mermaid points from child to parent as `Child --|> Parent`
        # but the commonly used literal form is `Parent <|-- Child`; we use the
        # from/to order as given with the resolved arrow token.
        lines.append("    %s %s %s" % (frm, arrow, to))
    return "\n".join(lines)


_C4_SHAPES = {
    "person": lambda nid, label: '%s(["%s"])' % (nid, label),
    "system": lambda nid, label: '%s["%s"]' % (nid, label),
    "external_system": lambda nid, label: '%s[["%s"]]' % (nid, label),
}


def _gen_c4_context(nodes, edges, title):
    lines = [
        "%% NOTE: Mermaid has no native C4 diagram support without a plugin.",
        "%% This is an honest approximation using a styled flowchart:",
        "%%   stadium shape = person, rect = system, subroutine shape = external system.",
        "flowchart TD",
    ]
    if title:
        lines.insert(3, "    %%%% %s" % title)
    for n in nodes:
        nid = _sanitize_id(n.get("id"))
        label = str(n.get("label", nid))
        ntype = n.get("type") or "system"
        shape_fn = _C4_SHAPES.get(ntype, _C4_SHAPES["system"])
        lines.append("    " + shape_fn(nid, label))
    for e in edges:
        frm = _sanitize_id(e.get("from"))
        to = _sanitize_id(e.get("to"))
        label = e.get("label")
        if label:
            lines.append("    %s -->|%s| %s" % (frm, label, to))
        else:
            lines.append("    %s --> %s" % (frm, to))
    return "\n".join(lines)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"diagram_type": "flowchart",
                        "nodes": [{"id": "a", "label": "Start"}],
                        "edges": []},
        }))
        return 0

    diagram_type = q.get("diagram_type")
    if diagram_type not in DIAGRAM_TYPES:
        print(json.dumps({
            "error": "'diagram_type' must be one of: %s" % ", ".join(DIAGRAM_TYPES),
            "example": {"diagram_type": "flowchart",
                        "nodes": [{"id": "a", "label": "Start", "shape": "round"},
                                  {"id": "b", "label": "Done"}],
                        "edges": [{"from": "a", "to": "b", "label": "next"}]},
        }))
        return 0

    nodes = q.get("nodes") or []
    edges = q.get("edges") or []
    title = q.get("title")

    if not isinstance(nodes, list) or not isinstance(edges, list):
        print(json.dumps({"error": "'nodes' and 'edges' must be lists"}))
        return 0

    try:
        for n in nodes:
            if not isinstance(n, dict) or not n.get("id"):
                raise ValueError("each node needs an 'id'")
        for e in edges:
            if not isinstance(e, dict) or not e.get("from") or not e.get("to"):
                raise ValueError("each edge needs 'from' and 'to'")

        if diagram_type == "flowchart":
            code = _gen_flowchart(nodes, edges, title)
        elif diagram_type == "sequence":
            code = _gen_sequence(nodes, edges, title)
        elif diagram_type == "er":
            code = _gen_er(nodes, edges, title)
        elif diagram_type == "class":
            code = _gen_class(nodes, edges, title)
        else:
            code = _gen_c4_context(nodes, edges, title)
    except Exception as e:
        print(json.dumps({"error": "could not build diagram: %s" % e}))
        return 0

    print(json.dumps({"mermaid_code": code, "diagram_type": diagram_type}, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
