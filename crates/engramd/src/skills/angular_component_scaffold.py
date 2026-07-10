#!/usr/bin/env python3
"""angular_component_scaffold — Engram skill (no network). Generate an Angular
component from a name and optional @Input() names.

Produces the .component.ts (with an @Component decorator, a kebab-case
`app-{name}` selector, the standalone flag, external templateUrl/styleUrl, and
one @Input() per input) plus stub .component.html and .component.css files.
The class name is PascalCase; the selector and file basenames are kebab-case.

Request (stdin): {"name": "userCard", "standalone": true, "inputs": ["userId", "title"]}
Output (stdout): {files: {filename: content}, note}
"""
import json
import re
import sys

_EXAMPLE = {"name": "userCard", "standalone": True, "inputs": ["userId", "title"]}


def _split_words(name):
    parts = re.split(r"[^A-Za-z0-9]+", name.strip())
    words = []
    for part in parts:
        if not part:
            continue
        sub = re.findall(r"[A-Z]+(?=[A-Z][a-z0-9])|[A-Z]?[a-z0-9]+|[A-Z]+", part)
        words.extend(sub if sub else [part])
    return [w for w in words if w]


def _to_pascal_case(name):
    return "".join(w[:1].upper() + w[1:] for w in _split_words(name))


def _to_kebab(name):
    return "-".join(w.lower() for w in _split_words(name))


def _valid_inputs(inputs):
    ident_re = re.compile(r"^[A-Za-z_$][A-Za-z0-9_$]*$")
    out = []
    for p in inputs:
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
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE}))
        return 0

    raw_name = q.get("name")
    if not isinstance(raw_name, str) or not raw_name.strip():
        print(json.dumps({
            "error": "missing required field 'name' (non-empty string)",
            "example": _EXAMPLE,
        }))
        return 0

    standalone = bool(q.get("standalone", True))
    raw_inputs = q.get("inputs") or []
    if not isinstance(raw_inputs, list):
        print(json.dumps({"error": "'inputs' must be a list of strings if provided", "example": _EXAMPLE}))
        return 0

    try:
        words = _split_words(raw_name)
        if not words:
            print(json.dumps({"error": "could not derive a valid component name from %r" % raw_name}))
            return 0
        class_name = _to_pascal_case(raw_name) + "Component"
        kebab = _to_kebab(raw_name)
        selector = "app-%s" % kebab
        base = "%s.component" % kebab
        ts_file = "%s.ts" % base
        html_file = "%s.html" % base
        css_file = "%s.css" % base
        inputs = _valid_inputs(raw_inputs)

        ts = []
        ts.append("import { Component, Input } from '@angular/core';")
        ts.append("")
        ts.append("@Component({")
        ts.append("  selector: '%s'," % selector)
        if standalone:
            ts.append("  standalone: true,")
            ts.append("  imports: [],")
        ts.append("  templateUrl: './%s'," % html_file)
        ts.append("  styleUrl: './%s'," % css_file)
        ts.append("})")
        ts.append("export class %s {" % class_name)
        if inputs:
            for inp in inputs:
                ts.append("  @Input() %s!: string;" % inp)
        else:
            ts.append("  // TODO: add @Input() / @Output() members")
        ts.append("}")
        ts.append("")

        html = ["<div class=\"%s\">" % kebab, "  <h1>%s</h1>" % class_name]
        if inputs:
            for inp in inputs:
                html.append("  <p>{{ %s }}</p>" % inp)
        else:
            html.append("  <!-- TODO: implement %s -->" % kebab)
        html.append("</div>")
        html.append("")

        css = [".%s {" % kebab, "  /* TODO: style %s */" % kebab, "}", ""]

        files = {
            ts_file: "\n".join(ts),
            html_file: "\n".join(html),
            css_file: "\n".join(css),
        }
        result = {
            "files": files,
            "note": "%s is the component class; template is %s and styles are %s." % (
                class_name, html_file, css_file),
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "angular_component_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
