#!/usr/bin/env python3
"""laravel_controller_scaffold — Engram skill (no network). Scaffold a Laravel
resource controller (PHP 8, PSR-12) with the 7 RESTful methods plus the
matching Route::resource(...) line for routes/web.php or routes/api.php.

Request (stdin): {"name": "post", "resource": true, "model": "Post"}
Output (stdout): {files, route_snippet, notes, next_steps}
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
            "example": {"name": "Post", "resource": True, "model": "Post"},
        }))
        return 0

    raw_name = q.get("name")
    if not isinstance(raw_name, str) or not raw_name.strip():
        print(json.dumps({
            "error": "missing required field 'name' (non-empty string)",
            "example": {"name": "Post", "resource": True, "model": "Post"},
        }))
        return 0

    resource = bool(q.get("resource", True))
    raw_model = q.get("model")
    model = None
    if isinstance(raw_model, str) and raw_model.strip():
        model = _to_pascal_case(raw_model)

    try:
        base = _to_pascal_case(raw_name)
        if not base:
            print(json.dumps({"error": "could not derive a valid name from %r" % raw_name}))
            return 0
        # controllers are named <Name>Controller; strip a trailing "Controller"
        if base.endswith("Controller"):
            base = base[: -len("Controller")] or base
        class_name = "%sController" % base
        snake = _to_snake_case(base)
        plural_snake = _pluralize(snake)
        route_uri = plural_snake.replace("_", "-")
        var = "$" + snake

        L = []
        L.append("<?php")
        L.append("")
        L.append("namespace App\\Http\\Controllers;")
        L.append("")
        L.append("use Illuminate\\Http\\Request;")
        L.append("use Illuminate\\Http\\Response;")
        if model:
            L.append("use App\\Models\\%s;" % model)
        L.append("")
        L.append("class %s extends Controller" % class_name)
        L.append("{")
        L.append("    /**")
        L.append("     * Display a listing of the resource.")
        L.append("     */")
        L.append("    public function index(): Response")
        L.append("    {")
        if model:
            L.append("        $%s = %s::all();" % (plural_snake, model))
            L.append("")
            L.append("        return response()->view('%s.index', compact('%s'));" % (plural_snake, plural_snake))
        else:
            L.append("        // TODO: fetch and return the collection")
            L.append("        return response()->noContent();")
        L.append("    }")
        L.append("")
        L.append("    /**")
        L.append("     * Show the form for creating a new resource.")
        L.append("     */")
        L.append("    public function create(): Response")
        L.append("    {")
        L.append("        return response()->view('%s.create');" % plural_snake)
        L.append("    }")
        L.append("")
        L.append("    /**")
        L.append("     * Store a newly created resource in storage.")
        L.append("     */")
        L.append("    public function store(Request $request): Response")
        L.append("    {")
        L.append("        $validated = $request->validate([")
        L.append("            // 'title' => 'required|string|max:255',")
        L.append("        ]);")
        L.append("")
        if model:
            L.append("        %s = %s::create($validated);" % (var, model))
            L.append("")
            L.append("        return response()->redirectToRoute('%s.show', %s);" % (route_uri, var))
        else:
            L.append("        // TODO: persist $validated")
            L.append("        return response()->noContent(201);")
        L.append("    }")
        L.append("")
        L.append("    /**")
        L.append("     * Display the specified resource.")
        L.append("     */")
        if model:
            L.append("    public function show(%s %s): Response" % (model, var))
        else:
            L.append("    public function show(string $id): Response")
        L.append("    {")
        if model:
            L.append("        return response()->view('%s.show', compact('%s'));" % (plural_snake, snake))
        else:
            L.append("        // TODO: fetch and return the resource for $id")
            L.append("        return response()->noContent();")
        L.append("    }")
        L.append("")
        L.append("    /**")
        L.append("     * Show the form for editing the specified resource.")
        L.append("     */")
        if model:
            L.append("    public function edit(%s %s): Response" % (model, var))
        else:
            L.append("    public function edit(string $id): Response")
        L.append("    {")
        if model:
            L.append("        return response()->view('%s.edit', compact('%s'));" % (plural_snake, snake))
        else:
            L.append("        return response()->view('%s.edit');" % plural_snake)
        L.append("    }")
        L.append("")
        L.append("    /**")
        L.append("     * Update the specified resource in storage.")
        L.append("     */")
        if model:
            L.append("    public function update(Request $request, %s %s): Response" % (model, var))
        else:
            L.append("    public function update(Request $request, string $id): Response")
        L.append("    {")
        L.append("        $validated = $request->validate([")
        L.append("            // 'title' => 'required|string|max:255',")
        L.append("        ]);")
        L.append("")
        if model:
            L.append("        %s->update($validated);" % var)
            L.append("")
            L.append("        return response()->redirectToRoute('%s.show', %s);" % (route_uri, var))
        else:
            L.append("        // TODO: update the resource for $id")
            L.append("        return response()->noContent();")
        L.append("    }")
        L.append("")
        L.append("    /**")
        L.append("     * Remove the specified resource from storage.")
        L.append("     */")
        if model:
            L.append("    public function destroy(%s %s): Response" % (model, var))
        else:
            L.append("    public function destroy(string $id): Response")
        L.append("    {")
        if model:
            L.append("        %s->delete();" % var)
            L.append("")
            L.append("        return response()->redirectToRoute('%s.index');" % route_uri)
        else:
            L.append("        // TODO: delete the resource for $id")
            L.append("        return response()->noContent();")
        L.append("    }")
        L.append("}")
        L.append("")
        controller_code = "\n".join(L)

        controller_path = "app/Http/Controllers/%s.php" % class_name
        if resource:
            route_snippet = "Route::resource('%s', %s::class);" % (route_uri, class_name)
        else:
            route_snippet = "Route::get('%s', [%s::class, 'index']);" % (route_uri, class_name)

        files = {controller_path: controller_code}

        next_steps = [
            "Add the route line to routes/web.php (or routes/api.php for an API resource).",
            "Add `use App\\Http\\Controllers\\%s;` at the top of your routes file." % class_name,
        ]
        if model:
            next_steps.append("Run `php artisan make:model %s -m` to generate the %s model and migration." % (model, model))
        else:
            next_steps.append("Consider `php artisan make:model %s -m` and re-run with model set for typed route-model binding." % base)

        result = {
            "files": files,
            "route_snippet": route_snippet,
            "notes": [
                "Resource controller with the 7 RESTful actions (index/create/store/show/edit/update/destroy).",
                "PSR-12 formatted; 4-space indent; typed Request/Response signatures.",
            ],
            "next_steps": next_steps,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "laravel_controller_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
