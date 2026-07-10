#!/usr/bin/env python3
"""capacity_planner — Engram skill (no network). Back-of-envelope capacity math.

Turns simple product inputs (daily active users, requests/user/day, payload
size) into QPS, peak QPS, read/write split, and bandwidth estimates using the
classic system-design napkin formulas. These are rough order-of-magnitude
estimates — always add headroom and validate with a load test. Stdlib only.

Request (stdin): {"daily_active_users": 1000000, "requests_per_user_per_day": 10,
                  "avg_payload_kb": 2, "peak_factor": 3, "read_write_ratio": 10}
Output (stdout): {total_requests_per_day, avg_qps, peak_qps, read_qps, write_qps,
                  bandwidth_mbps, notes}
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
            "example": {"daily_active_users": 1000000,
                        "requests_per_user_per_day": 10, "avg_payload_kb": 2},
        }))
        return 0

    dau = q.get("daily_active_users", 0)
    rpu = q.get("requests_per_user_per_day", 0)
    payload_kb = q.get("avg_payload_kb", 0)
    peak_factor = q.get("peak_factor", 3)
    rw_ratio = q.get("read_write_ratio", 10)

    try:
        dau = float(dau or 0)
        rpu = float(rpu or 0)
        payload_kb = float(payload_kb or 0)
        peak_factor = float(peak_factor if peak_factor is not None else 3)
        rw_ratio = float(rw_ratio if rw_ratio is not None else 10)
    except (TypeError, ValueError) as e:
        print(json.dumps({
            "error": "numeric fields must be numbers: %s" % e,
            "example": {"daily_active_users": 1000000,
                        "requests_per_user_per_day": 10, "avg_payload_kb": 2},
        }))
        return 0

    if dau <= 0 or rpu <= 0:
        print(json.dumps({
            "error": "provide positive 'daily_active_users' and "
                     "'requests_per_user_per_day'",
            "example": {"daily_active_users": 1000000,
                        "requests_per_user_per_day": 10, "avg_payload_kb": 2},
        }))
        return 0
    if peak_factor <= 0:
        peak_factor = 3.0
    if rw_ratio < 0:
        rw_ratio = 0.0

    try:
        seconds_per_day = 86400.0
        total_per_day = dau * rpu
        avg_qps = total_per_day / seconds_per_day
        peak_qps = avg_qps * peak_factor

        # read = write * ratio  ->  write*(1+ratio) = avg_qps
        write_qps = avg_qps / (1.0 + rw_ratio)
        read_qps = write_qps * rw_ratio

        avg_bytes_per_sec = avg_qps * payload_kb * 1024.0
        bandwidth_mbps = avg_bytes_per_sec * 8.0 / 1_000_000.0

        result = {
            "total_requests_per_day": round(total_per_day, 2),
            "avg_qps": round(avg_qps, 2),
            "peak_qps": round(peak_qps, 2),
            "read_qps": round(read_qps, 2),
            "write_qps": round(write_qps, 2),
            "avg_bytes_per_sec": round(avg_bytes_per_sec, 2),
            "bandwidth_mbps": round(bandwidth_mbps, 3),
            "assumptions": {
                "peak_factor": peak_factor,
                "read_write_ratio": rw_ratio,
                "avg_payload_kb": payload_kb,
            },
            "notes": [
                "These are order-of-magnitude estimates — add 30-50% headroom "
                "for growth and safety before provisioning.",
                "peak_qps assumes traffic concentrates by peak_factor=%g over an "
                "even 24h average; real peaks can be spikier." % peak_factor,
                "bandwidth_mbps is application payload only (avg_qps * payload); "
                "it excludes TCP/TLS/HTTP header overhead.",
                "read_qps/write_qps split assumes read = write * %g." % rw_ratio,
                "Always validate with a real load test.",
            ],
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "capacity_planner failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
