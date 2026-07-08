#!/usr/bin/env python3
"""ssl_cert_check — Engram skill (network). Check a host's TLS certificate.

Opens a real TLS connection (with full certificate verification) and reports
the certificate's validity window, issuer, and subject. Verification failures
(self-signed, expired, hostname mismatch) and connection errors are reported
as clear errors rather than crashes.

Request (stdin): {"host": "example.com", "port": 443}
Output (stdout): {host, port, issuer, subject, not_before, not_after,
                  days_until_expiry, is_expired, expiring_soon}
"""
import datetime
import json
import socket
import ssl
import sys
import urllib.parse

TIMEOUT = 15
DEFAULT_PORT = 443


def _clean_host(host):
    host = host.strip()
    if "://" in host or "/" in host:
        parsed = urllib.parse.urlsplit(host if "://" in host else "//" + host)
        return parsed.hostname or host
    return host


def _parse_cert_time(s):
    # e.g. "Jun  1 12:00:00 2027 GMT"
    s = s.strip()
    if s.endswith(" GMT"):
        s = s[:-4]
    return datetime.datetime.strptime(s, "%b %d %H:%M:%S %Y")


def _flatten(name_tuples):
    flat = {}
    for rdn in name_tuples or ():
        for key, value in rdn:
            flat[key] = value
    return flat


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"host": "example.com", "port": 443}}))
        return 0

    raw_host = q.get("host")
    if not raw_host:
        print(json.dumps({"error": "provide a 'host'",
                          "example": {"host": "example.com", "port": 443}}))
        return 0

    host = _clean_host(str(raw_host))
    if not host:
        print(json.dumps({"error": "could not determine hostname from input",
                          "example": {"host": "example.com", "port": 443}}))
        return 0

    port = q.get("port", DEFAULT_PORT)
    try:
        port = int(port)
    except (TypeError, ValueError):
        print(json.dumps({"error": "'port' must be an integer",
                          "example": {"host": "example.com", "port": 443}}))
        return 0

    try:
        ctx = ssl.create_default_context()
        try:
            with socket.create_connection((host, port), timeout=TIMEOUT) as sock:
                with ctx.wrap_socket(sock, server_hostname=host) as ssock:
                    cert = ssock.getpeercert()
        except ssl.SSLCertVerificationError as e:
            print(json.dumps({
                "error": "TLS verification failed: %s" % e,
                "host": host,
                "port": port,
            }))
            return 0
        except (socket.timeout, TimeoutError):
            print(json.dumps({
                "error": "connection to %s:%s timed out" % (host, port),
                "host": host,
                "port": port,
            }))
            return 0
        except ConnectionRefusedError:
            print(json.dumps({
                "error": "connection to %s:%s was refused" % (host, port),
                "host": host,
                "port": port,
            }))
            return 0
        except socket.gaierror as e:
            print(json.dumps({
                "error": "could not resolve host %s: %s" % (host, e),
                "host": host,
                "port": port,
            }))
            return 0
        except OSError as e:
            print(json.dumps({
                "error": "connection to %s:%s failed: %s" % (host, port, e),
                "host": host,
                "port": port,
            }))
            return 0

        not_before = _parse_cert_time(cert["notBefore"])
        not_after = _parse_cert_time(cert["notAfter"])
        days_until_expiry = (not_after - datetime.datetime.utcnow()).days

        result = {
            "host": host,
            "port": port,
            "issuer": _flatten(cert.get("issuer")),
            "subject": _flatten(cert.get("subject")),
            "not_before": not_before.isoformat(),
            "not_after": not_after.isoformat(),
            "days_until_expiry": days_until_expiry,
            "is_expired": days_until_expiry < 0,
            "expiring_soon": 0 <= days_until_expiry <= 30,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "ssl_cert_check failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
