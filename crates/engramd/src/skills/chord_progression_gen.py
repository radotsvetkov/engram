#!/usr/bin/env python3
"""chord_progression_gen — Engram skill (no network). Turn a key + roman-numeral
progression into real chord names using diatonic music theory.

Determines major vs. natural-minor from the key (a trailing lowercase "m",
e.g. "Am", means minor), builds the 7-note diatonic scale for that key
(major: W-W-H-W-W-W-H; natural minor: W-H-W-W-H-W-W), and maps each roman
numeral in the progression to its diatonic chord quality (major key:
I/IV/V major, ii/iii/vi minor, vii° diminished; minor key: i/iv/v minor,
III/VI/VII major, ii° diminished). The chord quality is always the
diatonic default regardless of the case/suffix the user typed; a
disagreement between what the user typed and the diatonic default is
reported in `notes`. Internal computation uses a 12-note sharps-only
chromatic scale (C, C#, D, D#, E, F, F#, G, G#, A, A#, B) — flat spellings
like Bb are accepted as input but chord names are always rendered with
sharps, which is a disclosed simplification, not an auto-selected
flat/sharp key signature.

Request (stdin): {"key": "C", "progression": "I-V-vi-IV"}
Output (stdout): {key, progression, chords: ["C","G","Am","F"], scale_notes: [...]}
"""
import json
import re
import sys

NOTE_NAMES_SHARP = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]

NOTE_TO_INDEX = {
    "C": 0, "C#": 1, "Db": 1, "D": 2, "D#": 3, "Eb": 3, "E": 4, "Fb": 4,
    "E#": 5, "F": 5, "F#": 6, "Gb": 6, "G": 7, "G#": 8, "Ab": 8, "A": 9,
    "A#": 10, "Bb": 10, "B": 11, "Cb": 11, "B#": 0,
}

MAJOR_STEPS = [0, 2, 4, 5, 7, 9, 11]
MINOR_STEPS = [0, 2, 3, 5, 7, 8, 10]

MAJOR_QUALITIES = ["major", "minor", "minor", "major", "major", "minor", "diminished"]
MINOR_QUALITIES = ["minor", "diminished", "major", "minor", "minor", "major", "major"]

ROMAN_MAP = {"i": 1, "ii": 2, "iii": 3, "iv": 4, "v": 5, "vi": 6, "vii": 7}
ROMAN_UPPER_RE = re.compile(r"^(I|II|III|IV|V|VI|VII)$")
ROMAN_LOWER_RE = re.compile(r"^(i|ii|iii|iv|v|vi|vii)$")

KEY_RE = re.compile(r"^([A-G])([#b]?)(m?)$")

QUALITY_SUFFIX = {"major": "", "minor": "m", "diminished": "dim"}

EXAMPLE = {"key": "C", "progression": "I-V-vi-IV"}


def _parse_key(key):
    """Returns (root_index, is_minor, key_display) or raises ValueError."""
    m = KEY_RE.match(key.strip())
    if not m:
        raise ValueError("key %r does not parse to a root note (A-G, optional # or b, optional trailing m)" % key)
    letter, accidental, minor_flag = m.groups()
    note_name = letter + accidental
    if note_name not in NOTE_TO_INDEX:
        raise ValueError("unrecognized note %r" % note_name)
    return NOTE_TO_INDEX[note_name], bool(minor_flag), note_name + ("m" if minor_flag else "")


def _parse_roman_token(token):
    """Returns (degree_index_0based, user_expected_quality_hint) or raises ValueError."""
    token = token.strip()
    dim_suffix = False
    core = token
    lowered = core.lower()
    if core.endswith("°"):
        dim_suffix = True
        core = core[:-1]
    elif lowered.endswith("dim"):
        dim_suffix = True
        core = core[: -3]
    core = core.strip()

    if ROMAN_UPPER_RE.match(core):
        degree = ROMAN_MAP[core.lower()] - 1
        user_hint = "diminished" if dim_suffix else "major"
        return degree, user_hint, token
    if ROMAN_LOWER_RE.match(core):
        degree = ROMAN_MAP[core] - 1
        user_hint = "diminished" if dim_suffix else "minor"
        return degree, user_hint, token
    raise ValueError(
        "invalid roman numeral %r — use I-VII (major case) or i-vii (minor/diminished case), "
        "optionally suffixed with ° or 'dim'" % token
    )


def main():
    try:
        q = json.loads(sys.stdin.read() or "{}")
    except Exception as e:
        print(json.dumps({"error": "invalid JSON: %s" % e}))
        return 0
    if not isinstance(q, dict):
        print(json.dumps({"error": "request must be a JSON object", "example": EXAMPLE}))
        return 0

    key = q.get("key")
    if not isinstance(key, str) or not key.strip():
        print(json.dumps({"error": "provide 'key' (e.g. 'C', 'Am', 'F#')", "example": EXAMPLE}))
        return 0

    progression = q.get("progression", "I-V-vi-IV")
    if not isinstance(progression, str) or not progression.strip():
        print(json.dumps({"error": "'progression' must be a non-empty string like 'I-V-vi-IV'", "example": EXAMPLE}))
        return 0

    try:
        root_index, is_minor, key_display = _parse_key(key)
    except ValueError as e:
        print(json.dumps({"error": str(e), "example": EXAMPLE}))
        return 0

    steps = MINOR_STEPS if is_minor else MAJOR_STEPS
    qualities = MINOR_QUALITIES if is_minor else MAJOR_QUALITIES
    scale_notes = [NOTE_NAMES_SHARP[(root_index + s) % 12] for s in steps]

    tokens = [t for t in progression.split("-") if t.strip() != ""]
    if not tokens:
        print(json.dumps({"error": "'progression' had no roman numerals after splitting on '-'", "example": EXAMPLE}))
        return 0

    chords = []
    notes = []
    try:
        for raw_token in tokens:
            degree, user_hint, original = _parse_roman_token(raw_token)
            actual_quality = qualities[degree]
            chord_root = NOTE_NAMES_SHARP[(root_index + steps[degree]) % 12]
            chord_name = chord_root + QUALITY_SUFFIX[actual_quality]
            chords.append(chord_name)
            if user_hint != actual_quality:
                notes.append(
                    "%r typed as %s but the diatonic default for this scale degree in %s is %s (rendered as %s)"
                    % (original, user_hint, key_display, actual_quality, chord_name)
                )
    except ValueError as e:
        print(json.dumps({
            "error": str(e),
            "example": EXAMPLE,
        }))
        return 0

    result = {
        "key": key_display,
        "progression": progression,
        "chords": chords,
        "scale_notes": scale_notes,
        "is_minor": is_minor,
        "flat_spelling_note": (
            "chord names use sharps only (e.g. 'A#' not 'Bb'); flat-key spelling "
            "preferences are not auto-selected — this is a disclosed simplification"
        ),
    }
    if notes:
        result["notes"] = notes

    print(json.dumps(result, indent=2, default=str))
    return 0


if __name__ == "__main__":
    sys.exit(main())
