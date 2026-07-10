#!/usr/bin/env python3
"""android_compose_scaffold — Engram skill (no network). Generate an Android
Jetpack Compose screen in Kotlin from a name.

Emits a Material3 `@Composable` (Scaffold/Column/Text/Button) plus a `@Preview`.
With `with_viewmodel`, also emits a `ViewModel` exposing a `StateFlow` UI state
that the composable collects via `collectAsStateWithLifecycle()`. Stdlib only.

Request (stdin): {"name": "Profile", "screen": true, "with_viewmodel": false}
Output (stdout): {files: {filename: code}, package, next_steps}
"""
import json
import re
import sys

_PACKAGE = "com.example.app"


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


def _screen_file(comp, title, with_viewmodel):
    imports = [
        "androidx.compose.foundation.layout.Column",
        "androidx.compose.foundation.layout.padding",
        "androidx.compose.foundation.layout.Arrangement",
        "androidx.compose.material3.Button",
        "androidx.compose.material3.Scaffold",
        "androidx.compose.material3.Text",
        "androidx.compose.runtime.*",
        "androidx.compose.ui.Modifier",
        "androidx.compose.ui.tooling.preview.Preview",
        "androidx.compose.ui.unit.dp",
    ]
    if with_viewmodel:
        imports.append("androidx.lifecycle.compose.collectAsStateWithLifecycle")
        imports.append("androidx.lifecycle.viewmodel.compose.viewModel")

    L = ["package %s" % _PACKAGE, ""]
    for imp in sorted(imports):
        L.append("import %s" % imp)
    L.append("")
    L.append("@Composable")
    if with_viewmodel:
        L.append("fun %s(" % comp)
        L.append("    modifier: Modifier = Modifier,")
        L.append("    viewModel: %sViewModel = viewModel()," % _vm_base(comp))
        L.append(") {")
        L.append("    val uiState by viewModel.uiState.collectAsStateWithLifecycle()")
        L.append("    Scaffold(modifier = modifier) { innerPadding ->")
        L.append("        Column(")
        L.append("            modifier = Modifier")
        L.append("                .padding(innerPadding)")
        L.append("                .padding(16.dp),")
        L.append("            verticalArrangement = Arrangement.spacedBy(12.dp),")
        L.append("        ) {")
        L.append("            Text(text = uiState.title)")
        L.append("            Button(onClick = { viewModel.onButtonClick() }) {")
        L.append('                Text(text = "Clicked ${uiState.count} times")')
        L.append("            }")
        L.append("        }")
        L.append("    }")
        L.append("}")
    else:
        L.append("fun %s(" % comp)
        L.append("    modifier: Modifier = Modifier,")
        L.append(") {")
        L.append("    var count by remember { mutableStateOf(0) }")
        L.append("    Scaffold(modifier = modifier) { innerPadding ->")
        L.append("        Column(")
        L.append("            modifier = Modifier")
        L.append("                .padding(innerPadding)")
        L.append("                .padding(16.dp),")
        L.append("            verticalArrangement = Arrangement.spacedBy(12.dp),")
        L.append("        ) {")
        L.append('            Text(text = "%s")' % title)
        L.append("            Button(onClick = { count++ }) {")
        L.append('                Text(text = "Clicked $count times")')
        L.append("            }")
        L.append("        }")
        L.append("    }")
        L.append("}")
    L.append("")
    L.append("@Preview(showBackground = true)")
    L.append("@Composable")
    L.append("fun %sPreview() {" % comp)
    L.append("    %s()" % comp)
    L.append("}")
    L.append("")
    return "\n".join(L)


