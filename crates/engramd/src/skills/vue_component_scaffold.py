#!/usr/bin/env python3
"""vue_component_scaffold — Engram skill (no network). Generate a Vue 3
Single-File Component boilerplate using `<script setup>` syntax, with a
`defineProps` call for the given prop names and a minimal template that
interpolates each prop.

Request (stdin): {"name": "userCard", "props": ["userId", "onClose"], "typescript": true}
Output (stdout): {filename, code}
"""
import json
import re
import sys


def _to_pascal_case(name):
    parts = re.split(r"[^A-Za-z0-9]+", name.strip())
    words = []
    for part in parts:
        if not part:
            continue
        sub = re.findall(r"[A-Z]+(?=[A-Z][a-z0-9])|[A-Z]?[a-z0-9]+|[A-Z]+", part)
        words.extend(sub if sub else [part])
    if not words:
        return ""
    return "".join(w[:1].upper() + w[1:] for w in words if w)


def _valid_props(props):
    ident_re = re.compile(r"^[A-Za-z_$][A-Za-z0-9_$]*$")
    out = []
    for p in props:
        if isinstance(p, str) and ident_re.match(p.strip()):
            out.append(p.strip())
    return out


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"name": "UserCard", "props": ["userId", "onClose"], "typescript": True},
        }))
        return 0

    raw_name = q.get("name")
    if not isinstance(raw_name, str) or not raw_name.strip():
        print(json.dumps({
            "error": "missing required field 'name' (non-empty string)",
            "example": {"name": "UserCard", "props": ["userId", "onClose"], "typescript": True},
        }))
        return 0

    typescript = bool(q.get("typescript", False))
    raw_props = q.get("props") or []
    if not isinstance(raw_props, list):
        print(json.dumps({
            "error": "'props' must be a list of strings if provided",
            "example": {"name": "UserCard", "props": ["userId", "onClose"]},
        }))
        return 0

    try:
        component_name = _to_pascal_case(raw_name)
        if not component_name:
            print(json.dumps({"error": "could not derive a valid component name from %r" % raw_name}))
            return 0

        props = _valid_props(raw_props)
        filename = "%s.vue" % component_name

        lines = ["<script setup%s>" % (" lang=\"ts\"" if typescript else "")]
        if props:
            if typescript:
                lines.append("interface Props {")
                for p in props:
                    lines.append("  %s: string" % p)
                lines.append("}")
                lines.append("")
                lines.append("defineProps<Props>()")
            else:
                lines.append("defineProps({")
                for i, p in enumerate(props):
                    comma = "," if i < len(props) - 1 else ""
                    lines.append("  %s: {" % p)
                    lines.append("    type: String,")
                    lines.append("    required: true,")
                    lines.append("  }%s" % comma)
                lines.append("})")
        else:
            lines.append("// no props defined")
        lines.append("</script>")
        lines.append("")
        lines.append("<template>")
        lines.append("  <div class=\"%s\">" % re.sub(r"(?<!^)(?=[A-Z])", "-", component_name).lower())
        lines.append("    <h1>%s</h1>" % component_name)
        if props:
            for p in props:
                lines.append("    <p>{{ %s }}</p>" % p)
        else:
            lines.append("    <!-- TODO: implement %s -->" % component_name)
        lines.append("  </div>")
        lines.append("</template>")
        lines.append("")
        lines.append("<style scoped>")
        lines.append(".%s {" % re.sub(r"(?<!^)(?=[A-Z])", "-", component_name).lower())
        lines.append("  /* TODO: style %s */" % component_name)
        lines.append("}")
        lines.append("</style>")
        lines.append("")

        code = "\n".join(lines)
        print(json.dumps({"filename": filename, "code": code}, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "vue_component_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
