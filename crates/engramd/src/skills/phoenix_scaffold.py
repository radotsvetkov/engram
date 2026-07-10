#!/usr/bin/env python3
"""phoenix_scaffold — Engram skill (no network). Scaffold an Elixir/Phoenix
controller (index/show actions) or a Phoenix LiveView module (mount/3,
handle_event/3, render/1 with a ~H heex template). Idiomatic Elixir.

Request (stdin): {"name": "product", "kind": "controller"}
Output (stdout): {files, notes, next_steps}
"""
import json
import re
import sys

_KINDS = ["controller", "live"]


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
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"name": "Product", "kind": "controller"},
        }))
        return 0

    raw_name = q.get("name")
    if not isinstance(raw_name, str) or not raw_name.strip():
        print(json.dumps({
            "error": "missing required field 'name' (non-empty string)",
            "example": {"name": "Product", "kind": "controller"},
        }))
        return 0

    kind = q.get("kind", "controller")
    if not isinstance(kind, str) or kind.strip().lower() not in _KINDS:
        print(json.dumps({
            "error": "'kind' must be one of %s" % _KINDS,
            "supported_kinds": _KINDS,
        }))
        return 0
    kind = kind.strip().lower()

    try:
        base = _to_pascal_case(raw_name)
        if not base:
            print(json.dumps({"error": "could not derive a valid name from %r" % raw_name}))
            return 0
        for suffix in ("Controller", "Live"):
            if base.endswith(suffix):
                base = base[: -len(suffix)] or base
        snake = _to_snake_case(base)
        app = "MyApp"  # placeholder — user replaces with their OTP app module

        if kind == "controller":
            module = "%sWeb.%sController" % (app, base)
            L = []
            L.append("defmodule %s do" % module)
            L.append("  use %sWeb, :controller" % app)
            L.append("")
            L.append("  def index(conn, _params) do")
            L.append("    # TODO: load the collection, e.g. %ss = MyApp.Catalog.list_%ss()" % (snake, snake))
            L.append("    render(conn, :index, %ss: [])" % snake)
            L.append("  end")
            L.append("")
            L.append("  def show(conn, %{\"id\" => id}) do")
            L.append("    # TODO: %s = MyApp.Catalog.get_%s!(id)" % (snake, snake))
            L.append("    render(conn, :show, %s: %%{id: id})" % snake)
            L.append("  end")
            L.append("end")
            L.append("")
            code = "\n".join(L)
            path = "lib/%s_web/controllers/%s_controller.ex" % (_to_snake_case(app), snake)
            notes = [
                "Phoenix controller with index/2 and show/2 actions.",
                "Replace the `MyApp` placeholder with your OTP app module (e.g. the value of :app in mix.exs, PascalCased).",
                "render/3 delegates to a matching %sHTML view/component module." % base,
            ]
            next_steps = [
                "Add routes to lib/%s_web/router.ex, e.g. `resources \"/%ss\", %sController, only: [:index, :show]`." % (_to_snake_case(app), snake, base),
                "Create the %sHTML module + index.html.heex / show.html.heex templates." % base,
            ]
        else:  # live
            module = "%sWeb.%sLive" % (app, base)
            L = []
            L.append("defmodule %s do" % module)
            L.append("  use %sWeb, :live_view" % app)
            L.append("")
            L.append("  @impl true")
            L.append("  def mount(_params, _session, socket) do")
            L.append("    {:ok, assign(socket, count: 0, %ss: [])}" % snake)
            L.append("  end")
            L.append("")
            L.append("  @impl true")
            L.append("  def handle_event(\"increment\", _params, socket) do")
            L.append("    {:noreply, update(socket, :count, &(&1 + 1))}")
            L.append("  end")
            L.append("")
            L.append("  @impl true")
            L.append("  def render(assigns) do")
            L.append("    ~H\"\"\"")
            L.append("    <div>")
            L.append("      <h1>%s</h1>" % base)
            L.append("      <p>Count: {@count}</p>")
            L.append("      <button phx-click=\"increment\">Increment</button>")
            L.append("    </div>")
            L.append("    \"\"\"")
            L.append("  end")
            L.append("end")
            L.append("")
            code = "\n".join(L)
            path = "lib/%s_web/live/%s_live.ex" % (_to_snake_case(app), snake)
            notes = [
                "Phoenix LiveView with mount/3, handle_event/3, and render/1 (~H heex).",
                "Replace the `MyApp` placeholder with your OTP app module.",
                "The `{@count}` interpolation is HEEx syntax for Phoenix 1.7+ (use `<%= @count %>` on older versions).",
            ]
            next_steps = [
                "Add a live route to lib/%s_web/router.ex, e.g. `live \"/%s\", %sLive`." % (_to_snake_case(app), snake, base),
                "Wire real domain data into mount/3 and the handle_event/3 callbacks.",
            ]

        result = {"files": {path: code}, "notes": notes, "next_steps": next_steps}
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "phoenix_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
