#!/usr/bin/env python3
"""django_view_scaffold — Engram skill (no network). Generate a Django
class-based view boilerplate (ListView, DetailView, or CreateView) for a
model, plus a matching urls.py path() snippet.

Request (stdin): {"model_name": "blogPost", "view_type": "list"}
Output (stdout): {filename, view_code, urls_snippet}
"""
import json
import re
import sys

_SUPPORTED_VIEW_TYPES = ["list", "detail", "create"]


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


def _to_snake_case(pascal):
    s = re.sub(r"(?<!^)(?=[A-Z])", "_", pascal)
    return s.lower()


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"model_name": "BlogPost", "view_type": "list"},
        }))
        return 0

    raw_model = q.get("model_name")
    if not isinstance(raw_model, str) or not raw_model.strip():
        print(json.dumps({
            "error": "missing required field 'model_name' (non-empty string)",
            "example": {"model_name": "BlogPost", "view_type": "list"},
        }))
        return 0

    view_type = q.get("view_type", "list")
    if not isinstance(view_type, str) or view_type.strip().lower() not in _SUPPORTED_VIEW_TYPES:
        print(json.dumps({
            "error": "'view_type' must be one of %s, got %r" % (_SUPPORTED_VIEW_TYPES, view_type),
            "supported_view_types": _SUPPORTED_VIEW_TYPES,
        }))
        return 0
    view_type = view_type.strip().lower()

    try:
        model_name = _to_pascal_case(raw_model)
        if not model_name:
            print(json.dumps({"error": "could not derive a valid model name from %r" % raw_model}))
            return 0
        slug = _to_snake_case(model_name)

        view_lines = []
        urls_snippet = ""

        if view_type == "list":
            view_class = "%sListView" % model_name
            view_lines.append("from django.views.generic import ListView")
            view_lines.append("")
            view_lines.append("from .models import %s" % model_name)
            view_lines.append("")
            view_lines.append("")
            view_lines.append("class %s(ListView):" % view_class)
            view_lines.append("    model = %s" % model_name)
            view_lines.append("    template_name = \"%s/%s_list.html\"" % (slug, slug))
            view_lines.append("    context_object_name = \"%s_list\"" % slug)
            view_lines.append("    paginate_by = 20")
            view_lines.append("")
            urls_snippet = (
                "path(\"%s/\", %s.as_view(), name=\"%s-list\"),"
                % (slug.replace("_", "-"), view_class, slug.replace("_", "-"))
            )
        elif view_type == "detail":
            view_class = "%sDetailView" % model_name
            view_lines.append("from django.views.generic import DetailView")
            view_lines.append("")
            view_lines.append("from .models import %s" % model_name)
            view_lines.append("")
            view_lines.append("")
            view_lines.append("class %s(DetailView):" % view_class)
            view_lines.append("    model = %s" % model_name)
            view_lines.append("    template_name = \"%s/%s_detail.html\"" % (slug, slug))
            view_lines.append("    context_object_name = \"%s\"" % slug)
            view_lines.append("")
            urls_snippet = (
                "path(\"%s/<int:pk>/\", %s.as_view(), name=\"%s-detail\"),"
                % (slug.replace("_", "-"), view_class, slug.replace("_", "-"))
            )
        else:  # create
            view_class = "%sCreateView" % model_name
            view_lines.append("from django.views.generic import CreateView")
            view_lines.append("")
            view_lines.append("from .models import %s" % model_name)
            view_lines.append("")
            view_lines.append("")
            view_lines.append("class %s(CreateView):" % view_class)
            view_lines.append("    model = %s" % model_name)
            view_lines.append("    template_name = \"%s/%s_form.html\"" % (slug, slug))
            view_lines.append("    # TODO: fill in the real field names for %s" % model_name)
            view_lines.append("    fields = [\"__all__\"]")
            view_lines.append("    success_url = \"/%s/\"" % slug.replace("_", "-"))
            view_lines.append("")
            urls_snippet = (
                "path(\"%s/new/\", %s.as_view(), name=\"%s-create\"),"
                % (slug.replace("_", "-"), view_class, slug.replace("_", "-"))
            )

        view_code = "\n".join(view_lines)
        result = {
            "filename": "views.py",
            "view_code": view_code,
            "urls_snippet": urls_snippet,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "django_view_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
