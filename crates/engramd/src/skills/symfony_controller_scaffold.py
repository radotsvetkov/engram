#!/usr/bin/env python3
"""symfony_controller_scaffold — Engram skill (no network). Scaffold a Symfony
controller (PHP 8) extending AbstractController with attribute routing
(#[Route(...)]) and index/show/create actions returning Response/JsonResponse.

Request (stdin): {"name": "product", "route_prefix": "/products"}
Output (stdout): {files, notes, next_steps}
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


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"name": "Product", "route_prefix": "/products"},
        }))
        return 0

    raw_name = q.get("name")
    if not isinstance(raw_name, str) or not raw_name.strip():
        print(json.dumps({
            "error": "missing required field 'name' (non-empty string)",
            "example": {"name": "Product", "route_prefix": "/products"},
        }))
        return 0

    try:
        base = _to_pascal_case(raw_name)
        if not base:
            print(json.dumps({"error": "could not derive a valid name from %r" % raw_name}))
            return 0
        if base.endswith("Controller"):
            base = base[: -len("Controller")] or base
        class_name = "%sController" % base
        snake = _to_snake_case(base)

        raw_prefix = q.get("route_prefix")
        if isinstance(raw_prefix, str) and raw_prefix.strip():
            prefix = "/" + raw_prefix.strip().strip("/")
        else:
            prefix = "/" + snake.replace("_", "-")
        route_name = "app_" + snake  # e.g. app_product

        L = []
        L.append("<?php")
        L.append("")
        L.append("namespace App\\Controller;")
        L.append("")
        L.append("use Symfony\\Bundle\\FrameworkBundle\\Controller\\AbstractController;")
        L.append("use Symfony\\Component\\HttpFoundation\\JsonResponse;")
        L.append("use Symfony\\Component\\HttpFoundation\\Request;")
        L.append("use Symfony\\Component\\HttpFoundation\\Response;")
        L.append("use Symfony\\Component\\Routing\\Attribute\\Route;")
        L.append("")
        L.append("#[Route('%s')]" % prefix)
        L.append("class %s extends AbstractController" % class_name)
        L.append("{")
        L.append("    #[Route('', name: '%s_index', methods: ['GET'])]" % route_name)
        L.append("    public function index(): Response")
        L.append("    {")
        L.append("        // TODO: fetch the collection")
        L.append("        return $this->render('%s/index.html.twig', [" % snake)
        L.append("            '%ss' => []," % snake)
        L.append("        ]);")
        L.append("    }")
        L.append("")
        L.append("    #[Route('/{id}', name: '%s_show', methods: ['GET'], requirements: ['id' => '\\\\d+'])]" % route_name)
        L.append("    public function show(int $id): JsonResponse")
        L.append("    {")
        L.append("        // TODO: look up the entity for $id")
        L.append("        return $this->json([")
        L.append("            'id' => $id,")
        L.append("        ]);")
        L.append("    }")
        L.append("")
        L.append("    #[Route('', name: '%s_create', methods: ['POST'])]" % route_name)
        L.append("    public function create(Request $request): JsonResponse")
        L.append("    {")
        L.append("        $data = json_decode($request->getContent(), true) ?? [];")
        L.append("")
        L.append("        // TODO: validate $data and persist the entity")
        L.append("        return $this->json($data, Response::HTTP_CREATED);")
        L.append("    }")
        L.append("}")
        L.append("")
        code = "\n".join(L)

        path = "src/Controller/%s.php" % class_name
        result = {
            "files": {path: code},
            "notes": [
                "Symfony controller extending AbstractController with PHP 8 attribute routing.",
                "Group prefix on the class; index (GET), show (GET /{id}), create (POST) actions.",
                "Uses Symfony\\Component\\Routing\\Attribute\\Route (Symfony 6.4+/7.x); on older versions use Symfony\\Component\\Routing\\Annotation\\Route.",
            ],
            "next_steps": [
                "Attribute routes are auto-registered via config/routes/attributes.yaml (default in symfony/skeleton).",
                "Create the templates/%s/index.html.twig template for the index action." % snake,
                "Run `php bin/console debug:router` to confirm %s_* routes are registered." % route_name,
            ],
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "symfony_controller_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
