#!/usr/bin/env python3
"""video_metadata — Engram skill (no network). Extract container/dimensions/
duration from raw video file bytes by hand-parsing the ISO Base Media File
Format (used by both MP4 and QuickTime/MOV) via stdlib `struct` — no ffmpeg,
no moviepy, no third-party media library of any kind.

Walks the top-level box structure (4-byte big-endian size, 4-byte ASCII
type, then payload; size 0 means "rest of file", size 1 means the real
size follows as an 8-byte big-endian "largesize" field), finds `ftyp` for
the container brand, recurses into `moov` to find `mvhd` (timescale +
duration, versioned box) and into `moov/trak/tkhd` (versioned box, with
32.16 fixed-point width/height near the end of the payload) for the first
video track (an audio-only track typically has width/height == 0, so those
tracks are skipped).

Request (stdin): {"data_base64": "<base64-encoded video file bytes>"}
Output (stdout): {format: "mp4_or_mov", container_brand, width, height,
                  duration_seconds, file_size_bytes}
                 or {"error": ..., "format": "unknown"} if unsupported/malformed.
"""
import base64
import json
import struct
import sys

BOX_HEADER_MIN = 8  # 4-byte size + 4-byte type


def _iter_boxes(data, start, end):
    """Yield (box_type, payload_start, payload_end) for each top-level box in data[start:end]."""
    pos = start
    while pos + BOX_HEADER_MIN <= end:
        size = struct.unpack(">I", data[pos:pos + 4])[0]
        box_type = data[pos + 4:pos + 8]
        header_len = 8
        if size == 1:
            # 64-bit "largesize" follows the type field.
            if pos + 16 > end:
                break
            size = struct.unpack(">Q", data[pos + 8:pos + 16])[0]
            header_len = 16
        elif size == 0:
            # Box extends to the end of the enclosing container.
            size = end - pos
        if size < header_len:
            break
        box_end = pos + size
        if box_end > end:
            box_end = end
        yield box_type, pos + header_len, box_end
        if size == 0:
            break
        pos = box_end


def _find_box(data, start, end, wanted):
    for box_type, p_start, p_end in _iter_boxes(data, start, end):
        if box_type == wanted:
            return p_start, p_end
    return None


def _parse_mvhd(payload):
    version = payload[0]
    if version == 1:
        # version(1) + flags(3) + creation(8) + modification(8) + timescale(4) + duration(8)
        timescale = struct.unpack(">I", payload[20:24])[0]
        duration = struct.unpack(">Q", payload[24:32])[0]
    else:
        # version(1) + flags(3) + creation(4) + modification(4) + timescale(4) + duration(4)
        timescale = struct.unpack(">I", payload[12:16])[0]
        duration = struct.unpack(">I", payload[16:20])[0]
    if timescale == 0:
        raise ValueError("mvhd timescale is zero")
    return duration / timescale


def _parse_tkhd_dimensions(payload):
    version = payload[0]
    if version == 1:
        # version(1)+flags(3)+creation(8)+modification(8)+track_id(4)+reserved(4)
        # +duration(8)+reserved(8)+layer(2)+alt_group(2)+volume(2)+reserved(2)
        # +matrix(36)+width(4)+height(4)
        base = 4 + 8 + 8 + 4 + 4 + 8 + 8 + 2 + 2 + 2 + 2 + 36
    else:
        # version(1)+flags(3)+creation(4)+modification(4)+track_id(4)+reserved(4)
        # +duration(4)+reserved(8)+layer(2)+alt_group(2)+volume(2)+reserved(2)
        # +matrix(36)+width(4)+height(4)
        base = 4 + 4 + 4 + 4 + 4 + 4 + 8 + 2 + 2 + 2 + 2 + 36
    width_fixed, height_fixed = struct.unpack(">II", payload[base:base + 8])
    return width_fixed / 65536.0, height_fixed / 65536.0


def _find_first_video_track_dims(data, moov_start, moov_end):
    for box_type, p_start, p_end in _iter_boxes(data, moov_start, moov_end):
        if box_type != b"trak":
            continue
        tkhd = _find_box(data, p_start, p_end, b"tkhd")
        if tkhd is None:
            continue
        t_start, t_end = tkhd
        width, height = _parse_tkhd_dimensions(data[t_start:t_end])
        if width > 0 and height > 0:
            return width, height
    return None


def _parse_video(data):
    n = len(data)
    ftyp = _find_box(data, 0, min(n, 4096), b"ftyp")
    if ftyp is None:
        # Not necessarily fatal (some MOVs lead with other boxes), but if we
        # cannot even walk a single valid box near the start, bail out clearly.
        found_any = False
        for box_type, _, _ in _iter_boxes(data, 0, min(n, 64)):
            found_any = True
            break
        if not found_any:
            raise ValueError("no recognizable ISO BMFF box found in first 64 bytes")

    container_brand = None
    if ftyp is not None:
        f_start, f_end = ftyp
        brand_bytes = data[f_start:f_start + 4]
        try:
            container_brand = brand_bytes.decode("ascii")
        except Exception:
            container_brand = repr(brand_bytes)

    moov = _find_box(data, 0, n, b"moov")
    if moov is None:
        raise ValueError("no 'moov' box found — cannot read movie header")
    moov_start, moov_end = moov

    mvhd = _find_box(data, moov_start, moov_end, b"mvhd")
    if mvhd is None:
        raise ValueError("no 'mvhd' box found inside 'moov'")
    mvhd_start, mvhd_end = mvhd
    duration_seconds = _parse_mvhd(data[mvhd_start:mvhd_end])

    dims = _find_first_video_track_dims(data, moov_start, moov_end)
    if dims is None:
        raise ValueError("no video track with non-zero width/height found in 'moov'")
    width, height = dims

    return {
        "format": "mp4_or_mov",
        "container_brand": container_brand,
        "width": int(round(width)),
        "height": int(round(height)),
        "duration_seconds": duration_seconds,
    }


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object",
                          "example": {"data_base64": "<base64-encoded video bytes>"}}))
        return 0

    data_b64 = q.get("data_base64")
    if not isinstance(data_b64, str) or not data_b64.strip():
        print(json.dumps({
            "error": "provide 'data_base64' (base64-encoded video file bytes)",
            "example": {"data_base64": "<base64-encoded video bytes>"},
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
        result = _parse_video(data)
    except (IndexError, struct.error, ValueError) as e:
        print(json.dumps({
            "error": "unrecognized or unsupported video container — only MP4/MOV (ISO BMFF) "
                     "are supported without ffmpeg (%s)" % e,
            "format": "unknown",
        }))
        return 0
    except Exception as e:
        print(json.dumps({"error": "video parsing failed unexpectedly: %s" % e, "format": "unknown"}))
        return 1

    result["file_size_bytes"] = len(data)
    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
