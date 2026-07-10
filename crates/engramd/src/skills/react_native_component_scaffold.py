#!/usr/bin/env python3
"""react_native_component_scaffold — Engram skill (no network). Generate a
React Native component (TSX or JSX) from a name.

Emits a functional component built from `View`/`Text`/`Pressable` with a
`StyleSheet.create({...})` block. With `screen`, wraps in `SafeAreaView` and
adds a `{ navigation }` prop stub. Imports from 'react-native'. Stdlib only.

Request (stdin): {"name": "UserCard", "typescript": true, "screen": false, "with_styles": true}
Output (stdout): {files: {filename: code}, next_steps}
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


def _build(comp, typescript, screen, with_styles):
    L = ["import React from 'react';"]

    def sty(name):
        return " style={styles.%s}" % name if with_styles else ""

    rn_imports = ["Pressable", "Text", "View"]
    if with_styles:
        rn_imports.append("StyleSheet")
    if screen:
        rn_imports.append("SafeAreaView")
    # Sort for stable, idiomatic ordering.
    rn_named = ", ".join(sorted(rn_imports))
    L.append("import { %s } from 'react-native';" % rn_named)
    L.append("")

    if typescript:
        L.append("type %sProps = {" % comp)
        L.append("  title?: string;")
        if screen:
            L.append("  navigation: any;")
        L.append("};")
        L.append("")
        if screen:
            sig = "export default function %s({ title = '%s', navigation }: %sProps) {" % (
                comp, comp, comp)
        else:
            sig = "export default function %s({ title = '%s' }: %sProps) {" % (comp, comp, comp)
        L.append(sig)
    else:
        if screen:
            L.append("export default function %s({ title = '%s', navigation }) {" % (comp, comp))
        else:
            L.append("export default function %s({ title = '%s' }) {" % (comp, comp))

    # Body
    if screen:
        L.append("  return (")
        L.append("    <SafeAreaView%s>" % sty("container"))
        L.append("      <View%s>" % sty("content"))
        L.append("        <Text%s>{title}</Text>" % sty("title"))
        L.append("        <Pressable")
        if with_styles:
            L.append("          style={styles.button}")
        L.append("          onPress={() => navigation.goBack()}")
        L.append("        >")
        L.append("          <Text%s>Go back</Text>" % sty("buttonText"))
        L.append("        </Pressable>")
        L.append("      </View>")
        L.append("    </SafeAreaView>")
        L.append("  );")
    else:
        L.append("  return (")
        L.append("    <View%s>" % sty("container"))
        L.append("      <Text%s>{title}</Text>" % sty("title"))
        L.append("      <Pressable%s onPress={() => {}}>" % sty("button"))
        L.append("        <Text%s>Press me</Text>" % sty("buttonText"))
        L.append("      </Pressable>")
        L.append("    </View>")
        L.append("  );")
    L.append("}")
    L.append("")

    if with_styles:
        L.append("const styles = StyleSheet.create({")
        if screen:
            L.append("  container: {")
            L.append("    flex: 1,")
            L.append("  },")
            L.append("  content: {")
            L.append("    flex: 1,")
            L.append("    padding: 16,")
            L.append("    gap: 12,")
            L.append("  },")
        else:
            L.append("  container: {")
            L.append("    padding: 16,")
            L.append("    gap: 12,")
            L.append("  },")
        L.append("  title: {")
        L.append("    fontSize: 20,")
        L.append("    fontWeight: '600',")
        L.append("  },")
        L.append("  button: {")
        L.append("    paddingVertical: 10,")
        L.append("    paddingHorizontal: 16,")
        L.append("    borderRadius: 8,")
        L.append("    backgroundColor: '#2563eb',")
        L.append("    alignSelf: 'flex-start',")
        L.append("  },")
        L.append("  buttonText: {")
        L.append("    color: '#ffffff',")
        L.append("    fontWeight: '600',")
        L.append("  },")
        L.append("});")
        L.append("")

    return "\n".join(L)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"name": "UserCard", "typescript": True, "screen": False},
        }))
        return 0

    raw_name = q.get("name")
    if not isinstance(raw_name, str) or not raw_name.strip():
        print(json.dumps({
            "error": "missing required field 'name' (non-empty string)",
            "example": {"name": "UserCard", "typescript": True, "screen": False},
        }))
        return 0

    typescript = bool(q.get("typescript", True))
    screen = bool(q.get("screen", False))
    with_styles = bool(q.get("with_styles", True))

    try:
        comp = _to_pascal_case(raw_name)
        if not comp:
            print(json.dumps({"error": "could not derive a valid name from %r" % raw_name}))
            return 0

        ext = "tsx" if typescript else "jsx"
        filename = "%s.%s" % (comp, ext)
        code = _build(comp, typescript, screen, with_styles)

        next_steps = [
            "Drop %s into your components/ (or screens/) folder." % filename,
        ]
        if screen:
            next_steps.append(
                "Register this screen in your navigator "
                "(e.g. <Stack.Screen name=\"%s\" component={%s} />)." % (comp, comp))
        result = {"files": {filename: code}, "next_steps": next_steps}
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "react_native_component_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
