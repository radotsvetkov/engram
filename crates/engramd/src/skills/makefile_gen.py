#!/usr/bin/env python3
"""makefile_gen — Engram skill (no network). Generate a real, valid Makefile
with standard targets for a given stack (node/python/rust/go), using each
ecosystem's idiomatic commands.

Request (stdin): {"stack": "node"|"python"|"rust"|"go", "targets"?: [str]}
Output (stdout): {"filename": "Makefile", "content": str,
                   "targets_included": [str], "skipped"?: [str]}
"""
import json
import sys

# Each stack maps target-name -> (recipe_lines, comment). Order here is the
# order targets appear in the generated Makefile.
_STACKS = {
    "node": [
        ("install", ["npm install"], None),
        ("build", ["npm run build"], None),
        ("test", ["npm test"], None),
        ("lint", ["npm run lint"], None),
        ("clean", ["rm -rf node_modules dist"], None),
        ("run", ["npm start"], None),
        ("start", ["npm start"], None),
    ],
    "python": [
        ("install", ["pip install -r requirements.txt"], None),
        ("test", ["pytest"], None),
        ("lint", ["ruff check ."], "suggestion: swap for 'flake8 .' if you prefer flake8"),
        ("format", ["black ."], None),
        ("clean", ["find . -type d -name __pycache__ -exec rm -rf {} +"], None),
        ("run", ["python main.py"], "placeholder — adjust to your actual entry point"),
    ],
    "rust": [
        ("build", ["cargo build"], None),
        ("test", ["cargo test"], None),
        ("lint", ["cargo clippy -- -D warnings"], None),
        ("format", ["cargo fmt"], None),
        ("clean", ["cargo clean"], None),
        ("run", ["cargo run"], None),
    ],
    "go": [
        ("build", ["go build ./..."], None),
        ("test", ["go test ./..."], None),
        ("lint", ["go vet ./..."], None),
        ("format", ["gofmt -w ."], None),
        ("clean", ["go clean"], None),
        ("run", ["go run ."], None),
    ],
}


def _generate_makefile(stack, requested_targets):
    all_entries = _STACKS[stack]
    all_names = [name for name, _, _ in all_entries]

    skipped = []
    if requested_targets is not None:
        wanted_lower = {t.strip().lower() for t in requested_targets if isinstance(t, str) and t.strip()}
        known_lower = {n.lower() for n in all_names}
        skipped = sorted(t for t in wanted_lower if t not in known_lower)
        entries = [(name, recipe, note) for name, recipe, note in all_entries if name.lower() in wanted_lower]
    else:
        entries = all_entries

    targets_included = [name for name, _, _ in entries]

    lines = []
    lines.append(".PHONY: %s" % " ".join(targets_included))
    lines.append("")
    for name, recipe, note in entries:
        if note:
            lines.append("# %s" % note)
        lines.append("%s:" % name)
        for cmd in recipe:
            lines.append("\t%s" % cmd)
        lines.append("")

    content = "\n".join(lines).rstrip("\n") + "\n"
    return content, targets_included, skipped


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"stack": "node", "targets": ["install", "build", "test"]}}))
        return 0

    stack = q.get("stack")
    if not isinstance(stack, str) or stack.strip().lower() not in _STACKS:
        print(json.dumps({
            "error": "provide 'stack' as one of: node, python, rust, go",
            "example": {"stack": "python", "targets": ["install", "test", "lint"]},
        }))
        return 0
    stack = stack.strip().lower()

    targets = q.get("targets")
    if targets is not None and (not isinstance(targets, list) or not all(isinstance(t, str) for t in targets)):
        print(json.dumps({"error": "'targets', if provided, must be a list of strings"}))
        return 0

    try:
        content, targets_included, skipped = _generate_makefile(stack, targets)
    except Exception as e:
        print(json.dumps({"error": "Makefile generation failed: %s" % e}))
        return 1

    result = {
        "filename": "Makefile",
        "content": content,
        "targets_included": targets_included,
    }
    if skipped:
        result["skipped"] = skipped
    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
