#!/usr/bin/env python3
"""redux_slice_scaffold — Engram skill (no network). Generate a Redux Toolkit
slice or a Zustand store from a name and a state shape.

library=redux_toolkit -> a `createSlice` with typed initialState, a `reset`
reducer and one `set<Field>` reducer per field, plus exported actions and the
reducer. library=zustand -> a `create` store with the state fields and a setter
per field. State defaults come from `state_shape` (field -> default value).

Request (stdin): {"name": "counter", "state_shape": {"value": 0, "label": "hi"}, "library": "redux_toolkit", "typescript": true}
Output (stdout): {filename, code}
"""
import json
import re
import sys

_EXAMPLE = {
    "name": "counter",
    "state_shape": {"value": 0, "label": "hi"},
    "library": "redux_toolkit",
    "typescript": True,
}


def _split_words(name):
    parts = re.split(r"[^A-Za-z0-9]+", name.strip())
    words = []
    for part in parts:
        if not part:
            continue
        sub = re.findall(r"[A-Z]+(?=[A-Z][a-z0-9])|[A-Z]?[a-z0-9]+|[A-Z]+", part)
        words.extend(sub if sub else [part])
    return [w for w in words if w]


def _pascal(name):
    return "".join(w[:1].upper() + w[1:] for w in _split_words(name))


def _camel(name):
    p = _pascal(name)
    return p[:1].lower() + p[1:] if p else p


def _cap(field):
    return field[:1].upper() + field[1:] if field else field


def _ts_type(v):
    if isinstance(v, bool):
        return "boolean"
    if isinstance(v, (int, float)):
        return "number"
    if isinstance(v, str):
        return "string"
    if isinstance(v, list):
        return "any[]"
    if v is None:
        return "any"
    return "any"


def _js_default(v):
    return json.dumps(v)


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
        print(json.dumps({"error": "missing required field 'name' (non-empty string)", "example": _EXAMPLE}))
        return 0

    library = str(q.get("library", "redux_toolkit")).lower()
    if library not in ("redux_toolkit", "zustand"):
        print(json.dumps({"error": "'library' must be 'redux_toolkit' or 'zustand'", "example": _EXAMPLE}))
        return 0
    typescript = bool(q.get("typescript", True))
    state_shape = q.get("state_shape") or {}
    if not isinstance(state_shape, dict):
        print(json.dumps({"error": "'state_shape' must be an object of {field: default}", "example": _EXAMPLE}))
        return 0

    try:
        ident_re = re.compile(r"^[A-Za-z_$][A-Za-z0-9_$]*$")
        fields = {k: v for k, v in state_shape.items() if ident_re.match(str(k))}

        camel = _camel(raw_name)
        pascal = _pascal(raw_name)
        if not pascal:
            print(json.dumps({"error": "could not derive a valid name from %r" % raw_name}))
            return 0
        ext = "ts" if typescript else "js"
        state_ty = "%sState" % pascal

        if library == "redux_toolkit":
            filename = "%sSlice.%s" % (camel, ext)
            lines = ["import { createSlice } from '@reduxjs/toolkit';"]
            if typescript:
                lines.append("import type { PayloadAction } from '@reduxjs/toolkit';")
            lines.append("")
            if typescript:
                lines.append("interface %s {" % state_ty)
                if fields:
                    for f, v in fields.items():
                        lines.append("  %s: %s;" % (f, _ts_type(v)))
                else:
                    lines.append("  // TODO: define state fields")
                lines.append("}")
                lines.append("")
                lines.append("const initialState: %s = {" % state_ty)
            else:
                lines.append("const initialState = {")
            for f, v in fields.items():
                lines.append("  %s: %s," % (f, _js_default(v)))
            lines.append("};")
            lines.append("")
            lines.append("const %sSlice = createSlice({" % camel)
            lines.append("  name: '%s'," % camel)
            lines.append("  initialState,")
            lines.append("  reducers: {")
            lines.append("    reset: () => initialState,")
            for f, v in fields.items():
                pay = "PayloadAction<%s>" % _ts_type(v) if typescript else ""
                arg = "action: %s" % pay if typescript else "action"
                lines.append("    set%s: (state, %s) => {" % (_cap(f), arg))
                lines.append("      state.%s = action.payload;" % f)
                lines.append("    },")
            lines.append("  },")
            lines.append("});")
            lines.append("")
            action_names = ["reset"] + ["set%s" % _cap(f) for f in fields]
            lines.append("export const { %s } = %sSlice.actions;" % (", ".join(action_names), camel))
            lines.append("export default %sSlice.reducer;" % camel)
            lines.append("")
        else:  # zustand
            filename = "use%sStore.%s" % (pascal, ext)
            store_ty = "%sStore" % pascal
            lines = ["import { create } from 'zustand';"]
            lines.append("")
            if typescript:
                lines.append("interface %s {" % store_ty)
                for f, v in fields.items():
                    lines.append("  %s: %s;" % (f, _ts_type(v)))
                for f, v in fields.items():
                    lines.append("  set%s: (%s: %s) => void;" % (_cap(f), f, _ts_type(v)))
                lines.append("  reset: () => void;")
                lines.append("}")
                lines.append("")
                lines.append("const initialState = {")
            else:
                lines.append("const initialState = {")
            for f, v in fields.items():
                lines.append("  %s: %s," % (f, _js_default(v)))
            lines.append("};")
            lines.append("")
            create_ty = "<%s>" % store_ty if typescript else ""
            lines.append("export const use%sStore = create%s((set) => ({" % (pascal, create_ty))
            lines.append("  ...initialState,")
            for f, v in fields.items():
                lines.append("  set%s: (%s) => set({ %s })," % (_cap(f), f, f))
            lines.append("  reset: () => set(initialState),")
            lines.append("}));")
            lines.append("")

        code = "\n".join(lines)
        print(json.dumps({"filename": filename, "code": code}, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "redux_slice_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
