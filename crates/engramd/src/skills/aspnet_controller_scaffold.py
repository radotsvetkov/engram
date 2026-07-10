#!/usr/bin/env python3
"""aspnet_controller_scaffold — Engram skill (no network). Scaffold an
ASP.NET Core Web API: either an [ApiController] class extending ControllerBase
with async CRUD actions returning ActionResult<T>, or Minimal API
app.MapGet/MapPost(...) endpoint registrations for Program.cs.

Request (stdin): {"name": "product", "style": "controller", "model": "Product"}
Output (stdout): {files, notes, next_steps}
"""
import json
import re
import sys

_STYLES = ["controller", "minimal_api"]


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


def _camel(pascal):
    return pascal[:1].lower() + pascal[1:] if pascal else pascal


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
            "example": {"name": "Product", "style": "controller", "model": "Product"},
        }))
        return 0

    raw_name = q.get("name")
    if not isinstance(raw_name, str) or not raw_name.strip():
        print(json.dumps({
            "error": "missing required field 'name' (non-empty string)",
            "example": {"name": "Product", "style": "controller", "model": "Product"},
        }))
        return 0

    style = q.get("style", "controller")
    if not isinstance(style, str) or style.strip().lower() not in _STYLES:
        print(json.dumps({
            "error": "'style' must be one of %s" % _STYLES,
            "supported_styles": _STYLES,
        }))
        return 0
    style = style.strip().lower()

    raw_model = q.get("model")
    model = _to_pascal_case(raw_model) if isinstance(raw_model, str) and raw_model.strip() else None

    try:
        base = _to_pascal_case(raw_name)
        if not base:
            print(json.dumps({"error": "could not derive a valid name from %r" % raw_name}))
            return 0
        if base.endswith("Controller"):
            base = base[: -len("Controller")] or base
        entity = model or base
        entity_var = _camel(entity)
        plural = _pluralize(base)
        namespace = "WebApi"

        if style == "controller":
            class_name = "%sController" % base
            L = []
            L.append("using Microsoft.AspNetCore.Mvc;")
            L.append("")
            L.append("namespace %s.Controllers;" % namespace)
            L.append("")
            L.append("[ApiController]")
            L.append("[Route(\"api/[controller]\")]")
            L.append("public class %s : ControllerBase" % class_name)
            L.append("{")
            L.append("    // TODO: inject a service/repository via the constructor.")
            L.append("")
            L.append("    [HttpGet]")
            L.append("    public async Task<ActionResult<IEnumerable<%s>>> GetAll()" % entity)
            L.append("    {")
            L.append("        // TODO: return the collection")
            L.append("        return Ok(Array.Empty<%s>());" % entity)
            L.append("    }")
            L.append("")
            L.append("    [HttpGet(\"{id:int}\")]")
            L.append("    public async Task<ActionResult<%s>> GetById(int id)" % entity)
            L.append("    {")
            L.append("        // TODO: look up by id")
            L.append("        return NotFound();")
            L.append("    }")
            L.append("")
            L.append("    [HttpPost]")
            L.append("    public async Task<ActionResult<%s>> Create([FromBody] %s %s)" % (entity, entity, entity_var))
            L.append("    {")
            L.append("        // TODO: persist %s" % entity_var)
            L.append("        return CreatedAtAction(nameof(GetById), new { id = 0 }, %s);" % entity_var)
            L.append("    }")
            L.append("")
            L.append("    [HttpPut(\"{id:int}\")]")
            L.append("    public async Task<IActionResult> Update(int id, [FromBody] %s %s)" % (entity, entity_var))
            L.append("    {")
            L.append("        // TODO: update the entity with the given id")
            L.append("        return NoContent();")
            L.append("    }")
            L.append("")
            L.append("    [HttpDelete(\"{id:int}\")]")
            L.append("    public async Task<IActionResult> Delete(int id)")
            L.append("    {")
            L.append("        // TODO: delete the entity with the given id")
            L.append("        return NoContent();")
            L.append("    }")
            L.append("}")
            L.append("")
            code = "\n".join(L)

            files = {"Controllers/%s.cs" % class_name: code}
            if model:
                mL = []
                mL.append("namespace %s.Models;" % namespace)
                mL.append("")
                mL.append("public class %s" % model)
                mL.append("{")
                mL.append("    public int Id { get; set; }")
                mL.append("    // TODO: add the remaining properties.")
                mL.append("    public string Name { get; set; } = string.Empty;")
                mL.append("}")
                mL.append("")
                files["Models/%s.cs" % model] = "\n".join(mL)

            notes = [
                "[ApiController] controller with attribute routing api/[controller] and async ActionResult<T> actions.",
                "GET (all), GET {id}, POST, PUT {id}, DELETE {id}.",
                "Adjust the `namespace WebApi` to match your project's root namespace.",
            ]
            next_steps = [
                "Ensure builder.Services.AddControllers() and app.MapControllers() are in Program.cs.",
                "Inject your data service/DbContext through the constructor.",
            ]
        else:  # minimal_api
            route = "/api/%s" % _pluralize(base).lower()
            L = []
            L.append("// Minimal API endpoints for %s — add to Program.cs after `var app = builder.Build();`." % entity)
            L.append("")
            L.append("var %s = app.MapGroup(\"%s\");" % (_camel(plural), route))
            L.append("")
            L.append("%s.MapGet(\"/\", () =>" % _camel(plural))
            L.append("{")
            L.append("    // TODO: return the collection")
            L.append("    return Results.Ok(Array.Empty<%s>());" % entity)
            L.append("});")
            L.append("")
            L.append("%s.MapGet(\"/{id:int}\", (int id) =>" % _camel(plural))
            L.append("{")
            L.append("    // TODO: look up by id")
            L.append("    return Results.NotFound();")
            L.append("});")
            L.append("")
            L.append("%s.MapPost(\"/\", (%s %s) =>" % (_camel(plural), entity, entity_var))
            L.append("{")
            L.append("    // TODO: persist %s" % entity_var)
            L.append("    return Results.Created($\"%s/0\", %s);" % (route, entity_var))
            L.append("});")
            L.append("")
            L.append("%s.MapPut(\"/{id:int}\", (int id, %s %s) =>" % (_camel(plural), entity, entity_var))
            L.append("{")
            L.append("    // TODO: update the entity with the given id")
            L.append("    return Results.NoContent();")
            L.append("});")
            L.append("")
            L.append("%s.MapDelete(\"/{id:int}\", (int id) =>" % _camel(plural))
            L.append("{")
            L.append("    // TODO: delete the entity with the given id")
            L.append("    return Results.NoContent();")
            L.append("});")
            L.append("")
            code = "\n".join(L)
            files = {"Endpoints/%sEndpoints.cs" % base: code}
            if model:
                files["Models/%s.cs" % model] = (
                    "namespace %s.Models;\n\npublic record %s(int Id, string Name);\n" % (namespace, model)
                )
            notes = [
                "Minimal API endpoint registrations grouped under %s via MapGroup." % route,
                "MapGet/MapGet{id}/MapPost/MapPut/MapDelete returning Results.* (typed results).",
                "Paste the body into Program.cs between `builder.Build()` and `app.Run()`.",
            ]
            next_steps = [
                "Register services (DI) on `builder.Services` before `builder.Build()`.",
                "Replace the in-line lambdas with handler methods as the endpoints grow.",
            ]

        result = {"files": files, "notes": notes, "next_steps": next_steps}
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "aspnet_controller_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
