#!/usr/bin/env python3
"""firewall_rule_reference — Engram skill (no network). Firewall rule cheatsheet.

Returns a static, curated list of common security-hardening firewall rules
(with a one-line purpose for each) for a given tool: ufw, iptables,
aws_security_group, or nginx. Pure reference data — nothing is executed or
looked up, and no state is read from the host. Stdlib only.

Request (stdin): {"tool": "ufw"}   (tool is optional — omit to get all 4)
Output (stdout): {"tool": "ufw", "rules": [{"rule": "...", "purpose": "..."}, ...]}
                 or, with no "tool": {"rules": {"ufw": [...], "iptables": [...], ...}}
"""
import json
import sys

RULES = {
    "ufw": [
        {"rule": "ufw default deny incoming", "purpose": "deny all inbound traffic by default"},
        {"rule": "ufw default allow outgoing", "purpose": "allow all outbound traffic by default"},
        {"rule": "ufw allow ssh   (or: ufw allow 22/tcp)", "purpose": "allow SSH"},
        {"rule": "ufw allow 80,443/tcp", "purpose": "allow HTTP/HTTPS"},
        {"rule": "ufw allow from <ip> to any port 22", "purpose": "allow SSH from one trusted IP only"},
        {"rule": "ufw limit ssh",
         "purpose": "rate-limit repeated SSH connection attempts — brute-force mitigation"},
        {"rule": "ufw enable", "purpose": "activate the firewall (rules have no effect until enabled)"},
    ],
    "iptables": [
        {"rule": "iptables -P INPUT DROP", "purpose": "default deny inbound"},
        {"rule": "iptables -A INPUT -i lo -j ACCEPT", "purpose": "allow loopback traffic"},
        {"rule": "iptables -A INPUT -m state --state ESTABLISHED,RELATED -j ACCEPT",
         "purpose": "allow return traffic for established connections"},
        {"rule": "iptables -A INPUT -p tcp --dport 22 -j ACCEPT", "purpose": "allow SSH"},
        {"rule": "iptables -A INPUT -p tcp --dport 22 -m limit --limit 3/min -j ACCEPT",
         "purpose": "rate-limit SSH — brute-force mitigation"},
        {"rule": "iptables -A INPUT -j DROP",
         "purpose": "final default-deny catch-all; must come after the allow rules"},
    ],
    "aws_security_group": [
        {"rule": "Inbound: allow TCP 22 from your office/VPN CIDR only",
         "purpose": "never expose SSH to 0.0.0.0/0"},
        {"rule": "Inbound: allow TCP 443 from 0.0.0.0/0 for a public web service",
         "purpose": "avoid opening port 80 without a redirect-to-443 rule"},
        {"rule": "Outbound: default allow-all",
         "purpose": "common default; consider restricting to known destinations for high-security workloads"},
        {"rule": "Use security-group-to-security-group references instead of hardcoded IPs",
         "purpose": "robust, self-maintaining rules for intra-VPC traffic"},
    ],
    "nginx": [
        {"rule": "deny all;  (inside a location block for admin paths)",
         "purpose": "block all access to sensitive/admin paths by default"},
        {"rule": "allow <ip>; deny all;", "purpose": "allowlist a single IP, block everyone else"},
        {"rule": "limit_req_zone ...; limit_req ...;", "purpose": "basic request-rate limiting"},
        {"rule": "add_header X-Frame-Options DENY;  (and similar security headers)",
         "purpose": "security headers — e.g. clickjacking protection"},
    ],
}

_SUPPORTED = list(RULES)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "supported_tools": _SUPPORTED}))
        return 0

    tool_raw = q.get("tool")
    if tool_raw is None:
        print(json.dumps({"rules": RULES}, indent=2, default=str))
        return 0

    if not isinstance(tool_raw, str):
        print(json.dumps({"error": "'tool' must be a string", "supported_tools": _SUPPORTED}))
        return 0

    tool = tool_raw.strip().lower()
    if tool not in RULES:
        print(json.dumps({
            "error": "unsupported tool %r" % tool,
            "supported_tools": _SUPPORTED,
            "example": {"tool": "ufw"},
        }))
        return 0

    print(json.dumps({"tool": tool, "rules": RULES[tool]}, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
