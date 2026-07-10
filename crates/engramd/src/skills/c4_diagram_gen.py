#!/usr/bin/env python3
"""c4_diagram_gen — Engram skill (no network). Generate C4 Mermaid diagrams.

Emits a Mermaid `graph TD` for a C4 model level. "context" shows the system as
one box surrounded by actors and external systems. "container" opens the system
into a boundary subgraph of its containers (apps, services, datastores) with the
relationships between them. Paste the output into any Mermaid renderer. Stdlib
only.

Request (stdin): {"level": "context", "system": "Banking App",
                  "actors": ["Customer"], "external_systems": ["Email"]}
Output (stdout): {level, system, mermaid}
"""
import json
import re
import sys


def _node_id(prefix, name, seen):
    slug = re.sub(r"[^A-Za-z0-9]+", "_", str(name)).strip("_") or "n"
    base = "%s_%s" % (prefix, slug)
    nid = base
    i = 2
    while nid in seen:
        nid = "%s_%d" % (base, i)
        i += 1
    seen.add(nid)
    return nid


def _esc(text):
    # Mermaid labels: keep quotes safe by using single quotes inside "..."
    return str(text).replace('"', "'").replace("\n", " ")


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"level": "context", "system": "Banking App",
                        "actors": ["Customer"]},
        }))
        return 0

    level = str(q.get("level") or "").strip().lower()
    system = q.get("system")
    if level not in ("context", "container"):
        print(json.dumps({
            "error": "'level' must be 'context' or 'container'",
            "example": {"level": "context", "system": "Banking App"},
        }))
        return 0
    if not system or not str(system).strip():
        print(json.dumps({
            "error": "missing required field: system",
            "example": {"level": "context", "system": "Banking App"},
        }))
        return 0
    system = str(system).strip()

    try:
        actors = q.get("actors") or []
        external = q.get("external_systems") or []
        containers = q.get("containers") or []
        relationships = q.get("relationships") or []
        if not isinstance(actors, list) or not isinstance(external, list):
            raise ValueError("'actors' and 'external_systems' must be lists")

        seen = set()
        lines = ["graph TD"]
        name_to_id = {}

        if level == "context":
            sys_id = _node_id("sys", system, seen)
            name_to_id[system] = sys_id
            lines.append('    %s["%s<br/><i>[Software System]</i>"]'
                         % (sys_id, _esc(system)))

            actor_ids = {}
            for a in actors:
                aid = _node_id("actor", a, seen)
                actor_ids[str(a)] = aid
                name_to_id[str(a)] = aid
                lines.append('    %s(["%s<br/><i>[Person]</i>"])'
                             % (aid, _esc(a)))
            ext_ids = {}
            for e in external:
                eid = _node_id("ext", e, seen)
                ext_ids[str(e)] = eid
                name_to_id[str(e)] = eid
                lines.append('    %s["%s<br/><i>[External System]</i>"]'
                             % (eid, _esc(e)))

            rel_lines = []
            if relationships:
                for r in relationships:
                    if not isinstance(r, dict):
                        continue
                    frm = name_to_id.get(str(r.get("from")))
                    to = name_to_id.get(str(r.get("to")))
                    if not frm or not to:
                        continue
                    label = r.get("label")
                    if label:
                        rel_lines.append('    %s -->|"%s"| %s'
                                         % (frm, _esc(label), to))
                    else:
                        rel_lines.append("    %s --> %s" % (frm, to))
            if not rel_lines:
                # default: actors use the system, system uses externals
                for a in actors:
                    rel_lines.append('    %s -->|"Uses"| %s'
                                     % (actor_ids[str(a)], sys_id))
                for e in external:
                    rel_lines.append('    %s -->|"Calls"| %s'
                                     % (sys_id, ext_ids[str(e)]))
            lines.extend(rel_lines)

        else:  # container
            if not isinstance(containers, list) or len(containers) == 0:
                print(json.dumps({
                    "error": "container level requires a non-empty 'containers' "
                             "list of {name, tech, description}",
                    "example": {"level": "container", "system": "Banking App",
                                "containers": [{"name": "API", "tech": "Go",
                                                "description": "REST backend"}]},
                }))
                return 0

            boundary_id = _node_id("boundary", system, seen)
            lines.append('    subgraph %s["%s"]' % (boundary_id, _esc(system)))
            for c in containers:
                if not isinstance(c, dict):
                    raise ValueError("each container must be an object")
                cname = str(c.get("name") or "Container")
                cid = _node_id("c", cname, seen)
                name_to_id[cname] = cid
                tech = c.get("tech")
                desc = c.get("description")
                label = _esc(cname)
                if tech:
                    label += "<br/><i>[%s]</i>" % _esc(tech)
                if desc:
                    label += "<br/>%s" % _esc(desc)
                lines.append('        %s["%s"]' % (cid, label))
            lines.append("    end")

            # external actors/systems live outside the boundary
            actor_ids = {}
            for a in actors:
                aid = _node_id("actor", a, seen)
                actor_ids[str(a)] = aid
                name_to_id[str(a)] = aid
                lines.append('    %s(["%s<br/><i>[Person]</i>"])'
                             % (aid, _esc(a)))
            ext_ids = {}
            for e in external:
                eid = _node_id("ext", e, seen)
                ext_ids[str(e)] = eid
                name_to_id[str(e)] = eid
                lines.append('    %s["%s<br/><i>[External System]</i>"]'
                             % (eid, _esc(e)))

            rel_lines = []
            if relationships:
                for r in relationships:
                    if not isinstance(r, dict):
                        continue
                    frm = name_to_id.get(str(r.get("from")))
                    to = name_to_id.get(str(r.get("to")))
                    if not frm or not to:
                        continue
                    label = r.get("label")
                    if label:
                        rel_lines.append('    %s -->|"%s"| %s'
                                         % (frm, _esc(label), to))
                    else:
                        rel_lines.append("    %s --> %s" % (frm, to))
            if not rel_lines and containers:
                # default: actors talk to the first container
                first = name_to_id[str(containers[0].get("name") or "Container")]
                for a in actors:
                    rel_lines.append('    %s -->|"Uses"| %s'
                                     % (actor_ids[str(a)], first))
            lines.extend(rel_lines)

        mermaid = "\n".join(lines)
        result = {"level": level, "system": system, "mermaid": mermaid}
        print(json.dumps(result, indent=2, default=str))
        return 0
    except (TypeError, ValueError) as e:
        print(json.dumps({"error": "invalid input: %s" % e}))
        return 0
    except Exception as e:
        print(json.dumps({"error": "c4_diagram_gen failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
