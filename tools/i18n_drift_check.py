#!/usr/bin/env python3
"""i18n-drift — did an English string's WORDING move without the 30 locales?

# The hole this closes, measured rather than reasoned

`make lint` already runs four i18n gates, and on 2026-09-16 all four were shown
green against a deliberate edit that changed an English value's wording and
touched no locale:

    ("peerconn.connector_add", Message::Simple("Add connector"))
                                            -> "Add rendezvous node"

    i18n-locale-check   OK   (parity, slots, plurals, script — all still true)
    i18n-callsite-check OK   (the call site still resolves, slots match)
    i18n-untranslated   OK   (every non-Latin value still carries its script)
    i18n-lint           OK   (no raw literal was introduced)

Every one of them is correct about its own subject. **None of them is about the
translations still MEANING the English.** So the 30 overlays silently become
stale: the app ships a locale that says something the product no longer says,
and the only way anyone finds out is a speaker of that language reading it.

That is strictly worse than a missing key, which `i18n-locale-check` catches
loudly — a wrong translation renders perfectly.

# The mechanism

One row per key: the key, and a digest of the English text **the locales were
translated from**. A row's digest not matching the live EN value means the
English moved, and the run fails asking the only question that matters:

    did you resweep the 30 locales, or are you taking the debt?

Both answers are legitimate and both are spelled the same way — update the row.
Taking the debt is `stale`, which keeps the row's digest current (so the gate
stops nagging) while **printing the debt on every lint run**, by name. A debt
that prints is a debt somebody pays; a debt in a handoff is one nobody reads.

# Three things it deliberately does not do

**It does not check that a translation is GOOD.** Nothing mechanical can. What
it checks is that somebody was asked.

**It does not fire on a NEW key.** A key with no row is reported as unrecorded
and fails, which is the same question in its other direction — a new English
string needs 30 translations too, and `i18n-locale-check` already refuses a key
the overlays lack, so in practice this arm catches a key added to the baseline's
blind spot rather than an ordinary addition.

**It does not fire on a key being REMOVED** — it says so and fails, because a
baseline outliving its subject is the shape that silently re-admits debt (the
`vocab-lint` lesson: a set comparison must fail BOTH ways).
"""

import hashlib
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from i18n_locale_check import parse_en, values  # noqa: E402  (one EN parser, not two)

BASELINE = os.path.join('tools', 'i18n-drift-baseline.txt')

# `translated` — the overlays were written against this English.
# `stale`      — the English moved and the overlays have NOT been reswept.
#                A deliberate, named, printed debt.
STATES = ('translated', 'stale')


def digest(msg):
    """A digest over every string in a message, plural categories included."""
    h = hashlib.sha256()
    for v in values(msg):
        h.update(v.encode('utf-8'))
        h.update(b'\x1f')
    return h.hexdigest()[:16]


def read_baseline(path=BASELINE):
    rows = {}
    if not os.path.exists(path):
        sys.exit(f'i18n-drift: no baseline at {path} (generate with --write)')
    for n, line in enumerate(open(path, encoding='utf-8'), 1):
        line = line.split('#', 1)[0].strip()
        if not line:
            continue
        parts = line.split()
        if len(parts) != 3 or parts[2] not in STATES:
            sys.exit(f'i18n-drift: {path}:{n}: expected "<key> <digest> <{"|".join(STATES)}>"')
        key, dig, state = parts
        if key in rows:
            sys.exit(f'i18n-drift: {path}:{n}: {key} appears twice')
        rows[key] = (dig, state)
    return rows


def write_baseline(en, previous, path=BASELINE):
    """Rewrite the baseline at the CURRENT English, preserving each row's state.

    A regenerate is not a way to clear the debt: a row already marked `stale`
    stays `stale`, and only a human editing that word says the locales were
    reswept. Otherwise the escape hatch for "the gate is nagging" would also be
    the escape hatch for "the debt is gone".
    """
    with open(path, 'w', encoding='utf-8') as f:
        f.write('# i18n-drift baseline — see tools/i18n_drift_check.py.\n')
        f.write('# <key> <digest of the English the locales were translated from> '
                '<translated|stale>\n')
        f.write('# Regenerate: python3 tools/i18n_drift_check.py --write\n')
        f.write('# A `stale` row stays `stale` across a regenerate. Only you can say\n')
        f.write('# the 30 overlays were reswept, and the way you say it is this word.\n')
        for key in sorted(en):
            state = previous.get(key, (None, 'translated'))[1]
            f.write(f'{key} {digest(en[key])} {state}\n')


def main():
    en = parse_en()
    if '--write' in sys.argv:
        previous = read_baseline() if os.path.exists(BASELINE) else {}
        write_baseline(en, previous)
        print(f'i18n-drift: wrote {BASELINE} ({len(en)} keys)')
        return 0

    rows = read_baseline()
    moved, unrecorded, orphaned = [], [], []
    for key, msg in sorted(en.items()):
        if key not in rows:
            unrecorded.append(key)
            continue
        if rows[key][0] != digest(msg):
            moved.append(key)
    for key in sorted(rows):
        if key not in en:
            orphaned.append(key)

    stale = sorted(k for k, (_, s) in rows.items() if s == 'stale' and k in en)

    if moved or unrecorded or orphaned:
        print('i18n-drift: FAIL', file=sys.stderr)
        for key in moved:
            print(f'  MOVED       {key}\n'
                  f'              the English changed. Resweep the 30 locales, or take the\n'
                  f'              debt by marking this row `stale` — then '
                  f'`python3 tools/i18n_drift_check.py --write`.', file=sys.stderr)
        for key in unrecorded:
            print(f'  UNRECORDED  {key} — a new English string owes 30 translations; '
                  f'add a row.', file=sys.stderr)
        for key in orphaned:
            print(f'  ORPHANED    {key} — in the baseline, gone from the catalog. A '
                  f'baseline that outlives its subject re-admits debt silently.',
                  file=sys.stderr)
        return 1

    if stale:
        print(f'i18n-drift: OK — {len(en)} keys, but {len(stale)} carry UNSWEPT '
              f'translations (the overlays still say the old thing):')
        for key in stale:
            print(f'    stale: {key}')
    else:
        print(f'i18n-drift: OK — {len(en)} keys, every overlay swept against the '
              f'English it was translated from')
    return 0


if __name__ == '__main__':
    sys.exit(main())
