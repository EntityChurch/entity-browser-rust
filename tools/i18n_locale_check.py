#!/usr/bin/env python3
"""Structural validation of the 30 overlay catalogs.

The prose scanner answers "is anything still hardcoded in Rust". This answers
the other half — "are the catalogs themselves well-formed" — which nothing in
the build checked before, so each extraction commit re-verified it by hand.

Four invariants, all of which have been violated at least once in this
project's history:

  1. PARITY      — every overlay carries exactly the EN key set. A missing key
                   silently falls back to English for that locale only.
  2. SLOTS       — a translation preserves its message's `{slot}` set. A
                   dropped slot renders a sentence with a hole; an invented one
                   renders a literal `{foo}`.
  3. PLURALS     — a key that is a plural in EN is a plural everywhere, and the
                   category set matches that locale's own `peer.count`.
  4. HOMOGLYPHS  — no Cyrillic/Greek letters inside a Latin-script locale.
                   This catches the classic paste typo: `paritа` with a
                   Cyrillic `а` looks identical to `parita` and breaks nothing
                   visibly, but it is not the word. Two were found in `fi` the
                   first time this ran — one of them months old.
  5. WHITESPACE  — no leading/trailing or doubled space that EN doesn't have.
                   `he/accesslog.actor_system_backend` shipped as
                   `" backend מערכת"`: a leading space, and the one key in that
                   locale that left "backend" in English.
  6. CONSISTENCY — one EN string that several keys share renders the same way
                   within a locale, unless CONSISTENCY_OK says why not. Six
                   locales rendered "System backend" two different ways because
                   two translation passes chose different terms; the same split
                   hit "Peer ID", "Loading…", "System peer". Divergence is
                   sometimes correct (grammatical gender, a proper noun vs an
                   adjective) — hence an allowlist that records the reason
                   rather than a rule that bans it.

The reference for all three structural checks is the **EN base itself**, parsed
out of `src/i18n.rs`. It used to be `es`, which made the gate self-referential:
it proved the overlays agreed with each other, never that they matched the
source of truth. A key added to `EN` and to no overlay was invisible — `es`
lacked it too, so the reference lacked it, so nothing was missing. Every other
guard tolerates that by design: `build.rs` and `overlay_is_subset_of_en` catch
*orphans* (an overlay key not in EN), and `overlay_missing_key_falls_back_to_en`
deliberately permits *absences* ("a missing string is a safe en fallback").
Absence was therefore ungated end to end. It only ever stayed clean because each
extraction commit updated `es` by hand in the same pass, so the reference always
agreed with itself. Verified by simulation: adding one EN key left the old gate
reporting "OK — 512 keys, parity clean" with all 30 locales short a string.

Exit non-zero on any violation. `python3 tools/i18n_locale_check.py`.
"""
import glob
import json
import os
import re
import sys

LOCALES_DIR = 'locales'
EN_SRC = os.path.join('src', 'i18n.rs')
EN_ANCHOR = 'pub const EN: &[(&str, Message)] = &['

# Overlays that are deliberately partial, mapping locale id -> reason. The
# README's "partial is fine" policy stays expressible: list a locale here and
# its missing keys become an accepted en fallback rather than an error. Empty
# today — all 30 overlays are declared complete, and that is what we ratchet.
PARTIAL_OK = {}

# (locale, EN string) -> why two keys sharing that EN string legitimately
# render differently in this locale. Everything not listed here must agree.
# A reason of "PENDING" flags a real divergence that needs a native speaker —
# it is held at today's value so it cannot drift further, not blessed.
CONSISTENCY_OK = {
    ('el', 'All'):
        'gender agreement — sites are masculine (Όλοι), log entries neuter (Όλα)',
    ('el', 'Light'):
        'theme name (Ανοιχτό) vs. the render-mode adjective (Φωτεινό)',
    ('ko', 'Light'):
        'theme name transliterated as a proper noun (라이트) vs. the '
        'render-mode adjective (밝은) — the documented theme-name policy',
    ('ko', 'Dark'):
        'theme name transliterated as a proper noun (다크) vs. the '
        'render-mode adjective (어두운) — the documented theme-name policy',
    ('ar', 'Device'):
        'indefinite (جهاز) as a field label vs. definite (الجهاز) mid-sentence',
    ('id', 'Stop'):
        'transitive (Hentikan — stop a peer) vs. intransitive '
        '(Berhenti — stop scanning)',
    ('ur', 'All'):
        'PENDING native review — سب and تمام both render "all"; pick one',
    ('cs', 'Peer ID'):
        'PENDING native review — genitive of the loanword "peer" differs '
        '(peera/peeru) by whether it is treated as animate',
}

