#!/usr/bin/env python3
"""llama_cpp_config_gen — Engram skill (no network). Build a llama.cpp server launch command.

Generates the CLI invocation (and equivalent flag reference) for llama.cpp's
`llama-server` binary from a model path and a few common tuning knobs. Does not
run anything itself — it only builds the command string for you to run locally.

Request (stdin): {"model_path": "/path/to/model.gguf", "context_size"?: 4096,
                  "gpu_layers"?: 0, "port"?: 8080, "host"?: "127.0.0.1",
                  "threads"?: null}
Output (stdout): {command, flags_explained}
"""
import json
import sys


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object"}))
        return 0

    model_path = q.get("model_path")
    if not isinstance(model_path, str) or not model_path.strip():
        print(json.dumps({
            "error": "provide 'model_path' — the path to a local .gguf model file",
            "example": {"model_path": "/models/llama-3-8b.Q4_K_M.gguf", "context_size": 4096, "gpu_layers": 32},
        }))
        return 0

    context_size = q.get("context_size", 4096)
    gpu_layers = q.get("gpu_layers", 0)
    port = q.get("port", 8080)
    host = q.get("host") or "127.0.0.1"
    threads = q.get("threads")

    for name, val in (("context_size", context_size), ("gpu_layers", gpu_layers), ("port", port)):
        if not isinstance(val, int) or val < 0:
            print(json.dumps({"error": "'%s' must be a non-negative integer" % name}))
            return 0
    if threads is not None and (not isinstance(threads, int) or threads <= 0):
        print(json.dumps({"error": "'threads', if given, must be a positive integer"}))
        return 0

    parts = [
        "./llama-server",
        "-m", model_path,
        "-c", str(context_size),
        "-ngl", str(gpu_layers),
        "--host", host,
        "--port", str(port),
    ]
    if threads:
        parts += ["-t", str(threads)]

    def q_arg(s):
        if any(c in s for c in " \t\"'$"):
            return "'" + s.replace("'", "'\\''") + "'"
        return s

    command = " ".join(q_arg(p) for p in parts)

    flags_explained = {
        "-m": "path to the .gguf model file",
        "-c": "context window size in tokens (higher uses more RAM/VRAM)",
        "-ngl": "number of model layers to offload to GPU (0 = CPU only; set to a large number, e.g. 999, to offload all layers on a capable GPU)",
        "--host": "bind address for the OpenAI-compatible HTTP server",
        "--port": "TCP port for the HTTP server",
        "-t": "number of CPU threads to use (only included if 'threads' was given; omitted lets llama.cpp auto-detect)",
    }
    print(json.dumps({
        "command": command,
        "flags_explained": flags_explained,
        "note": "requires a local llama.cpp build (the 'llama-server' binary) — this only generates the launch command, it does not install or run anything",
    }, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
