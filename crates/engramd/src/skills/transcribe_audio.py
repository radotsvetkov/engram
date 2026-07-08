#!/usr/bin/env python3
"""transcribe_audio — Engram skill (no network, but requires a local binary).

Transcribes an audio file to text by shelling out to a LOCALLY INSTALLED
speech-to-text CLI. Unlike the keyless, API-based skills elsewhere in this
codebase, this one does NOT work out of the box: it requires OpenAI Whisper
(pip install openai-whisper) or a whisper.cpp build to already be installed
and on PATH. If no such binary is found, this skill reports exactly that and
a how_to_fix, rather than crashing or attempting anything else — the same
pattern email_tool.py uses for the himalaya CLI.

Request (stdin): {"audio_path": "/path/to/file.wav", "model": "base"}
    ("model" is optional, default "base" — a Whisper model size: tiny/base/small/medium/large)
Output (stdout):
    success: {"transcript": "...", "model_used": "base"}
    failure: {"error": "...", "how_to_fix": "..."}  (also on missing/unreadable file, timeout, etc.)
"""
import json
import os
import shutil
import subprocess
import sys
import tempfile

_EXAMPLE = {"audio_path": "/path/to/file.wav", "model": "base"}
_CANDIDATE_BINARIES = ("whisper", "whisper-cli", "main")


def _find_whisper_cli():
    for name in _CANDIDATE_BINARIES:
        if shutil.which(name):
            return name
    return None


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": _EXAMPLE}))
        return 0

    audio_path = q.get("audio_path")
    if not isinstance(audio_path, str) or not audio_path.strip():
        print(json.dumps({"error": "provide 'audio_path' (path to a local audio file)", "example": _EXAMPLE}))
        return 0
    audio_path = audio_path.strip()

    model = q.get("model", "base")
    if not isinstance(model, str) or not model.strip():
        print(json.dumps({"error": "'model' must be a non-empty string if provided", "example": _EXAMPLE}))
        return 0
    model = model.strip()

    # Fail fast on a missing file before even checking for the binary — cheaper.
    if not os.path.exists(audio_path):
        print(json.dumps({"error": "audio file not found: %s" % audio_path}))
        return 0

    binary = _find_whisper_cli()
    if binary is None:
        print(json.dumps({
            "error": "no local speech-to-text CLI found (checked: %s)" % ", ".join(_CANDIDATE_BINARIES),
            "how_to_fix": "install OpenAI Whisper (`pip install openai-whisper`) or whisper.cpp, then retry",
        }))
        return 0

    try:
        out_dir = tempfile.mkdtemp(prefix="engram_transcribe_")
    except Exception as e:
        print(json.dumps({"error": "could not create temp output directory: %s" % e}))
        return 0

    try:
        proc = subprocess.run(
            [binary, audio_path, "--model", model, "--output_format", "txt", "--output_dir", out_dir],
            capture_output=True,
            text=True,
            timeout=600,
        )
    except subprocess.TimeoutExpired:
        print(json.dumps({
            "error": "transcription timed out after 600s",
            "how_to_fix": "the audio file may be too long for the default timeout; try a shorter clip or a smaller/faster model",
        }))
        return 0
    except Exception as e:
        print(json.dumps({"error": "could not run %r: %s" % (binary, e)}))
        return 0

    if proc.returncode != 0:
        print(json.dumps({
            "error": "%s exited with code %d" % (binary, proc.returncode),
            "stderr": (proc.stderr or "").strip()[:2000],
        }))
        return 0

    base_name = os.path.splitext(os.path.basename(audio_path))[0]
    txt_path = os.path.join(out_dir, base_name + ".txt")
    if not os.path.exists(txt_path):
        print(json.dumps({
            "error": "expected transcript file not found at %s" % txt_path,
            "stdout": (proc.stdout or "").strip()[:2000],
            "stderr": (proc.stderr or "").strip()[:2000],
        }))
        return 0

    try:
        with open(txt_path, "r", encoding="utf-8", errors="replace") as f:
            transcript = f.read().strip()
    except Exception as e:
        print(json.dumps({"error": "could not read transcript file: %s" % e}))
        return 0

    print(json.dumps({"transcript": transcript, "model_used": model}, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
