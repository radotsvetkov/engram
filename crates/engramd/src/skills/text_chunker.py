#!/usr/bin/env python3
"""text_chunker — Engram skill (no network). Split text into overlapping chunks for RAG.

Sliding window over words (default) or characters, stepping by (chunk_size - overlap).
In char mode it prefers to break at whitespace within the last ~20% of a chunk to avoid
mid-word cuts (best-effort). Per-chunk token_estimate = word_count*1.3 (rounded).

Request (stdin): {"text": "...", "chunk_size": 512, "overlap": 64, "unit": "words"}
Output (stdout): {chunk_count, unit, chunk_size, overlap, chunks:[{index,text,start,end,token_estimate}]}
"""
import json, sys


def _tok(s):
    return round(len(s.split()) * 1.3)


def _chunk_words(words, size, step):
    chunks = []
    n = len(words)
    i = 0
    idx = 0
    while i < n:
        window = words[i:i + size]
        text = " ".join(window)
        chunks.append({
            "index": idx, "text": text, "start": i, "end": i + len(window),
            "token_estimate": _tok(text),
        })
        idx += 1
        if i + size >= n:
            break
        i += step
    return chunks


def _chunk_chars(text, size, overlap):
    chunks = []
    n = len(text)
    i = 0
    idx = 0
    while i < n:
        end = min(i + size, n)
        if end < n:
            # best-effort: break at whitespace within the last ~20% of the window
            window_len = end - i
            lookback_start = i + int(window_len * 0.8)
            brk = max(
                text.rfind(" ", lookback_start, end),
                text.rfind("\n", lookback_start, end),
            )
            if brk > i:
                end = brk
        piece = text[i:end]
        chunks.append({
            "index": idx, "text": piece, "start": i, "end": end,
            "token_estimate": _tok(piece),
        })
        idx += 1
        if end >= n:
            break
        i = max(i + 1, end - overlap)
    return chunks


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON request: %s" % e})); return 0

    if not isinstance(q, dict):
        print(json.dumps({
            "error": "request must be a JSON object",
            "example": {"text": "long text ...", "chunk_size": 512, "overlap": 64, "unit": "words"},
        })); return 0

    text = q.get("text")
    if text is None or not isinstance(text, str):
        print(json.dumps({
            "error": "missing required field 'text' (string)",
            "example": {"text": "long text ...", "chunk_size": 200, "overlap": 20, "unit": "words"},
        })); return 0

    try:
        chunk_size = q.get("chunk_size", 512)
        overlap = q.get("overlap", 64)
        unit = q.get("unit", "words")
        if not isinstance(chunk_size, int) or isinstance(chunk_size, bool) or chunk_size < 1:
            print(json.dumps({"error": "'chunk_size' must be a positive integer"})); return 0
        if not isinstance(overlap, int) or isinstance(overlap, bool) or overlap < 0:
            print(json.dumps({"error": "'overlap' must be a non-negative integer"})); return 0
        if overlap >= chunk_size:
            print(json.dumps({
                "error": "'overlap' (%d) must be less than 'chunk_size' (%d)" % (overlap, chunk_size),
            })); return 0
        if unit not in ("words", "chars"):
            print(json.dumps({
                "error": "'unit' must be 'words' or 'chars'",
                "example": {"unit": "words"},
            })); return 0

        if unit == "words":
            words = text.split()
            chunks = _chunk_words(words, chunk_size, chunk_size - overlap)
        else:
            chunks = _chunk_chars(text, chunk_size, overlap)

        result = {
            "chunk_count": len(chunks),
            "unit": unit,
            "chunk_size": chunk_size,
            "overlap": overlap,
            "chunks": chunks,
        }
        print(json.dumps(result, indent=2, default=str)); return 0
    except Exception as e:
        print(json.dumps({"error": "text_chunker failed: %s" % e})); return 1


if __name__ == "__main__":
    sys.exit(main())