# Locales written in Latin script — a Cyrillic or Greek letter here is a typo,
# never intentional. (Greek/Cyrillic-script locales are excluded by omission.)
LATIN_SCRIPT = {
    'cs', 'da', 'de', 'es', 'fi', 'fr', 'hu', 'id', 'it', 'nl', 'no', 'pl',
    'pt', 'ro', 'sv', 'tr', 'vi',
}
CYRILLIC_OR_GREEK = re.compile(r'[Ͱ-ϿЀ-ӿ]')
SLOT = re.compile(r'\{(\w+)\}')


def values(msg):
    """Every string in a message — one for Simple, N for a plural."""
    return [msg] if isinstance(msg, str) else list(msg.values())


def _elements(src):
    """Yield the raw text of each top-level entry of the `EN` array.

    A hand-rolled scan rather than a regex: entries span multiple lines, nest
    parens (`Message::Plural(&[(PluralCategory::One, "…")])`), and carry
    comments and escaped quotes. A regex over `("key",` silently mis-parsed
    both the multi-line entries and the trailing comma of a wrapped
    `Message::Simple(\\n "…",\\n)` value.
    """
    i = src.index(EN_ANCHOR) + len(EN_ANCHOR)
    depth, start = 1, i
    while i < len(src):
        c = src[i]
        if c == '/' and src[i:i + 2] == '//':
            i = src.index('\n', i)
            continue
        if c == '/' and src[i:i + 2] == '/*':
            i = src.index('*/', i) + 2
            continue
        if c == '"':
            i += 1
            while src[i] != '"':
                i += 2 if src[i] == '\\' else 1
            i += 1
            continue
        if c in '([{':
            depth += 1
        elif c in ')]}':
            depth -= 1
            if depth == 0:
                tail = src[start:i].strip()
                if tail:
                    yield tail
                return
        elif c == ',' and depth == 1:
            yield src[start:i]
            start = i + 1
        i += 1
    raise SystemExit(f'i18n-locale-check: unterminated EN array in {EN_SRC}')


def _literals(text):
    """Every Rust string literal in `text`, unescaped enough for our checks."""
    out, i = [], 0
    while i < len(text):
        if text[i] == '/' and text[i:i + 2] == '//':
            i = text.index('\n', i) if '\n' in text[i:] else len(text)
            continue
        if text[i] != '"':
            i += 1
            continue
        i += 1
        buf = []
        while text[i] != '"':
            if text[i] == '\\':
                nxt = text[i + 1]
                # A `\` before a newline is Rust's line continuation: the
                # literal continues with leading whitespace stripped.
                if nxt == '\n':
                    i += 2
                    while text[i] in ' \t':
                        i += 1
                    continue
                # `\u{25be}` — must become the real character, not `u{25be}`,
                # which the `{slot}` regex would otherwise read as a slot named
                # `25be` and report against every locale at once.
                if nxt == 'u' and text[i + 2] == '{':
                    end = text.index('}', i)
                    buf.append(chr(int(text[i + 3:end], 16)))
                    i = end + 1
                    continue
                buf.append({'n': '\n', 't': '\t', '0': '\0'}.get(nxt, nxt))
                i += 2
                continue
            buf.append(text[i])
            i += 1
        out.append(''.join(buf))
        i += 1
    return out


def parse_en(path=EN_SRC):
    """The EN base as {key: str | {category: str}} — the reference catalog.

    Mirrors the JSON overlay shape exactly, so the same checks run against it.
    """
    src = open(path, encoding='utf-8').read()
    if EN_ANCHOR not in src:
        sys.exit(f'i18n-locale-check: could not find the EN base in {path}')
    en = {}
    for elem in _elements(src):
        lits = _literals(elem)
        if not lits:
            continue
        key, rest = lits[0], lits[1:]
        if 'Message::Plural' in elem:
            cats = [c.lower() for c in re.findall(r'PluralCategory::(\w+)', elem)]
            if len(cats) != len(rest):
                sys.exit(f'i18n-locale-check: cannot parse plural {key!r} in {path}')
            en[key] = dict(zip(cats, rest))
        else:
            if len(rest) != 1:
                sys.exit(f'i18n-locale-check: cannot parse message {key!r} in {path}')
            en[key] = rest[0]
    return en


