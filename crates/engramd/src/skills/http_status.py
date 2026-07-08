#!/usr/bin/env python3
"""http_status — Engram skill (no network). Look up an HTTP status code's
reason phrase, category, and a one-line description of when it's used.

Request (stdin): {"code": 404}
Output (stdout): {code, phrase, category, description}
"""
import json
import sys

_CATEGORY_BY_DIGIT = {
    "1": "Informational", "2": "Success", "3": "Redirection",
    "4": "Client Error", "5": "Server Error",
}

_STATUSES = {
    100: ("Continue", "Client should continue sending the request body; used before a large upload."),
    101: ("Switching Protocols", "Server is switching protocols as requested via the Upgrade header."),
    200: ("OK", "Standard success response for a completed request."),
    201: ("Created", "Request succeeded and a new resource was created as a result."),
    202: ("Accepted", "Request has been accepted for processing but is not yet complete."),
    204: ("No Content", "Request succeeded but there is no response body to return."),
    206: ("Partial Content", "Server is returning only part of the resource due to a Range header."),
    301: ("Moved Permanently", "Resource has permanently moved to a new URL given in Location."),
    302: ("Found", "Resource temporarily resides at a different URL."),
    303: ("See Other", "Client should GET a different URL to retrieve the result of the request."),
    304: ("Not Modified", "Cached copy is still valid; no need to retransmit the resource body."),
    307: ("Temporary Redirect", "Resource is temporarily at a different URL; method and body are preserved."),
    308: ("Permanent Redirect", "Resource is permanently at a different URL; method and body are preserved."),
    400: ("Bad Request", "Server cannot process the request due to malformed syntax or invalid data."),
    401: ("Unauthorized", "Authentication is required and either missing or invalid."),
    402: ("Payment Required", "Reserved for future use, e.g. digital payment or metering schemes."),
    403: ("Forbidden", "Server understood the request but refuses to authorize it."),
    404: ("Not Found", "The requested resource could not be found on the server."),
    405: ("Method Not Allowed", "The HTTP method used is not supported for this resource."),
    406: ("Not Acceptable", "Server cannot produce a response matching the request's Accept headers."),
    408: ("Request Timeout", "Server timed out waiting for the rest of the client's request."),
    409: ("Conflict", "Request conflicts with the current state of the target resource."),
    410: ("Gone", "The resource existed but has been permanently removed."),
    411: ("Length Required", "Server requires a Content-Length header that was not supplied."),
    412: ("Precondition Failed", "A precondition in a conditional request header was not met."),
    413: ("Payload Too Large", "Request body is larger than the server is willing to process."),
    414: ("URI Too Long", "The requested URI is longer than the server is willing to interpret."),
    415: ("Unsupported Media Type", "Request body's media type is not supported by the server."),
    416: ("Range Not Satisfiable", "The Range header's requested range cannot be satisfied."),
    417: ("Expectation Failed", "Server cannot meet the requirements of the request's Expect header."),
    418: ("I'm a Teapot", "April Fools' joke status from RFC 2324; not a real production status."),
    422: ("Unprocessable Entity", "Request is well-formed but contains semantic errors (WebDAV)."),
    425: ("Too Early", "Server is unwilling to risk processing a request that might be replayed."),
    426: ("Upgrade Required", "Client should switch to the protocol given in the Upgrade header."),
    428: ("Precondition Required", "Origin server requires the request to be conditional to avoid lost updates."),
    429: ("Too Many Requests", "Client has sent too many requests in a given time window (rate limiting)."),
    431: ("Request Header Fields Too Large", "Header fields are too large for the server to process."),
    451: ("Unavailable For Legal Reasons", "Resource is withheld due to a legal demand, e.g. censorship."),
    500: ("Internal Server Error", "A generic error occurred and no more specific message is available."),
    501: ("Not Implemented", "Server does not support the functionality required to fulfill the request."),
    502: ("Bad Gateway", "A gateway/proxy server received an invalid response from an upstream server."),
    503: ("Service Unavailable", "Server is temporarily unable to handle the request (overload or maintenance)."),
    504: ("Gateway Timeout", "A gateway/proxy server did not get a timely response from upstream."),
    505: ("HTTP Version Not Supported", "Server does not support the HTTP protocol version used in the request."),
    507: ("Insufficient Storage", "Server cannot store the representation needed to complete the request (WebDAV)."),
    511: ("Network Authentication Required", "Client needs to authenticate to gain network access."),
}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0
    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"code": 404},
        })); return 0

    code = q.get("code")
    if code is None:
        print(json.dumps({
            "error": "missing required field 'code' (integer HTTP status code)",
            "example": {"code": 404},
        })); return 0
    try:
        code = int(code)
    except (TypeError, ValueError):
        print(json.dumps({"error": "'code' must be an integer", "example": {"code": 404}})); return 0

    if code < 100 or code > 599:
        print(json.dumps({
            "error": "%d is not a valid HTTP status code (must be 100-599)" % code,
        })); return 0

    try:
        category = _CATEGORY_BY_DIGIT[str(code)[0]]
        if code in _STATUSES:
            phrase, description = _STATUSES[code]
        else:
            phrase, description = (
                "Unknown",
                "Not a commonly registered status code; category inferred from the first digit only.",
            )
        result = {
            "code": code, "phrase": phrase, "category": category, "description": description,
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "http_status failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
