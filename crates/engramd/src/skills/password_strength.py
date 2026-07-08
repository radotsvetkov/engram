#!/usr/bin/env python3
"""password_strength — Engram skill (no network). Estimate password strength.

Computes a charset-size entropy estimate (bits = length * log2(pool_size)),
flags common/breached-list passwords, sequential runs ("abc", "321") and
repeated characters ("aaa"). The raw password is NEVER echoed back — only
its length is included in the output.

Request (stdin): {"password": "hunter2"}
Output (stdout): {password_length, bits_of_entropy, strength, is_common_password,
                  has_sequential_chars, has_repeated_chars, suggestions}
"""
import json
import math
import re
import sys

_COMMON_PASSWORDS = {
    "password", "123456", "123456789", "qwerty", "abc123", "letmein", "admin",
    "welcome", "monkey", "dragon", "football", "iloveyou", "password1",
    "111111", "123123", "master", "login", "starwars", "hello", "freedom",
    "whatever", "trustno1", "superman", "batman", "000000", "1234", "12345",
    "1q2w3e4r", "zaq1zaq1", "qwerty123", "passw0rd", "123321", "666666",
    "1qaz2wsx", "sunshine", "princess", "shadow", "michael", "jennifer",
    "654321", "121212", "qazwsx", "baseball", "solo", "flower", "hottie",
    "loveme", "hockey",
}

_SYMBOLS = set("!@#$%^&*()-_=+[]{}|;:'\",.<>/?`~\\")


def _pool_size(password):
    size = 0
    if any(c.islower() for c in password):
        size += 26
    if any(c.isupper() for c in password):
        size += 26
    if any(c.isdigit() for c in password):
        size += 10
    if any(c in _SYMBOLS for c in password):
        size += 32
    return size


def _has_sequential_chars(password):
    # any 3-char window that is a monotonic ascending or descending run of
    # consecutive ascii codes, e.g. "abc", "321", "xyz"
    for i in range(len(password) - 2):
        a, b, c = (ord(ch) for ch in password[i:i + 3])
        if (b - a == 1 and c - b == 1) or (a - b == 1 and b - c == 1):
            return True
    return False


def _has_repeated_chars(password):
    return re.search(r"(.)\1{2,}", password) is not None


def _strength(bits, is_common):
    if is_common:
        return "very weak"
    if bits < 28:
        return "very weak"
    if bits < 36:
        return "weak"
    if bits < 60:
        return "fair"
    if bits < 128:
        return "strong"
    return "very strong"


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"password": "hunter2"}}))
        return 0

    password = q.get("password")
    if password is None:
        print(json.dumps({"error": "provide a password",
                          "example": {"password": "hunter2"}}))
        return 0

    if not isinstance(password, str):
        password = str(password)

    try:
        pool_size = _pool_size(password)
        bits = 0.0 if pool_size <= 1 else len(password) * math.log2(pool_size)
        is_common = password.lower() in _COMMON_PASSWORDS
        seq = _has_sequential_chars(password)
        rep = _has_repeated_chars(password)
        strength = _strength(bits, is_common)

        suggestions = []
        if strength not in ("strong", "very strong"):
            if is_common:
                suggestions.append("avoid common passwords")
            if pool_size < 68:
                suggestions.append("mix in symbols and numbers")
            if seq or rep:
                suggestions.append("avoid repeated/sequential characters")
            if len(password) < 12:
                suggestions.append("aim for 12+ characters")
            if not suggestions:
                suggestions.append("use a longer, more random passphrase")

        result = {
            "password_length": len(password),
            "bits_of_entropy": round(bits, 1),
            "strength": strength,
            "is_common_password": is_common,
            "has_sequential_chars": seq,
            "has_repeated_chars": rep,
            "suggestions": suggestions,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "password_strength failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
