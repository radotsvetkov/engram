#!/usr/bin/env python3
"""swiftui_view_scaffold — Engram skill (no network). Generate modern SwiftUI
boilerplate: a plain View, a List over an Identifiable model, or a Form with
@State fields — each with a #Preview macro, optionally backed by an
@Observable (Swift 5.9+) view model.

Request (stdin): {"name": "profile", "kind": "view|list|form", "with_viewmodel": false}
Output (stdout): {filename, code, notes: [...]}
"""
import json
import re
import sys

_KINDS = ("view", "list", "form")
_EXAMPLE = {"name": "profile", "kind": "view", "with_viewmodel": False}


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


def _vm_lines(base, kind):
    vm = base + "ViewModel"
    L = ["@Observable", "final class %s {" % vm]
    if kind == "view":
        L += [
            "    var isLoading = false",
            '    var statusMessage = "Ready"',
            "",
            "    func load() {",
            "        isLoading = true",
            "        // TODO: replace with a real data source",
            '        statusMessage = "Loaded"',
            "        isLoading = false",
            "    }",
        ]
    elif kind == "list":
        L += [
            "    var isLoading = false",
            "    var items: [%sItem] = []" % base,
            "",
            "    func load() {",
            "        isLoading = true",
            "        // TODO: replace with a real data source",
            "        items = [",
            '            %sItem(title: "First item", detail: "Replace with real data"),' % base,
            '            %sItem(title: "Second item", detail: "Replace with real data"),' % base,
            "        ]",
            "        isLoading = false",
            "    }",
        ]
    else:  # form
        L += [
            '    var name = ""',
            "    var notificationsEnabled = true",
            "    var volume = 0.5",
            "",
            "    func save() {",
            "        // TODO: persist the form values",
            "    }",
        ]
    L += ["}", ""]
    return L


def _item_lines(base):
    return [
        "struct %sItem: Identifiable {" % base,
        "    let id = UUID()",
        "    let title: String",
        "    let detail: String",
        "}",
        "",
    ]


def _build(base, view_name, kind, with_vm):
    title = _title(base)
    vm = base + "ViewModel"
    L = ["import SwiftUI", ""]

    if kind == "list":
        L += _item_lines(base)
    if with_vm:
        L += _vm_lines(base, kind)

    L.append("struct %s: View {" % view_name)

    if with_vm:
        L.append("    @State private var viewModel = %s()" % vm)
        L.append("")
    elif kind == "list":
        L += [
            "    let items: [%sItem] = [" % base,
            '        %sItem(title: "First item", detail: "Replace with real data"),' % base,
            '        %sItem(title: "Second item", detail: "Replace with real data"),' % base,
            "    ]",
            "",
        ]
    elif kind == "form":
        L += [
            '    @State private var name = ""',
            "    @State private var notificationsEnabled = true",
            "    @State private var volume = 0.5",
            "",
        ]

    L.append("    var body: some View {")

    if kind == "view":
        L += [
            "        VStack(spacing: 12) {",
            '            Text("%s")' % title,
            "                .font(.title2)",
            "                .fontWeight(.semibold)",
        ]
        if with_vm:
            L.append("            Text(viewModel.statusMessage)")
        else:
            L.append('            Text("TODO: build this view")')
        L += [
            "                .foregroundStyle(.secondary)",
            "        }",
            "        .padding()",
        ]
        if with_vm:
            L.append("        .task { viewModel.load() }")
    elif kind == "list":
        source = "viewModel.items" if with_vm else "items"
        L += [
            "        NavigationStack {",
            "            List {",
            "                ForEach(%s) { item in" % source,
            "                    VStack(alignment: .leading, spacing: 2) {",
            "                        Text(item.title)",
            "                        Text(item.detail)",
            "                            .font(.caption)",
            "                            .foregroundStyle(.secondary)",
            "                    }",
            "                }",
            "            }",
            '            .navigationTitle("%s")' % title,
        ]
        if with_vm:
            L.append("            .task { viewModel.load() }")
        L.append("        }")
    else:  # form
        prefix = "$viewModel." if with_vm else "$"
        L += [
            "        NavigationStack {",
            "            Form {",
            '                Section("Details") {',
            '                    TextField("Name", text: %sname)' % prefix,
            '                    Toggle("Enable notifications", isOn: %snotificationsEnabled)' % prefix,
            "                }",
            '                Section("Preferences") {',
            "                    Slider(value: %svolume, in: 0...1) {" % prefix,
            '                        Text("Volume")',
            "                    }",
            "                }",
            "                Section {",
            '                    Button("Save") {',
        ]
        if with_vm:
            L.append("                        viewModel.save()")
        else:
            L.append("                        // TODO: persist the form values")
        L += [
            "                    }",
            "                }",
            "            }",
            '            .navigationTitle("%s")' % title,
            "        }",
        ]

    L += [
        "    }",
        "}",
        "",
        "#Preview {",
        "    %s()" % view_name,
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
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE}))
        return 0

    raw_name = q.get("name")
    if not isinstance(raw_name, str) or not raw_name.strip():
        print(json.dumps({
            "error": "missing required field 'name' (non-empty string)",
            "example": _EXAMPLE,
        }))
        return 0

    kind = q.get("kind", "view")
    if kind not in _KINDS:
        print(json.dumps({
            "error": "'kind' must be one of %s" % ", ".join(_KINDS),
            "example": _EXAMPLE,
        }))
        return 0

    with_vm = bool(q.get("with_viewmodel", False))

    try:
        pascal = _pascal_case(raw_name)
        if not pascal:
            print(json.dumps({"error": "could not derive a view name from %r" % raw_name}))
            return 0

        base = pascal[: -len("View")] if pascal.endswith("View") and len(pascal) > 4 else pascal
        if kind == "list" and not base.endswith("List"):
            view_name = base + "ListView"
        elif kind == "form" and not base.endswith("Form"):
            view_name = base + "FormView"
        else:
            view_name = base + "View"

        filename = "%s.swift" % view_name
        code = _build(base, view_name, kind, with_vm)

        notes = [
            "Add %s to your Xcode app target (SwiftUI file, no storyboard needed)." % filename,
            "The #Preview macro requires Xcode 15+.",
        ]
        if kind in ("list", "form"):
            notes.append("NavigationStack requires iOS 16 / macOS 13 or newer.")
        if with_vm:
            notes.append("@Observable requires Swift 5.9+ and an iOS 17 / macOS 14 deployment target.")
        if with_vm:
            notes.append(
                "For deployment targets below iOS 17, swap @Observable for "
                "ObservableObject/@Published and @State for @StateObject."
            )

        result = {"filename": filename, "code": code, "notes": notes}
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "swiftui_view_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
