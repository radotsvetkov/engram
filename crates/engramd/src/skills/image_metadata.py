#!/usr/bin/env python3
"""image_metadata — Engram skill (no network). Extract width/height/format
from raw image bytes by hand-parsing file headers via stdlib `struct` —
no Pillow/PIL, no imaging library of any kind.

Supports PNG (IHDR chunk), JPEG (scans segments for an SOF marker), and
GIF (fixed little-endian width/height right after the magic bytes).

Request (stdin): {"data_base64": "<base64-encoded image file bytes>"}
Output (stdout): PNG/JPEG/GIF -> {format, width, height, file_size_bytes, [bit_depth, color_type for PNG]}
"""
import base64
import json
import struct
import sys

PNG_MAGIC = b"\x89PNG\r\n\x1a\n"
GIF_MAGICS = (b"GIF87a", b"GIF89a")

# True JPEG start-of-frame markers (excludes 0xC4 DHT, 0xC8 JPG, 0xCC DAC).
SOF_MARKERS = set(
    list(range(0xC0, 0xC4)) + list(range(0xC5, 0xC8)) + list(range(0xC9, 0xCC)) + list(range(0xCD, 0xD0))
)

PNG_COLOR_TYPES = {
    0: "grayscale",
    2: "truecolor",
    3: "indexed",
    4: "grayscale+alpha",
    6: "truecolor+alpha",
}


def _parse_png(data):
    # 8-byte signature, then first chunk: 4-byte length, 4-byte type "IHDR", then data.
    chunk_type = data[12:16]
    if chunk_type != b"IHDR":
        raise ValueError("expected IHDR as first chunk, found %r" % chunk_type)
    width, height = struct.unpack(">II", data[16:24])
    bit_depth = data[24]
    color_type = data[25]
    return {
        "format": "png",
        "width": width,
        "height": height,
        "bit_depth": bit_depth,
        "color_type": PNG_COLOR_TYPES.get(color_type, "unknown (%d)" % color_type),
    }


def _parse_jpeg(data):
    pos = 2  # past the 0xFFD8 SOI marker
    n = len(data)
    while pos < n - 1:
        if data[pos] != 0xFF:
            pos += 1
            continue
        marker = data[pos + 1]
        # Skip fill bytes (0xFF padding before the real marker byte).
        while marker == 0xFF and pos + 2 < n:
            pos += 1
            marker = data[pos + 1]
        if marker == 0xD8:
            pos += 2
            continue
        if marker == 0xD9:  # EOI, no more segments
            break
        if marker == 0x01 or (0xD0 <= marker <= 0xD7):
            # Markers with no length field (TEM, RSTn).
            pos += 2
            continue
        seg_len = struct.unpack(">H", data[pos + 2:pos + 4])[0]
        if marker in SOF_MARKERS:
            payload = data[pos + 4:pos + 4 + seg_len - 2]
            precision = payload[0]
            height, width = struct.unpack(">HH", payload[1:5])
            num_components = payload[5]
            return {
                "format": "jpeg",
                "width": width,
                "height": height,
                "precision_bits": precision,
                "num_components": num_components,
            }
        pos += 2 + seg_len
    raise ValueError("no SOF (start-of-frame) marker found before end of data")


def _parse_gif(data):
    width, height = struct.unpack("<HH", data[6:10])
    return {"format": "gif", "width": width, "height": height}


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"data_base64": "<base64-encoded image bytes>"}}))
        return 0

    data_b64 = q.get("data_base64")
    if not isinstance(data_b64, str) or not data_b64.strip():
        print(json.dumps({
            "error": "provide 'data_base64' (base64-encoded image file bytes)",
            "example": {"data_base64": "<base64-encoded image bytes>"},
        }))
        return 0

    try:
        data = base64.b64decode(data_b64, validate=False)
    except Exception as e:
        print(json.dumps({"error": "invalid base64 in 'data_base64': %s" % e}))
        return 0

    if not data:
        print(json.dumps({"error": "decoded 'data_base64' is empty"}))
        return 0

    try:
        if data.startswith(PNG_MAGIC):
            result = _parse_png(data)
        elif data[:2] == b"\xff\xd8":
            result = _parse_jpeg(data)
        elif data[:6] in GIF_MAGICS:
            result = _parse_gif(data)
        else:
            print(json.dumps({
                "error": "unrecognized image format — supported: PNG, JPEG, GIF",
                "format": "unknown",
            }))
            return 0
    except (IndexError, struct.error, ValueError) as e:
        print(json.dumps({"error": "malformed or truncated image data: %s" % e}))
        return 0
    except Exception as e:
        print(json.dumps({"error": "image parsing failed: %s" % e}))
        return 1

    result["file_size_bytes"] = len(data)
    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
