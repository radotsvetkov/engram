#!/usr/bin/env python3
"""nextjs_route_scaffold — Engram skill (no network). Generate a Next.js route
file for the app router or the pages router.

app + page  -> app/{route}/page.tsx  (default-export React server component)
app + api   -> app/{route}/route.ts  (GET/POST via the Web Request/Response API)
pages + page-> pages/{route}.tsx      (default-export React page)
pages + api -> pages/api/{route}.ts   (default handler with req/res)

Request (stdin): {"route": "dashboard/settings", "router": "app", "method": "page", "typescript": true}
Output (stdout): {filename, code}
"""
import json
import re
import sys

_EXAMPLE = {"route": "dashboard/settings", "router": "app", "method": "page", "typescript": True}


def _clean_route(route):
    # normalize slashes, strip leading/trailing slashes and file extensions
    route = route.strip().strip("/")
    route = re.sub(r"\.(tsx?|jsx?)$", "", route)
    parts = [p for p in route.split("/") if p]
    return "/".join(parts)


def _to_pascal_case(name):
    parts = re.split(r"[^A-Za-z0-9]+", name)
    words = []
    for part in parts:
        if not part:
            continue
        sub = re.findall(r"[A-Z]+(?=[A-Z][a-z0-9])|[A-Z]?[a-z0-9]+|[A-Z]+", part)
        words.extend(sub if sub else [part])
    return "".join(w[:1].upper() + w[1:] for w in words if w) or "Page"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE}))
        return 0

    raw_route = q.get("route")
    if not isinstance(raw_route, str) or not raw_route.strip():
        print(json.dumps({
            "error": "missing required field 'route' (non-empty string, e.g. 'dashboard/settings')",
            "example": _EXAMPLE,
        }))
        return 0

    router = str(q.get("router", "app")).lower()
    if router not in ("app", "pages"):
        print(json.dumps({"error": "'router' must be 'app' or 'pages'", "example": _EXAMPLE}))
        return 0
    method = str(q.get("method", "page")).lower()
    if method not in ("page", "api"):
        print(json.dumps({"error": "'method' must be 'page' or 'api'", "example": _EXAMPLE}))
        return 0
    typescript = bool(q.get("typescript", True))

    try:
        route = _clean_route(raw_route)
        if not route:
            print(json.dumps({"error": "could not derive a valid route from %r" % raw_route}))
            return 0

        last_seg = route.split("/")[-1]
        comp_name = _to_pascal_case(last_seg)

        if router == "app":
            if method == "page":
                ext = "tsx" if typescript else "jsx"
                filename = "app/%s/page.%s" % (route, ext)
                lines = [
                    "export default function %sPage() {" % comp_name,
                    "  return (",
                    "    <main>",
                    "      <h1>%s</h1>" % comp_name,
                    "      {/* TODO: implement %s */}" % route,
                    "    </main>",
                    "  );",
                    "}",
                    "",
                ]
            else:  # api
                ext = "ts" if typescript else "js"
                filename = "app/%s/route.%s" % (route, ext)
                req_ty = ": Request" if typescript else ""
                lines = [
                    "export async function GET(request%s) {" % req_ty,
                    "  return Response.json({ ok: true, route: %r });" % ("/" + route),
                    "}",
                    "",
                    "export async function POST(request%s) {" % req_ty,
                    "  const body = await request.json();",
                    "  return Response.json({ received: body }, { status: 201 });",
                    "}",
                    "",
                ]
        else:  # pages router
            if method == "page":
                ext = "tsx" if typescript else "jsx"
                filename = "pages/%s.%s" % (route, ext)
                lines = [
                    "export default function %sPage() {" % comp_name,
                    "  return (",
                    "    <main>",
                    "      <h1>%s</h1>" % comp_name,
                    "      {/* TODO: implement %s */}" % route,
                    "    </main>",
                    "  );",
                    "}",
                    "",
                ]
            else:  # api
                ext = "ts" if typescript else "js"
                filename = "pages/api/%s.%s" % (route, ext)
                if typescript:
                    lines = [
                        "import type { NextApiRequest, NextApiResponse } from 'next';",
                        "",
                        "export default function handler(",
                        "  req: NextApiRequest,",
                        "  res: NextApiResponse,",
                        ") {",
                        "  if (req.method === 'POST') {",
                        "    return res.status(201).json({ received: req.body });",
                        "  }",
                        "  return res.status(200).json({ ok: true, route: %r });" % ("/api/" + route),
                        "}",
                        "",
                    ]
                else:
                    lines = [
                        "export default function handler(req, res) {",
                        "  if (req.method === 'POST') {",
                        "    return res.status(201).json({ received: req.body });",
                        "  }",
                        "  return res.status(200).json({ ok: true, route: %r });" % ("/api/" + route),
                        "}",
                        "",
                    ]

        code = "\n".join(lines)
        print(json.dumps({"filename": filename, "code": code}, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "nextjs_route_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