def _component_file(comp, title):
    # Non-screen: a plain reusable Composable (no Scaffold).
    imports = sorted([
        "androidx.compose.foundation.layout.Column",
        "androidx.compose.foundation.layout.Arrangement",
        "androidx.compose.material3.Button",
        "androidx.compose.material3.Text",
        "androidx.compose.runtime.*",
        "androidx.compose.ui.Modifier",
        "androidx.compose.ui.tooling.preview.Preview",
        "androidx.compose.ui.unit.dp",
        "androidx.compose.foundation.layout.padding",
    ])
    L = ["package %s" % _PACKAGE, ""]
    for imp in imports:
        L.append("import %s" % imp)
    L.append("")
    L.append("@Composable")
    L.append("fun %s(" % comp)
    L.append("    modifier: Modifier = Modifier,")
    L.append(") {")
    L.append("    var count by remember { mutableStateOf(0) }")
    L.append("    Column(")
    L.append("        modifier = modifier.padding(16.dp),")
    L.append("        verticalArrangement = Arrangement.spacedBy(12.dp),")
    L.append("    ) {")
    L.append('        Text(text = "%s")' % title)
    L.append("        Button(onClick = { count++ }) {")
    L.append('            Text(text = "Clicked $count times")')
    L.append("        }")
    L.append("    }")
    L.append("}")
    L.append("")
    L.append("@Preview(showBackground = true)")
    L.append("@Composable")
    L.append("fun %sPreview() {" % comp)
    L.append("    %s()" % comp)
    L.append("}")
    L.append("")
    return "\n".join(L)


def _vm_base(comp):
    # Strip a trailing "Screen" so ProfileScreen -> ProfileViewModel.
    if comp.endswith("Screen"):
        return comp[: -len("Screen")]
    return comp


def _viewmodel_file(vm_base, title):
    L = ["package %s" % _PACKAGE, ""]
    for imp in sorted([
        "androidx.lifecycle.ViewModel",
        "kotlinx.coroutines.flow.MutableStateFlow",
        "kotlinx.coroutines.flow.StateFlow",
        "kotlinx.coroutines.flow.asStateFlow",
        "kotlinx.coroutines.flow.update",
    ]):
        L.append("import %s" % imp)
    L.append("")
    L.append("data class %sUiState(" % vm_base)
    L.append('    val title: String = "%s",' % title)
    L.append("    val count: Int = 0,")
    L.append(")")
    L.append("")
    L.append("class %sViewModel : ViewModel() {" % vm_base)
    L.append("    private val _uiState = MutableStateFlow(%sUiState())" % vm_base)
    L.append("    val uiState: StateFlow<%sUiState> = _uiState.asStateFlow()" % vm_base)
    L.append("")
    L.append("    fun onButtonClick() {")
    L.append("        _uiState.update { it.copy(count = it.count + 1) }")
    L.append("    }")
    L.append("}")
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
            "example": {"name": "Profile", "screen": True, "with_viewmodel": False},
        }))
        return 0

    raw_name = q.get("name")
    if not isinstance(raw_name, str) or not raw_name.strip():
        print(json.dumps({
            "error": "missing required field 'name' (non-empty string)",
            "example": {"name": "Profile", "screen": True, "with_viewmodel": False},
        }))
        return 0

    screen = bool(q.get("screen", True))
    with_viewmodel = bool(q.get("with_viewmodel", False))

    try:
        base = _to_pascal_case(raw_name)
        if not base:
            print(json.dumps({"error": "could not derive a valid name from %r" % raw_name}))
            return 0
        title = base

        if screen:
            comp = base if base.endswith("Screen") else base + "Screen"
        else:
            comp = base

        files = {}
        files["%s.kt" % comp] = _screen_file(comp, title, with_viewmodel) if screen else _component_file(comp, title)

        next_steps = [
            "Replace the `package %s` line with your app's package." % _PACKAGE,
            "Ensure Material3 + Compose BOM are on your Gradle classpath.",
        ]
        if with_viewmodel:
            vm_base = _vm_base(comp)
            files["%sViewModel.kt" % vm_base] = _viewmodel_file(vm_base, title)
            next_steps.append(
                "Add `androidx.lifecycle:lifecycle-viewmodel-compose` and "
                "`androidx.lifecycle:lifecycle-runtime-compose` dependencies."
            )

        result = {"package": _PACKAGE, "files": files, "next_steps": next_steps}
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "android_compose_scaffold failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
