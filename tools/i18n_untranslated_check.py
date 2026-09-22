#!/usr/bin/env python3
"""i18n-untranslated — the CONTENT half of the locale gate.

WHY THIS EXISTS, AND IT IS THIS REPO'S MOST REPEATED SHAPE IN A NEW PLACE.
`i18n_locale_check.py` reports *"30 locales x 701 keys ... parity, slots, plural
categories, script all clean"* — and every one of those is STRUCTURE. It cannot
see a value that is simply the English string sitting in `ja.json`. Measured
2026-08-23: **51 keys were verbatim English in all 13 non-Latin-script locales**
(the whole Registry Browser window, all of Chat's empty/prompt/placeholder text,
the site-directory verification sublines, the app-host failure messages) while
that line printed "all clean".

Note especially that the existing check's "script" rule is a HOMOGLYPH rule --
no Cyrillic/Greek letters inside a *Latin* locale. It runs in the opposite
direction to this one and gives no coverage here at all. A check that validates
structure reads as a check that validates content, and the gap is invisible for
exactly as long as every string happens to be translated.

WHAT IT CHECKS. For a locale written in a non-Latin script, a translated value
contains at least one character of that script. A value with none is either
untranslated or a technical token. That is a cheap, deterministic proxy and it
is the only one available without a translator in the loop.

WHAT IT CANNOT CHECK, STATED RATHER THAN DISCOVERED: **the 17 Latin-script
locales.** "Status" is legitimately "Status" in Dutch, "Token" is "Token" in
German. There is no mechanical signal separating a good cognate from a skipped
string, so this tool does not look, and a green run says NOTHING about de/nl/fr/
es/... Do not read it as full coverage.

MECHANISM: **an explicit allowlist, and the target is 0.** It shipped as a
baseline count (the ui-lint / i18n-lint idiom) because the backlog was 668 and a
ratchet was the only way to stop it growing. The backlog was then translated --
668 -> 0 outside the list below -- and a count carrying 78 anonymous survivors
is a number nobody can review, so the list is now by KEY WITH A REASON, exactly
as the previous version's docstring said to do once this point was reached.

Two properties this buys over the count, both of which the count could not have:

  * a NEW untranslated string fails immediately, in the locale it landed in --
    no baseline to absorb it, and no way to "ratchet" past it.
  * a STALE exemption fails too. An allowlisted key that is translated
    everywhere (or no longer exists) is reported as unused and must be deleted,
    so the list cannot quietly rot into a second baseline.

  something failed -> translate the key in all 30 locales, or -- if it is
                      genuinely a proper noun -- add it to ALLOWLIST with the
                      reason, in the same commit.
"""

from __future__ import annotations

import json
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
LOCALES = ROOT / "locales"

# Keys whose value is a proper noun and stays in Latin script in every locale.
# Each entry is a reviewable decision, not a tolerated count. Adding one is a
# translation-policy choice; making the list longer to get to green is the
# failure mode it exists to prevent.
ALLOWLIST: dict[str, str] = {
    # `src/i18n.rs` already states this policy where the themes are declared:
    # the descriptive words are translated, the scheme proper nouns name a
    # specific published palette and stay as written, the way a typeface would.
    "theme.nord": "published palette name — a proper noun, like a typeface",
    "theme.dracula": "published palette name — a proper noun, like a typeface",
    "theme.monokai": "published palette name — a proper noun, like a typeface",
    # Web platform API names. Rendered as the identifier a user would search
    # for; transliterating them would make the storage row unsearchable.
    "peerdisplay.storage_opfs": "web API name (Origin Private File System)",
    "peerdisplay.storage_indexeddb": "web API name",
    # "Markdown — {page}": a file-format proper noun plus a slot. There is no
    # prose in it to translate.
    "siteeditor.markdown_label": "file-format proper noun plus a slot",
    # "{window} · {app}": two slots and a separator. Nothing in it is a word.
    "palette.running_title": "two slots and a separator — no text to translate",
}

