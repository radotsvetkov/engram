#!/usr/bin/env python3
"""fastify_route_scaffold — Engram skill (no network). Scaffold a Fastify
plugin/route module for a resource: an `export default async function
(fastify, opts)` plugin registering GET/GET:id/POST/PUT/DELETE routes, each
with a Fastify `schema` (JSON Schema validation stubs).

Request (stdin): {"name": "product", "typescript": false}
Output (stdout): {files, notes, next_steps}
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


def _to_kebab(name):
    return "-".join(w.lower() for w in _split_words(name))


def _camel(name):
    words = _split_words(name)
    if not words:
        return ""
    return words[0].lower() + "".join(w[:1].upper() + w[1:] for w in words[1:])


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
            "example": {"name": "Product", "typescript": False},
        }))
        return 0

    raw_name = q.get("name")
    if not isinstance(raw_name, str) or not raw_name.strip():
        print(json.dumps({
            "error": "missing required field 'name' (non-empty string)",
            "example": {"name": "Product", "typescript": False},
        }))
        return 0

    typescript = bool(q.get("typescript", False))

    try:
        kebab = _to_kebab(raw_name)          # product / user-profile
        camel = _camel(raw_name)             # product / userProfile
        if not kebab:
            print(json.dumps({"error": "could not derive a valid name from %r" % raw_name}))
            return 0
        plural_kebab = _pluralize(kebab)     # products
        plural_camel = _pluralize(camel)     # products
        route_base = "/" + plural_kebab      # /products
        ext = "ts" if typescript else "js"

        # signature bits differ slightly for TS (typed FastifyInstance)
        if typescript:
            header = "import { FastifyInstance, FastifyPluginOptions } from 'fastify';"
            sig = "export default async function %sRoutes(\n  fastify: FastifyInstance,\n  opts: FastifyPluginOptions,\n) {" % plural_camel
            handler = "async (request, reply) =>"
        else:
            header = None
            sig = "export default async function %sRoutes(fastify, opts) {" % plural_camel
            handler = "async (request, reply) =>"

        # reusable schema fragments
        L = []
        if header:
            L.append(header)
            L.append("")
        L.append("// Fastify plugin exposing CRUD routes for the `%s` resource." % kebab)
        L.append("// Register with: fastify.register(%sRoutes, { prefix: '%s' })" % (plural_camel, route_base))
        L.append(sig)
        L.append("  // JSON Schema fragment for a single %s (fill in real properties)." % kebab)
        L.append("  const %sSchema = {" % camel)
        L.append("    type: 'object',")
        L.append("    properties: {")
        L.append("      id: { type: 'integer' },")
        L.append("      name: { type: 'string' },")
        L.append("    },")
        L.append("  };")
        L.append("")
        L.append("  const idParams = {")
        L.append("    type: 'object',")
        L.append("    required: ['id'],")
        L.append("    properties: { id: { type: 'integer' } },")
        L.append("  };")
        L.append("")
        # GET collection
        L.append("  // GET %s" % route_base)
        L.append("  fastify.get('/', {")
        L.append("    schema: {")
        L.append("      response: {")
        L.append("        200: { type: 'array', items: %sSchema }," % camel)
        L.append("      },")
        L.append("    },")
        L.append("  }, %s {" % handler)
        L.append("    // TODO: return the collection")
        L.append("    return [];")
        L.append("  });")
        L.append("")
        # GET one
        L.append("  // GET %s/:id" % route_base)
        L.append("  fastify.get('/:id', {")
        L.append("    schema: {")
        L.append("      params: idParams,")
        L.append("      response: {")
        L.append("        200: %sSchema," % camel)
        L.append("      },")
        L.append("    },")
        L.append("  }, %s {" % handler)
        L.append("    const { id } = request.params;")
        L.append("    // TODO: look up by id")
        L.append("    return reply.code(404).send({ message: `%s ${id} not found` });" % camel)
        L.append("  });")
        L.append("")
        # POST
        L.append("  // POST %s" % route_base)
        L.append("  fastify.post('/', {")
        L.append("    schema: {")
        L.append("      body: {")
        L.append("        type: 'object',")
        L.append("        required: ['name'],")
        L.append("        properties: {")
        L.append("          name: { type: 'string' },")
        L.append("        },")
        L.append("      },")
        L.append("      response: {")
        L.append("        201: %sSchema," % camel)
        L.append("      },")
        L.append("    },")
        L.append("  }, %s {" % handler)
        L.append("    const body = request.body;")
        L.append("    // TODO: persist and return the created resource")
        L.append("    return reply.code(201).send({ id: 1, ...body });")
        L.append("  });")
        L.append("")
        # PUT
        L.append("  // PUT %s/:id" % route_base)
        L.append("  fastify.put('/:id', {")
        L.append("    schema: {")
        L.append("      params: idParams,")
        L.append("      body: {")
        L.append("        type: 'object',")
        L.append("        properties: {")
        L.append("          name: { type: 'string' },")
        L.append("        },")
        L.append("      },")
        L.append("      response: {")
        L.append("        200: %sSchema," % camel)
        L.append("      },")
        L.append("    },")
        L.append("  }, %s {" % handler)
        L.append("    const { id } = request.params;")
        L.append("    // TODO: update and return the resource")
        L.append("    return { id, ...request.body };")
        L.append("  });")
        L.append("")
        # DELETE
        L.append("  // DELETE %s/:id" % route_base)
        L.append("  fastify.delete('/:id', {")
        L.append("    schema: {")
        L.append("      params: idParams,")
        L.append("      response: {")
        L.append("        204: { type: 'null' },")
        L.append("      },")
        L.append("    },")
        L.append("  }, %s {" % handler)
        L.append("    const { id } = request.params;")
        L.append("    // TODO: delete the resource for id")
        L.append("    return reply.code(204).send();")
        L.append("  });")
        L.append("}")
        L.append("")
        code = "\n".join(L)

        path = "routes/%s.%s" % (plural_kebab, ext)
        files = {path: code}

        result = {
            "files": files,
            "notes": [
                "Fastify plugin (`export default async function %sRoutes(fastify, opts)`) with CRUD routes for `%s`." % (plural_camel, kebab),
                "GET '/', GET '/:id', POST '/', PUT '/:id', DELETE '/:id' — each with a Fastify `schema` (params/body/response JSON Schema stubs).",
                "Encapsulated as a plugin; register under a prefix so routes resolve at %s." % route_base,
            ],
            "next_steps": [
                "Register it: `fastify.register(import('./routes/%s.%s'), { prefix: '%s' })`." % (plural_kebab, ext, route_base),
                "Flesh out the `%sSchema`/body schemas with the real properties and validation rules." % camel,
                "Replace the TODO handlers with your data-access layer.",
            ],
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "fastify_route_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
