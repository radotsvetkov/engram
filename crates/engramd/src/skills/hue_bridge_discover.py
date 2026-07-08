#!/usr/bin/env python3
"""hue_bridge_discover — Engram skill (network). Discover Philips Hue bridges
on the local network via Philips's free, keyless cloud discovery endpoint.

Only finds bridges (id + internal IP address) — pairing still requires
pressing the physical link button on the bridge and then requesting a
username token, which this skill cannot do non-interactively. The output
always includes a `next_steps` string explaining that flow.

Request (stdin): {} (no fields required; extra fields are ignored)
Output (stdout): {bridges_found: [...], count, next_steps}
"""
import json
import sys
import urllib.error
import urllib.request

TIMEOUT = 20
UA = "engram-hue_bridge_discover/1"
DISCOVERY_URL = "https://discovery.meethue.com/"

NEXT_STEPS = (
    "Press the physical link button on your Hue Bridge, then within 30 seconds "
    'POST {"devicetype":"engram#skill"} to http://<bridge-ip>/api to receive a '
    "username token you can reuse for all future Hue API calls."
)


def main():
    try:
        raw = sys.stdin.read()
        q = json.loads(raw) if raw.strip() else {}
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        q = {}

    try:
        req = urllib.request.Request(DISCOVERY_URL, headers={"User-Agent": UA, "Accept": "application/json"})
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            data = json.loads(r.read().decode("utf-8", "replace"))
    except urllib.error.HTTPError as e:
        print(json.dumps({"error": "Hue discovery HTTP error %s: %s" % (e.code, e.reason)}))
        return 0
    except urllib.error.URLError as e:
        print(json.dumps({"error": "network error reaching Hue discovery service: %s" % e.reason}))
        return 0
    except Exception as e:
        print(json.dumps({"error": "Hue bridge discovery failed: %s" % e}))
        return 1

    if not isinstance(data, list):
        print(json.dumps({"error": "unexpected response from Hue discovery service (not a JSON array)"}))
        return 0

    bridges = []
    for item in data:
        if isinstance(item, dict):
            bridges.append({
                "id": item.get("id", ""),
                "internalipaddress": item.get("internalipaddress", ""),
            })

    result = {
        "bridges_found": bridges,
        "count": len(bridges),
        "next_steps": NEXT_STEPS,
    }
    if not bridges:
        result["note"] = (
            "no bridges found on this network via the cloud discovery service — the "
            "bridge must have internet access for this to work; try local mDNS "
            "discovery instead if the bridge is offline"
        )

    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
