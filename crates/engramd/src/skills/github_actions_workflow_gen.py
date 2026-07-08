#!/usr/bin/env python3
"""github_actions_workflow_gen — Engram skill (no network). Generate a
working GitHub Actions CI workflow YAML for a given language stack: checkout,
toolchain setup, dependency install, and test run. Built via plain string
formatting (no YAML library).

Request (stdin): {"stack": "node", "test_command": "npm run test:ci"}
Output (stdout): {filename, yaml}
"""
import json
import sys

_SUPPORTED = ["node", "python", "rust", "go"]

_DEFAULT_TEST_CMD = {
    "node": "npm test",
    "python": "pytest",
    "rust": "cargo test",
    "go": "go test ./...",
}


def _node_yaml(test_command):
    return """name: CI

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: '20'
          cache: 'npm'
      - name: Install dependencies
        run: npm ci
      - name: Run tests
        run: {test_command}
""".format(test_command=test_command)


def _python_yaml(test_command):
    return """name: CI

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-python@v5
        with:
          python-version: '3.12'
      - name: Install dependencies
        run: |
          python -m pip install --upgrade pip
          pip install -r requirements.txt
      - name: Run tests
        run: {test_command}
""".format(test_command=test_command)


def _rust_yaml(test_command):
    return """name: CI

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Build
        run: cargo build --verbose
      - name: Run tests
        run: {test_command}
""".format(test_command=test_command)


def _go_yaml(test_command):
    return """name: CI

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-go@v5
        with:
          go-version: '1.22'
      - name: Install dependencies
        run: go mod download
      - name: Run tests
        run: {test_command}
""".format(test_command=test_command)


_BUILDERS = {
    "node": _node_yaml,
    "python": _python_yaml,
    "rust": _rust_yaml,
    "go": _go_yaml,
}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"stack": "node", "test_command": "npm test"},
        }))
        return 0

    stack = q.get("stack")
    if not isinstance(stack, str) or not stack.strip():
        print(json.dumps({
            "error": "missing required field 'stack' (one of: %s)" % ", ".join(_SUPPORTED),
            "example": {"stack": "node"},
        }))
        return 0
    stack = stack.strip().lower()
    if stack not in _SUPPORTED:
        print(json.dumps({
            "error": "unsupported stack %r" % stack,
            "supported_stacks": _SUPPORTED,
        }))
        return 0

    test_command = q.get("test_command")
    if test_command is not None and not isinstance(test_command, str):
        print(json.dumps({"error": "'test_command' must be a string if provided"}))
        return 0
    test_command = (test_command or "").strip() or _DEFAULT_TEST_CMD[stack]

    try:
        yaml_text = _BUILDERS[stack](test_command)
        result = {"filename": ".github/workflows/ci.yml", "yaml": yaml_text}
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "github_actions_workflow_gen failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
