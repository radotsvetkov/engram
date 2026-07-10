#!/usr/bin/env python3
"""flutter_widget_scaffold — Engram skill (no network). Generate idiomatic
Flutter/Dart boilerplate: a StatelessWidget, a StatefulWidget with setState,
or a screen (StatelessWidget wrapped in Scaffold + AppBar) — const
constructors with super.key and a snake_case filename.

Request (stdin): {"name": "user profile card", "kind": "stateless|stateful|screen"}
Output (stdout): {filename, code, notes: [...]}
"""
import json
import re
import sys

_KINDS = ("stateless", "stateful", "screen")
_EXAMPLE = {"name": "user profile card", "kind": "stateless"}


def _words(name):
    parts = re.split(r"[^A-Za-z0-9]+", str(name).strip())
    words = []
    for part in parts:
        if not part:
            continue
        sub = re.findall(r"[A-Z]+(?=[A-Z][a-z0-9])|[A-Z]?[a-z0-9]+|[A-Z]+", part)
        words.extend(sub if sub else [part])
    return [w for w in words if w]


def _pascal_case(name):
    return "".join(w[:1].upper() + w[1:] for w in _words(name))


def _camel_case(name):
    p = _pascal_case(name)
    return p[:1].lower() + p[1:]


def _snake_case(name):
    return "_".join(w.lower() for w in _words(name))


def _kebab_case(name):
    return "-".join(w.lower() for w in _words(name))


def _title(name):
    return " ".join(w[:1].upper() + w[1:] for w in _words(name))


_STATELESS_TMPL = """import 'package:flutter/material.dart';

class %(cls)s extends StatelessWidget {
  const %(cls)s({super.key});

  @override
  Widget build(BuildContext context) {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: const [
        Text(
          '%(title)s',
          style: TextStyle(fontSize: 20, fontWeight: FontWeight.w600),
        ),
        SizedBox(height: 8),
        Text('TODO: build this widget'),
      ],
    );
  }
}
"""

_STATEFUL_TMPL = """import 'package:flutter/material.dart';

class %(cls)s extends StatefulWidget {
  const %(cls)s({super.key});

  @override
  State<%(cls)s> createState() => _%(cls)sState();
}

class _%(cls)sState extends State<%(cls)s> {
  int _counter = 0;

  void _increment() {
    setState(() => _counter++);
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Text(
          '%(title)s: $_counter',
          style: const TextStyle(fontSize: 20, fontWeight: FontWeight.w600),
        ),
        const SizedBox(height: 8),
        ElevatedButton(
          onPressed: _increment,
          child: const Text('Increment'),
        ),
      ],
    );
  }
}
"""

_SCREEN_TMPL = """import 'package:flutter/material.dart';

class %(cls)s extends StatelessWidget {
  const %(cls)s({super.key});

  static const routeName = '/%(route)s';

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('%(title)s'),
      ),
      body: const Center(
        child: Text('TODO: build %(title)s'),
      ),
    );
  }
}
"""


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

    kind = q.get("kind", "stateless")
    if kind not in _KINDS:
        print(json.dumps({
            "error": "'kind' must be one of %s" % ", ".join(_KINDS),
            "example": _EXAMPLE,
        }))
        return 0

    try:
        cls = _pascal_case(raw_name)
        if not cls:
            print(json.dumps({"error": "could not derive a class name from %r" % raw_name}))
            return 0

        if kind == "screen":
            if not cls.endswith("Screen"):
                cls += "Screen"
            base = cls[:-6]
            code = _SCREEN_TMPL % {
                "cls": cls,
                "title": _title(base),
                "route": _kebab_case(base),
            }
            folder = "lib/screens"
        else:
            tmpl = _STATELESS_TMPL if kind == "stateless" else _STATEFUL_TMPL
            code = tmpl % {"cls": cls, "title": _title(cls)}
            folder = "lib/widgets"

        filename = "%s.dart" % _snake_case(cls)
        notes = [
            "Save as %s/%s and import it with "
            "package:<your_app>/%s/%s." % (folder, filename, folder.split("/")[-1], filename),
            "Uses only flutter/material — no pubspec.yaml changes needed.",
        ]
        if kind == "screen":
            notes.append(
                "Register the route in MaterialApp, e.g. routes: "
                "{%s.routeName: (context) => const %s()}, then open it with "
                "Navigator.pushNamed(context, %s.routeName) — or add an equivalent "
                "GoRoute if you use go_router." % (cls, cls, cls)
            )

        result = {"filename": filename, "code": code, "notes": notes}
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "flutter_widget_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