def main():
    paths = sorted(glob.glob(os.path.join(LOCALES_DIR, '*.json')))
    if not paths:
        sys.exit('i18n-locale-check: no locale files found')
    cats = {os.path.basename(p)[:-5]: json.load(open(p, encoding='utf-8'))
            for p in paths}

    errors = []
    en = parse_en()
    ref_keys = set(en)

    for loc, cat in sorted(cats.items()):
        # 1. parity — against EN, the source of truth, not a sibling overlay.
        missing, extra = ref_keys - set(cat), set(cat) - ref_keys
        if missing and loc not in PARTIAL_OK:
            errors.append(f'{loc}: missing {len(missing)} key(s) present in the '
                          f'EN base: {sorted(missing)[:8]}')
        if extra:
            errors.append(f'{loc}: unexpected {len(extra)} key(s) not in the '
                          f'EN base: {sorted(extra)[:8]}')

        # 3. plural shape + category set
        plural_cats = set(cat['peer.count']) if isinstance(cat.get('peer.count'), dict) else None
        for key, msg in sorted(cat.items()):
            ref = en.get(key)
            if ref is None:
                continue
            if isinstance(ref, dict) != isinstance(msg, dict):
                errors.append(f'{loc}/{key}: plural-vs-simple shape differs from the EN base')
            elif isinstance(msg, dict) and plural_cats and set(msg) != plural_cats:
                errors.append(
                    f'{loc}/{key}: plural categories {sorted(msg)} != '
                    f"this locale's peer.count {sorted(plural_cats)}")

            # 2. slots
            ref_slots = set()
            for v in values(ref):
                ref_slots |= set(SLOT.findall(v))
            for v in values(msg):
                got = set(SLOT.findall(v))
                if got != ref_slots:
                    errors.append(
                        f'{loc}/{key}: slots {sorted(got)} != {sorted(ref_slots)}')
                    break

            # 5. whitespace
            for v in values(msg):
                en_v = values(ref)[0]
                if v != v.strip() and en_v == en_v.strip():
                    errors.append(
                        f'{loc}/{key}: leading/trailing whitespace the EN '
                        f'template does not have: {v!r}')
                    break
                if '  ' in v and '  ' not in en_v:
                    errors.append(f'{loc}/{key}: doubled space: {v!r}')
                    break

            # 4. homoglyphs
            if loc in LATIN_SCRIPT:
                for v in values(msg):
                    hit = CYRILLIC_OR_GREEK.search(v)
                    if hit:
                        tok = next((t for t in v.split() if CYRILLIC_OR_GREEK.search(t)), v)
                        errors.append(
                            f'{loc}/{key}: non-Latin letter U+{ord(hit.group()):04X} '
                            f'in Latin-script locale — homoglyph typo in {tok!r}')
                        break

    # 6. consistency — EN strings shared by >1 key must render alike in-locale.
    shared = {}
    for key, msg in en.items():
        if isinstance(msg, str):
            shared.setdefault(msg, []).append(key)
    shared = {v: ks for v, ks in shared.items() if len(ks) > 1}
    for loc, cat in sorted(cats.items()):
        for en_val, keys in sorted(shared.items()):
            seen = {}
            for key in keys:
                if isinstance(cat.get(key), str):
                    seen.setdefault(cat[key], []).append(key)
            if len(seen) > 1 and (loc, en_val) not in CONSISTENCY_OK:
                rendered = ' | '.join(
                    f'{t!r} <- {sorted(ks)}' for t, ks in sorted(seen.items()))
                errors.append(
                    f'{loc}: EN {en_val!r} renders {len(seen)} different ways: '
                    f'{rendered} — unify them, or add ({loc!r}, {en_val!r}) to '
                    'CONSISTENCY_OK with the reason')

    if errors:
        print('i18n-locale-check: FAIL', file=sys.stderr)
        for e in errors:
            print(f'  {e}', file=sys.stderr)
        sys.exit(1)
    print(f'i18n-locale-check: OK — {len(cats)} locales × {len(ref_keys)} keys '
          'vs the EN base; parity, slots, plural categories, script all clean')


if __name__ == '__main__':
    main()
