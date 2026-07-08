#!/usr/bin/env python3
"""cve_lookup — Engram skill (keyless). Look up CVEs via the public NVD API.

Queries https://services.nvd.nist.gov/rest/json/cves/2.0 either by exact
CVE ID or by a free-text keyword search. No API key is required for light
usage, but NVD rate-limits unauthenticated requests (~5 req/30s) — a 403/429
is reported as a clear, retryable error rather than a crash.

Request (stdin): {"cve_id": "CVE-2021-44228"} or {"keyword": "log4j"}
Output (stdout): {results: [{id, description, published, cvss_score,
                  severity, link}, ...], result_count}
"""
import json
import sys
import urllib.error
import urllib.parse
import urllib.request

TIMEOUT = 25
UA = "engram-cve/1"
BASE_URL = "https://services.nvd.nist.gov/rest/json/cves/2.0"


def _english_description(descriptions):
    for d in descriptions or []:
        if d.get("lang") == "en":
            return d.get("value")
    if descriptions:
        return descriptions[0].get("value")
    return None


def _cvss(metrics):
    metrics = metrics or {}
    for key in ("cvssMetricV31", "cvssMetricV30", "cvssMetricV2"):
        entries = metrics.get(key)
        if entries:
            data = entries[0].get("cvssData", {})
            return data.get("baseScore"), data.get("baseSeverity")
    return None, None


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"cve_id": "CVE-2021-44228"},
        }))
        return 0

    cve_id = (q.get("cve_id") or "").strip()
    keyword = (q.get("keyword") or "").strip()

    if not cve_id and not keyword:
        print(json.dumps({
            "error": "provide 'cve_id' or 'keyword'",
            "example": {"cve_id": "CVE-2021-44228"},
        }))
        return 0

    if cve_id:
        url = BASE_URL + "?" + urllib.parse.urlencode({"cveId": cve_id})
    else:
        url = BASE_URL + "?" + urllib.parse.urlencode({
            "keywordSearch": keyword,
            "resultsPerPage": 5,
        })

    try:
        req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
                data = json.loads(resp.read().decode("utf-8", "replace"))
        except urllib.error.HTTPError as e:
            if e.code in (403, 429):
                print(json.dumps({
                    "error": "NVD rate-limited this request — wait ~30s and retry, "
                             "or set NVD_API_KEY for a higher limit",
                    "how_to_fix": "wait about 30 seconds before retrying; unauthenticated "
                                  "NVD requests are limited to roughly 5 per 30s.",
                }))
                return 0
            print(json.dumps({"error": "cve_lookup failed: HTTP %s from NVD" % e.code}))
            return 0

        vulns = data.get("vulnerabilities") or []
        results = []
        for entry in vulns:
            cve = entry.get("cve") or {}
            cve_id_out = cve.get("id")
            score, severity = _cvss(cve.get("metrics"))
            results.append({
                "id": cve_id_out,
                "description": _english_description(cve.get("descriptions")),
                "published": cve.get("published"),
                "cvss_score": score,
                "severity": severity,
                "link": "https://nvd.nist.gov/vuln/detail/" + (cve_id_out or ""),
            })

        result = {"results": results, "result_count": len(results)}
        print(json.dumps(result, indent=2, default=str))
        return 0
    except urllib.error.URLError as e:
        print(json.dumps({
            "error": "cve_lookup failed: network error: %s" % e.reason,
            "how_to_fix": "check connectivity to services.nvd.nist.gov.",
        }))
        return 0
    except Exception as e:
        print(json.dumps({"error": "cve_lookup failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
