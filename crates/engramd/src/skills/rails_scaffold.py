#!/usr/bin/env python3
"""rails_scaffold — Engram skill (no network). Scaffold a Rails controller
(app/controllers/{plural}_controller.rb) with the requested (or default
RESTful 7) actions, a strong-params private method, a before_action
:set_{name}, plus the `resources :{plural}` route line.

Request (stdin): {"name": "product", "actions": ["index", "show", "create"]}
Output (stdout): {files, route_snippet, notes, next_steps}
"""
import json
import re
import sys

_DEFAULT_ACTIONS = ["index", "show", "new", "create", "edit", "update", "destroy"]
_MEMBER_ACTIONS = {"show", "edit", "update", "destroy"}


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


def _pluralize(word):
    if not word:
        return word
    lower = word.lower()
    if lower.endswith(("s", "x", "z", "ch", "sh")):
        return word + "es"
    if lower.endswith("y") and len(word) > 1 and word[-2].lower() not in "aeiou":
        return word[:-1] + "ies"
    return word + "s"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"name": "Product", "actions": ["index", "show", "create"]},
        }))
        return 0

    raw_name = q.get("name")
    if not isinstance(raw_name, str) or not raw_name.strip():
        print(json.dumps({
            "error": "missing required field 'name' (non-empty string)",
            "example": {"name": "Product", "actions": ["index", "show", "create"]},
        }))
        return 0

    raw_actions = q.get("actions")
    if raw_actions is None:
        actions = list(_DEFAULT_ACTIONS)
    elif isinstance(raw_actions, list):
        cleaned = [a.strip().lower() for a in raw_actions if isinstance(a, str) and a.strip()]
        # keep only recognized RESTful actions, preserving canonical order
        actions = [a for a in _DEFAULT_ACTIONS if a in cleaned]
        if not actions:
            actions = list(_DEFAULT_ACTIONS)
    else:
        print(json.dumps({
            "error": "'actions' must be a list of strings if provided",
            "example": {"name": "Product", "actions": ["index", "show", "create"]},
        }))
        return 0

    try:
        base = _to_pascal_case(raw_name)
        if not base:
            print(json.dumps({"error": "could not derive a valid name from %r" % raw_name}))
            return 0
        singular = _to_snake_case(base)          # product
        plural = _pluralize(singular)            # products
        model = base                             # Product
        class_name = "%sController" % _pluralize(base)  # ProductsController

        needs_set = any(a in _MEMBER_ACTIONS for a in actions)
        needs_params = any(a in ("create", "update") for a in actions)

        L = []
        L.append("class %s < ApplicationController" % class_name)
        if needs_set:
            member = [a for a in actions if a in _MEMBER_ACTIONS]
            L.append("  before_action :set_%s, only: %%i[%s]" % (singular, " ".join(member)))
            L.append("")

        def render_action(a):
            if a == "index":
                L.append("  # GET /%s" % plural)
                L.append("  def index")
                L.append("    @%s = %s.all" % (plural, model))
                L.append("  end")
            elif a == "show":
                L.append("  # GET /%s/:id" % plural)
                L.append("  def show")
                L.append("  end")
            elif a == "new":
                L.append("  # GET /%s/new" % plural)
                L.append("  def new")
                L.append("    @%s = %s.new" % (singular, model))
                L.append("  end")
            elif a == "create":
                L.append("  # POST /%s" % plural)
                L.append("  def create")
                L.append("    @%s = %s.new(%s_params)" % (singular, model, singular))
                L.append("")
                L.append("    if @%s.save" % singular)
                L.append("      redirect_to @%s, notice: \"%s was successfully created.\"" % (singular, model))
                L.append("    else")
                L.append("      render :new, status: :unprocessable_entity")
                L.append("    end")
                L.append("  end")
            elif a == "edit":
                L.append("  # GET /%s/:id/edit" % plural)
                L.append("  def edit")
                L.append("  end")
            elif a == "update":
                L.append("  # PATCH/PUT /%s/:id" % plural)
                L.append("  def update")
                L.append("    if @%s.update(%s_params)" % (singular, singular))
                L.append("      redirect_to @%s, notice: \"%s was successfully updated.\"" % (singular, model))
                L.append("    else")
                L.append("      render :edit, status: :unprocessable_entity")
                L.append("    end")
                L.append("  end")
            elif a == "destroy":
                L.append("  # DELETE /%s/:id" % plural)
                L.append("  def destroy")
                L.append("    @%s.destroy" % singular)
                L.append("    redirect_to %s_url, notice: \"%s was successfully destroyed.\"" % (plural, model))
                L.append("  end")

        for i, a in enumerate(actions):
            render_action(a)
            L.append("")

        L.append("  private")
        L.append("")
        if needs_set:
            L.append("  def set_%s" % singular)
            L.append("    @%s = %s.find(params[:id])" % (singular, model))
            L.append("  end")
            L.append("")
        if needs_params:
            L.append("  # Only allow a list of trusted parameters through.")
            L.append("  def %s_params" % singular)
            L.append("    params.require(:%s).permit(:name)" % singular)
            L.append("  end")
        else:
            L.append("  # TODO: add a strong-params method when create/update actions are added.")
        L.append("end")
        L.append("")
        code = "\n".join(L)

        path = "app/controllers/%s_controller.rb" % plural
        route_snippet = "resources :%s" % plural
        result = {
            "files": {path: code},
            "route_snippet": route_snippet,
            "notes": [
                "Rails controller `%s` with actions: %s." % (class_name, ", ".join(actions)),
                "before_action :set_%s for member actions; strong-params `%s_params` (permit :name — edit to taste)." % (singular, singular),
                "2-space indent, idiomatic Rails 7 conventions (status: :unprocessable_entity on failed saves).",
            ],
            "next_steps": [
                "Add `%s` to config/routes.rb." % route_snippet,
                "Update `%s_params` with the real permitted attributes for %s." % (singular, model),
            ],
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "rails_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
