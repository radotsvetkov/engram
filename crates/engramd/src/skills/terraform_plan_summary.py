#!/usr/bin/env python3
"""terraform_plan_summary — Engram skill (no network). Summarize the raw JSON
text produced by `terraform show -json <planfile>`: counts resources being
added, changed, destroyed, replaced, or left as no-ops, and lists the
addresses of every destructive (destroy/replace) change so they're easy to
spot before applying.

Request (stdin): {"plan_json": "{\\"resource_changes\\": [...]}"}
Output (stdout): {to_add, to_change, to_destroy, to_replace, no_op, destructive_changes, total_resources}
"""
import json
import sys


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"plan_json": "{\"resource_changes\": []}"},
        }))
        return 0

    plan_json = q.get("plan_json")
    if not isinstance(plan_json, str) or not plan_json.strip():
        print(json.dumps({
            "error": "missing required field 'plan_json' (string output of `terraform show -json <planfile>`)",
            "example": {"plan_json": "{\"resource_changes\": []}"},
        }))
        return 0

    try:
        plan = json.loads(plan_json)
    except Exception as e:
        print(json.dumps({"error": "'plan_json' is not valid JSON: %s" % e}))
        return 0

    if not isinstance(plan, dict):
        print(json.dumps({"error": "'plan_json' must decode to a JSON object (the terraform plan)"}))
        return 0

    resource_changes = plan.get("resource_changes")
    if resource_changes is None:
        print(json.dumps({
            "error": "no 'resource_changes' key found — this doesn't look like `terraform show -json` output",
        }))
        return 0
    if not isinstance(resource_changes, list):
        print(json.dumps({"error": "'resource_changes' must be a list"}))
        return 0

    try:
        to_add = to_change = to_destroy = to_replace = no_op = 0
        destructive_changes = []

        for rc in resource_changes:
            if not isinstance(rc, dict):
                continue
            address = rc.get("address", "<unknown>")
            change = rc.get("change") or {}
            actions = change.get("actions") or []
            if not isinstance(actions, list):
                actions = []

            has_create = "create" in actions
            has_delete = "delete" in actions
            has_update = "update" in actions
            has_noop = "no-op" in actions

            if has_delete and has_create:
                to_replace += 1
                destructive_changes.append(address)
            elif actions == ["create"]:
                to_add += 1
            elif actions == ["update"]:
                to_change += 1
            elif actions == ["delete"]:
                to_destroy += 1
                destructive_changes.append(address)
            elif actions == ["no-op"]:
                no_op += 1
            elif has_create and not has_delete:
                to_add += 1
            elif has_delete and not has_create:
                to_destroy += 1
                destructive_changes.append(address)
            elif has_update:
                to_change += 1
            elif has_noop:
                no_op += 1

        result = {
            "to_add": to_add,
            "to_change": to_change,
            "to_destroy": to_destroy,
            "to_replace": to_replace,
            "no_op": no_op,
            "destructive_changes": destructive_changes,
            "total_resources": len(resource_changes),
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "terraform_plan_summary failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
