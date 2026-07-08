#!/usr/bin/env python3
"""subnet_calc — Engram skill (no network, stdlib ipaddress). IPv4/IPv6 subnet math.

Given a CIDR (e.g. "10.0.0.0/24") or an ip+prefix pair, computes network/
broadcast addresses, netmask, usable host range, and address count.

Request (stdin): {"cidr": "10.0.0.0/24"} or {"ip": "10.0.0.5", "prefix": 24}
Output (stdout): {network_address, broadcast_address, netmask, prefix_length,
                  num_addresses, first_usable_host, last_usable_host,
                  is_private, version}
"""
import ipaddress
import json
import sys


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"cidr": "10.0.0.0/24"}}))
        return 0

    cidr = q.get("cidr")
    ip = q.get("ip")
    prefix = q.get("prefix")

    if not cidr and not (ip and prefix is not None):
        print(json.dumps({
            "error": "provide 'cidr' or both 'ip' and 'prefix'",
            "example": {"cidr": "10.0.0.0/24"},
        }))
        return 0

    try:
        if cidr:
            spec = str(cidr).strip()
        else:
            spec = "%s/%s" % (str(ip).strip(), prefix)

        try:
            network = ipaddress.ip_network(spec, strict=False)
        except ValueError as e:
            print(json.dumps({
                "error": "invalid CIDR/IP: %s" % e,
                "example": {"cidr": "10.0.0.0/24"},
            }))
            return 0

        # Compute the usable host range with arithmetic rather than
        # materializing network.hosts() — for large IPv6 networks (e.g. a
        # /64 has ~1.8e19 addresses) list(network.hosts()) would hang/OOM.
        if network.num_addresses >= 4:
            # exclude the network and broadcast addresses
            first_usable = str(network.network_address + 1)
            last_usable = str(network.broadcast_address - 1)
        else:
            # /31, /32, /127, /128: no separate network/broadcast to exclude
            # (RFC 3021 point-to-point links, or a single host)
            first_usable = str(network.network_address)
            last_usable = str(network.broadcast_address)

        result = {
            "network_address": str(network.network_address),
            "broadcast_address": str(network.broadcast_address) if network.version == 4 else None,
            "netmask": str(network.netmask),
            "prefix_length": network.prefixlen,
            "num_addresses": network.num_addresses,
            "first_usable_host": first_usable,
            "last_usable_host": last_usable,
            "is_private": network.is_private,
            "version": network.version,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "subnet_calc failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
