#!/usr/bin/env python3
"""brand_voice_checklist — Engram skill (no network).

Screens a piece of copy against brand-voice hygiene checks: your own
`banned_words` (style-guide forbidden phrases, case-insensitive substring
match), a built-in list of ~15 common corporate buzzwords, a rough passive-
voice heuristic (regex-based, approximate — not a real grammar parser), and
average sentence length (flagged as dense/corporate only if you asked for a
"concise" or "friendly" tone). Stdlib only.

Request (stdin): {"text": str, "tone_attributes"?: [str], "banned_words"?: [str]}
Output (stdout): {banned_word_hits, buzzword_hits, avg_sentence_length_words,
  passive_voice_flag_count, overall_note}
"""
import json
import re
import sys

_BUZZWORDS = [
    "synergy", "leverage", "disrupt", "paradigm shift", "best-in-class",
    "circle back", "touch base", "low-hanging fruit", "move the needle",
    "bandwidth", "deep dive", "game-changer", "ecosystem", "holistic", "seamless",
]

_PASSIVE_RE = re.compile(r"\b(is|was|were|been)\s+\w*ed\b", re.IGNORECASE)
_SENTENCE_SPLIT_RE = re.compile(r"(?<=[.!?])\s+")
_WORD_RE = re.compile(r"[A-Za-z']+")

_SENTENCE_LENGTH_FLAG_THRESHOLD = 25


def _count_hits(text_lower, phrase):
    phrase_lower = phrase.lower()
    if not phrase_lower:
        return 0
    return text_lower.count(phrase_lower)


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0

    example = {
        "text": "We will leverage our best-in-class ecosystem to move the needle.",
        "tone_attributes": ["friendly", "confident", "concise"],
        "banned_words": ["synergy", "disrupt"],
    }

    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": example}))
        return 0

    text = q.get("text")
    if not isinstance(text, str) or not text.strip():
        print(json.dumps({
            "error": "missing required field 'text' (non-empty string)",
            "example": example,
        }))
        return 0

    tone_attributes = q.get("tone_attributes", [])
    if not isinstance(tone_attributes, list) or not all(isinstance(x, str) for x in tone_attributes):
        print(json.dumps({
            "error": "'tone_attributes' must be a list of strings if provided",
            "example": example,
        }))
        return 0

    banned_words = q.get("banned_words", [])
    if not isinstance(banned_words, list) or not all(isinstance(x, str) for x in banned_words):
        print(json.dumps({
            "error": "'banned_words' must be a list of strings if provided",
            "example": example,
        }))
        return 0

    try:
        text_lower = text.lower()
        tone_lower = set(t.strip().lower() for t in tone_attributes if t.strip())

        banned_word_hits = []
        for phrase in banned_words:
            phrase = phrase.strip()
            if not phrase:
                continue
            count = _count_hits(text_lower, phrase)
            if count:
                banned_word_hits.append({"word": phrase, "count": count})

        buzzword_hits = []
        for phrase in _BUZZWORDS:
            count = _count_hits(text_lower, phrase)
            if count:
                buzzword_hits.append({"word": phrase, "count": count})

        sentences = [s.strip() for s in _SENTENCE_SPLIT_RE.split(text.strip()) if s.strip()]
        if not sentences:
            sentences = [text.strip()]
        total_words = sum(len(_WORD_RE.findall(s)) for s in sentences)
        avg_sentence_length_words = round(total_words / len(sentences), 1) if sentences else 0.0

        passive_voice_flag_count = len(_PASSIVE_RE.findall(text))

        wants_concise = bool({"concise", "friendly"} & tone_lower)
        sentence_length_flagged = wants_concise and avg_sentence_length_words > _SENTENCE_LENGTH_FLAG_THRESHOLD

        issues = []
        if banned_word_hits:
            issues.append(
                "contains %d banned word(s)/phrase(s) from your style guide: %s"
                % (len(banned_word_hits), ", ".join(h["word"] for h in banned_word_hits))
            )
        if buzzword_hits:
            issues.append(
                "contains %d corporate buzzword(s): %s"
                % (len(buzzword_hits), ", ".join(h["word"] for h in buzzword_hits))
            )
        if sentence_length_flagged:
            issues.append(
                "average sentence length is %.1f words (> %d) — may read as dense/corporate "
                "given your requested tone" % (avg_sentence_length_words, _SENTENCE_LENGTH_FLAG_THRESHOLD)
            )
        if passive_voice_flag_count >= 2:
            issues.append(
                "%d likely passive-voice construction(s) detected (approximate heuristic)"
                % passive_voice_flag_count
            )

        if issues:
            overall_note = "Biggest issue: %s." % issues[0]
            if len(issues) > 1:
                overall_note += " Also flagged: %s." % "; ".join(issues[1:])
        else:
            overall_note = (
                "Text looks reasonably clean — no banned words, buzzwords, notably long "
                "sentences, or heavy passive voice detected."
            )

        result = {
            "banned_word_hits": banned_word_hits,
            "buzzword_hits": buzzword_hits,
            "avg_sentence_length_words": avg_sentence_length_words,
            "passive_voice_flag_count": passive_voice_flag_count,
            "overall_note": overall_note,
        }
        print(json.dumps(result, indent=2, default=str))
        return 0
    except Exception as e:
        print(json.dumps({"error": "brand_voice_checklist failed: %s" % e}))
        return 1


if __name__ == "__main__":
    sys.exit(main())
