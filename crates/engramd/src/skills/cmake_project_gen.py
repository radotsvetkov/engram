#!/usr/bin/env python3
"""cmake_project_gen — Engram skill (no network). Generate a modern CMakeLists.txt
for a C++ project (cmake_minimum_required 3.15+, project(... LANGUAGES CXX),
CXX standard pinned) building an executable or library target, plus a starter
src/main.cpp when an executable is requested.

Request (stdin): {"project_name": "my_app", "cpp_standard": 17, "executable": true, "sources": ["src/main.cpp", "src/util.cpp"], "use_fetchcontent": false}
Output (stdout): {files, notes, next_steps}
"""
import json
import re
import sys

_VALID_STD = [11, 14, 17, 20, 23]


def _sanitize_target(name):
    # CMake target names: keep alnum, dash, underscore; collapse others to '_'
    s = re.sub(r"[^A-Za-z0-9_\-]+", "_", name.strip())
    s = s.strip("_-")
    if not s:
        return ""
    if s[0].isdigit():
        s = "_" + s
    return s


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"project_name": "my_app", "cpp_standard": 17, "executable": True},
        }))
        return 0

    raw_name = q.get("project_name")
    if not isinstance(raw_name, str) or not raw_name.strip():
        print(json.dumps({
            "error": "missing required field 'project_name' (non-empty string)",
            "example": {"project_name": "my_app", "cpp_standard": 17, "executable": True},
        }))
        return 0

    std = q.get("cpp_standard", 17)
    if isinstance(std, bool) or not isinstance(std, int) or std not in _VALID_STD:
        print(json.dumps({
            "error": "'cpp_standard' must be one of %s" % _VALID_STD,
            "supported_standards": _VALID_STD,
        }))
        return 0

    executable = bool(q.get("executable", True))
    use_fetchcontent = bool(q.get("use_fetchcontent", False))

    raw_sources = q.get("sources")
    if raw_sources is None:
        sources = ["src/main.cpp"] if executable else ["src/lib.cpp"]
    elif isinstance(raw_sources, list):
        sources = [s.strip() for s in raw_sources if isinstance(s, str) and s.strip()]
        if not sources:
            sources = ["src/main.cpp"] if executable else ["src/lib.cpp"]
    else:
        print(json.dumps({
            "error": "'sources' must be a list of strings if provided",
            "example": {"project_name": "my_app", "sources": ["src/main.cpp"]},
        }))
        return 0

    try:
        target = _sanitize_target(raw_name)
        if not target:
            print(json.dumps({"error": "could not derive a valid target name from %r" % raw_name}))
            return 0

        L = []
        L.append("cmake_minimum_required(VERSION 3.15)")
        L.append("")
        L.append("project(%s VERSION 0.1.0 LANGUAGES CXX)" % target)
        L.append("")
        L.append("set(CMAKE_CXX_STANDARD %d)" % std)
        L.append("set(CMAKE_CXX_STANDARD_REQUIRED ON)")
        L.append("set(CMAKE_CXX_EXTENSIONS OFF)")
        L.append("")
        if not (raw_sources is not None and isinstance(raw_sources, list) and raw_sources):
            L.append("# Add or remove translation units as the project grows.")
        L.append("set(%s_SOURCES" % target.upper().replace("-", "_"))
        for s in sources:
            L.append("    %s" % s)
        L.append(")")
        L.append("")
        srcvar = "${%s_SOURCES}" % target.upper().replace("-", "_")
        if executable:
            L.append("add_executable(%s %s)" % (target, srcvar))
        else:
            L.append("add_library(%s %s)" % (target, srcvar))
        L.append("")
        L.append("target_include_directories(%s PUBLIC" % target)
        L.append("    ${CMAKE_CURRENT_SOURCE_DIR}/include")
        L.append(")")
        L.append("")
        L.append("target_compile_options(%s PRIVATE" % target)
        L.append("    $<$<CXX_COMPILER_ID:GNU,Clang,AppleClang>:-Wall -Wextra -Wpedantic>")
        L.append("    $<$<CXX_COMPILER_ID:MSVC>:/W4>")
        L.append(")")
        L.append("")
        if use_fetchcontent:
            L.append("# --- Example third-party dependency via FetchContent ---")
            L.append("# include(FetchContent)")
            L.append("# FetchContent_Declare(")
            L.append("#     fmt")
            L.append("#     GIT_REPOSITORY https://github.com/fmtlib/fmt.git")
            L.append("#     GIT_TAG        10.2.1")
            L.append("# )")
            L.append("# FetchContent_MakeAvailable(fmt)")
            L.append("# target_link_libraries(%s PRIVATE fmt::fmt)" % target)
            L.append("")
        code = "\n".join(L)

        files = {"CMakeLists.txt": code}

        if executable:
            main_cpp = (
                "#include <iostream>\n"
                "\n"
                "int main() {\n"
                "    std::cout << \"Hello from %s!\\n\";\n"
                "    return 0;\n"
                "}\n" % target
            )
            # emit main.cpp only if it's among the sources (or the default)
            main_source = next((s for s in sources if s.endswith("main.cpp")), sources[0])
            files[main_source] = main_cpp

        notes = [
            "Modern CMakeLists.txt: cmake_minimum_required 3.15, project(... LANGUAGES CXX), C++%d pinned." % std,
            "Builds %s target `%s`; sources collected in a %s_SOURCES variable." % (
                "an executable" if executable else "a library", target, target.upper().replace("-", "_")),
            "target_include_directories exposes ./include; warning flags set per-compiler via generator expressions.",
        ]
        if use_fetchcontent:
            notes.append("Includes a commented FetchContent example (fmt) — uncomment and adapt to pull a dependency.")

        next_steps = [
            "Configure & build: `cmake -S . -B build && cmake --build build`.",
        ]
        if executable:
            next_steps.append("Run the binary: `./build/%s`." % target)
        else:
            next_steps.append("Link the library into a consumer with target_link_libraries(<app> PRIVATE %s)." % target)

        result = {"files": files, "notes": notes, "next_steps": next_steps}
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "cmake_project_gen failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