# Codepoint ranges that count as "this locale's own script". A locale absent
# from this table is Latin-script and is deliberately NOT checked -- see the
# module docstring; a cognate is indistinguishable from a skipped string.
SCRIPTS: dict[str, tuple[tuple[int, int], ...]] = {
    "ar": ((0x0600, 0x06FF),),
    "fa": ((0x0600, 0x06FF),),
    "ur": ((0x0600, 0x06FF),),
    "he": ((0x0590, 0x05FF),),
    "el": ((0x0370, 0x03FF),),
    "ru": ((0x0400, 0x04FF),),
    "uk": ((0x0400, 0x04FF),),
    "hi": ((0x0900, 0x097F),),
    "bn": ((0x0980, 0x09FF),),
    "th": ((0x0E00, 0x0E7F),),
    # Japanese: kana OR han. A kana-free han-only string is legitimate.
    "ja": ((0x3040, 0x30FF), (0x4E00, 0x9FFF)),
    # Korean: precomposed hangul syllables or conjoining jamo.
    "ko": ((0xAC00, 0xD7AF), (0x1100, 0x11FF)),
    "zh": ((0x4E00, 0x9FFF),),
}


def has_native_script(text: str, locale: str) -> bool:
    ranges = SCRIPTS[locale]
    return any(any(lo <= ord(ch) <= hi for lo, hi in ranges) for ch in text)


def values_of(entry: object) -> list[str]:
    """A catalog value is a string, or a plural map of them."""
    if isinstance(entry, str):
        return [entry]
    if isinstance(entry, dict):
        return [v for v in entry.values() if isinstance(v, str)]
    return []


def scan() -> dict[str, list[str]]:
    """locale -> sorted keys whose value carries no character of its script.

    The allowlist is NOT applied here: `main` needs the raw set to tell a live
    exemption from a stale one.
    """
    found: dict[str, list[str]] = {}
    for path in sorted(LOCALES.glob("*.json")):
        locale = path.stem
        if locale not in SCRIPTS:
            continue
        catalog = json.loads(path.read_text(encoding="utf-8"))
        offenders = []
        for key, entry in sorted(catalog.items()):
            text = " ".join(values_of(entry)).strip()
            if text and not has_native_script(text, locale):
                offenders.append(key)
        found[locale] = offenders
    return found


def main() -> int:
    found = scan()
    if not found:
        print("i18n-untranslated: no non-Latin-script locales found", file=sys.stderr)
        return 1

    # A key still carrying English somewhere, that nobody has justified.
    untranslated = {
        loc: [k for k in keys if k not in ALLOWLIST] for loc, keys in found.items()
    }
    untranslated = {loc: keys for loc, keys in untranslated.items() if keys}

    # An exemption nobody needs any more. Left alone, the list becomes the
    # baseline it replaced -- a pile of entries no one rechecks.
    exercised = set().union(*found.values()) if found else set()
    stale = sorted(set(ALLOWLIST) - exercised)

    if untranslated or stale:
        print("i18n-untranslated: FAILED")
        for loc, keys in sorted(untranslated.items()):
            print(f"\n  {loc} — {len(keys)} untranslated:")
            for k in keys:
                print(f"    {k}")
        if stale:
            print("\n  allowlisted but translated everywhere (delete the entry):")
            for k in stale:
                print(f"    {k}  — {ALLOWLIST[k]}")
        if untranslated:
            print(
                "\n  Translate the key in all 30 locales, or — if it is genuinely a"
                "\n  proper noun — add it to ALLOWLIST with its reason, same commit."
            )
        return 1

    print(
        f"i18n-untranslated: OK — every value in the {len(found)} non-Latin-script "
        f"locales carries its own script, except {len(ALLOWLIST)} allowlisted proper "
        f"nouns. NOTE: the 17 Latin-script locales are not checked — a cognate is "
        f"indistinguishable from a skipped string."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
