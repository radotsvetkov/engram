#!/usr/bin/env python3
"""ionic_component_scaffold — Engram skill (no network). Generate an Ionic
Angular standalone page/component trio from a name.

Emits `{name}.page.ts` (@Component with standalone Ionic imports),
`{name}.page.html` (ion-header/ion-toolbar/ion-title/ion-content/ion-button),
and a `{name}.page.scss` stub. kebab-case selector `app-{name}`, PascalCase
class `{Name}Page`. Ionic is the leading hybrid framework. Stdlib only.

Request (stdin): {"name": "settings", "page": true}
Output (stdout): {files: {filename: code}, next_steps}
"""
import json
import re
import sys


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


def _to_kebab_case(name):
    return "-".join(w.lower() for w in _split_words(name))


def _title(name):
    return " ".join(w[:1].upper() + w[1:] for w in _split_words(name))


def _ts_file(kebab, cls, title, kind):
    # kind is "page" or "component"
    selector = "app-%s" % kebab
    L = [
        "import { Component } from '@angular/core';",
        "import {",
        "  IonButton,",
        "  IonContent,",
        "  IonHeader,",
        "  IonTitle,",
        "  IonToolbar,",
        "} from '@ionic/angular/standalone';",
        "",
        "@Component({",
        "  selector: '%s'," % selector,
        "  templateUrl: './%s.%s.html'," % (kebab, kind),
        "  styleUrls: ['./%s.%s.scss']," % (kebab, kind),
        "  standalone: true,",
        "  imports: [IonHeader, IonToolbar, IonTitle, IonContent, IonButton],",
        "})",
        "export class %s {" % cls,
        "  title = '%s';" % title,
        "",
        "  constructor() {}",
        "",
        "  onButtonClick(): void {",
        "    // TODO: handle button tap",
        "  }",
        "}",
        "",
    ]
    return "\n".join(L)


def _html_file(title):
    L = [
        "<ion-header>",
        "  <ion-toolbar>",
        "    <ion-title>%s</ion-title>" % title,
        "  </ion-toolbar>",
        "</ion-header>",
        "",
        '<ion-content class="ion-padding">',
        "  <p>{{ title }}</p>",
        '  <ion-button expand="block" (click)="onButtonClick()">',
        "    Continue",
        "  </ion-button>",
        "</ion-content>",
        "",
    ]
    return "\n".join(L)


def _scss_file(kebab):
    L = [
        "// Styles for %s" % kebab,
        "ion-content {",
        "  --padding-top: 16px;",
        "}",
        "",
    ]
    return "\n".join(L)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"name": "settings", "page": True},
        }))
        return 0

    raw_name = q.get("name")
    if not isinstance(raw_name, str) or not raw_name.strip():
        print(json.dumps({
            "error": "missing required field 'name' (non-empty string)",
            "example": {"name": "settings", "page": True},
        }))
        return 0

    is_page = bool(q.get("page", True))

    try:
        kebab = _to_kebab_case(raw_name)
        pascal = _to_pascal_case(raw_name)
        if not kebab or not pascal:
            print(json.dumps({"error": "could not derive a valid name from %r" % raw_name}))
            return 0

        kind = "page" if is_page else "component"
        suffix = "Page" if is_page else "Component"
        cls = pascal if pascal.endswith(suffix) else pascal + suffix
        title = _title(raw_name)

        files = {
            "%s.%s.ts" % (kebab, kind): _ts_file(kebab, cls, title, kind),
            "%s.%s.html" % (kebab, kind): _html_file(title),
            "%s.%s.scss" % (kebab, kind): _scss_file(kebab),
        }

        next_steps = [
            "Place the three files in src/app/%s/ (Ionic standalone layout)." % kebab,
        ]
        if is_page:
            next_steps.append(
                "Add a route: { path: '%s', loadComponent: () => "
                "import('./%s/%s.page').then((m) => m.%s) }." % (kebab, kebab, kebab, cls))
        result = {"files": files, "next_steps": next_steps}
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "ionic_component_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
