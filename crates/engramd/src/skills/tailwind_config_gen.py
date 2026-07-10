#!/usr/bin/env python3
"""tailwind_config_gen — Engram skill (no network). Generate a tailwind.config.js
from content globs, extended theme colors/fonts, a dark-mode strategy, and
plugins.

Builds the `content` array, `theme.extend.colors` and `theme.extend.fontFamily`
maps, a top-level `darkMode` key, and a `plugins` list of require() calls.
Sensible defaults are used for content_paths when none are given.

Request (stdin): {"content_paths": ["./src/**/*.{js,ts,jsx,tsx}"], "colors": {"brand": "#4f46e5"}, "fonts": {"sans": ["Inter", "sans-serif"]}, "dark_mode": "class", "plugins": ["@tailwindcss/forms"]}
Output (stdout): {filename, code}
"""
import json
import re
import sys

_EXAMPLE = {
    "content_paths": ["./src/**/*.{js,ts,jsx,tsx}"],
    "colors": {"brand": "#4f46e5"},
    "fonts": {"sans": ["Inter", "sans-serif"]},
    "dark_mode": "class",
    "plugins": ["@tailwindcss/forms"],
}
_DEFAULT_CONTENT = ["./src/**/*.{js,ts,jsx,tsx,html}"]


def _js_str(s):
    # emit a single-quoted JS string literal, escaping backslashes and quotes
    return "'" + str(s).replace("\\", "\\\\").replace("'", "\\'") + "'"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE}))
        return 0

    content_paths = q.get("content_paths")
    if content_paths is None:
        content_paths = list(_DEFAULT_CONTENT)
    if not isinstance(content_paths, list) or not all(isinstance(p, str) for p in content_paths):
        print(json.dumps({"error": "'content_paths' must be a list of strings", "example": _EXAMPLE}))
        return 0
    if not content_paths:
        content_paths = list(_DEFAULT_CONTENT)

    colors = q.get("colors") or {}
    if not isinstance(colors, dict):
        print(json.dumps({"error": "'colors' must be an object of {name: hex}", "example": _EXAMPLE}))
        return 0
    fonts = q.get("fonts") or {}
    if not isinstance(fonts, dict):
        print(json.dumps({"error": "'fonts' must be an object of {name: [families]}", "example": _EXAMPLE}))
        return 0
    dark_mode = q.get("dark_mode")
    if dark_mode is not None and dark_mode not in ("class", "media"):
        print(json.dumps({"error": "'dark_mode' must be 'class' or 'media'", "example": _EXAMPLE}))
        return 0
    plugins = q.get("plugins") or []
    if not isinstance(plugins, list) or not all(isinstance(p, str) for p in plugins):
        print(json.dumps({"error": "'plugins' must be a list of strings", "example": _EXAMPLE}))
        return 0

    try:
        ident_re = re.compile(r"^[A-Za-z_$][A-Za-z0-9_$]*$")

        def _key(k):
            k = str(k)
            return k if ident_re.match(k) else _js_str(k)

        lines = []
        lines.append("/** @type {import('tailwindcss').Config} */")
        lines.append("module.exports = {")
        # content
        lines.append("  content: [")
        for p in content_paths:
            lines.append("    %s," % _js_str(p))
        lines.append("  ],")
        # darkMode
        if dark_mode:
            lines.append("  darkMode: %s," % _js_str(dark_mode))
        # theme
        lines.append("  theme: {")
        lines.append("    extend: {")
        if colors:
            lines.append("      colors: {")
            for name, hexv in colors.items():
                lines.append("        %s: %s," % (_key(name), _js_str(hexv)))
            lines.append("      },")
        if fonts:
            lines.append("      fontFamily: {")
            for name, fams in fonts.items():
                if isinstance(fams, list):
                    fam_arr = "[%s]" % ", ".join(_js_str(f) for f in fams)
                else:
                    fam_arr = "[%s]" % _js_str(fams)
                lines.append("        %s: %s," % (_key(name), fam_arr))
            lines.append("      },")
        lines.append("    },")
        lines.append("  },")
        # plugins
        if plugins:
            lines.append("  plugins: [")
            for pl in plugins:
                lines.append("    require(%s)," % _js_str(pl))
            lines.append("  ],")
        else:
            lines.append("  plugins: [],")
        lines.append("};")
        lines.append("")

        code = "\n".join(lines)
        print(json.dumps({"filename": "tailwind.config.js", "code": code}, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "tailwind_config_gen failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
