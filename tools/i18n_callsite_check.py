#!/usr/bin/env python3
"""Validate i18n CALL SITES against the EN base — the direction nothing checked.

`i18n_locale_check.py` asks whether the catalogs match `EN`. `i18n_prose_scan.py`
asks whether anything is still hardcoded. Neither asks the third question: does
every `t("key")` in the code name a key that actually exists, and does it pass
the args that key's template interpolates?

Both failures are silent and user-visible, because keys are strings and nothing
in the type system checks them:

  1. MISSING KEY  — `t()` renders `key.to_string()`, so the user literally sees
                    `peers.col_peer_id` on screen (a D13 violation). Rename a
                    key in EN and miss one call site and this is what ships.
  2. SLOT MISMATCH— an arg the template doesn't name is dropped; a slot the call
                    doesn't supply renders as a literal `{peer}`.

Only *literal* keys can be checked. Dynamic lookups (`format!("window.{}", ..)`,
`t(label_key, ..)`) are enum→key mappings resolved at runtime; they are covered
instead by the Rust-side drift tests (e.g.
`every_window_title_key_has_a_registered_window`).

Exit non-zero on any violation. `python3 tools/i18n_callsite_check.py`.
"""
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from i18n_locale_check import parse_en  # noqa: E402

SRC = 'src'
# `src/i18n.rs` is the catalog itself, and its tests deliberately call
# `t("no.such.key")` to prove the missing-key path. Excluded for the same reason
# the prose scanner excludes it.
SKIP = {os.path.join('src', 'i18n.rs')}

SLOT = re.compile(r'\{(\w+)\}')
CALL = re.compile(r'\bt(_plural)?\(\s*"([a-z0-9_][a-z0-9_.]*)"\s*,', re.M)


def arg_names(src, start):
    """Slot names in the `&[("name", ...)]` list, and the raw call segment.

    `start` is already INSIDE `t(`'s parens, so depth begins at 1 and we scan to
    the paren closing the call. (Starting at 0 and stopping on the first `)`
    truncates at the first tuple and reports every multi-arg call as broken —
    which is exactly the false alarm this comment exists to prevent.)
    """
    depth, i = 1, start
    while i < len(src):
        c = src[i]
        if c == '"':
            i += 1
            while i < len(src) and src[i] != '"':
                i += 2 if src[i] == '\\' else 1
        elif c in '([{':
            depth += 1
        elif c in ')]}':
            depth -= 1
            if depth == 0:
                break
        i += 1
    seg = src[start:i]
    return set(re.findall(r'\(\s*"(\w+)"\s*,', seg)), seg


def main():
    en = parse_en()
    errors, checked = [], 0

    for root, _dirs, files in os.walk(SRC):
        for fname in sorted(files):
            if not fname.endswith('.rs'):
                continue
            path = os.path.join(root, fname)
            if path in SKIP:
                continue
            src = open(path, encoding='utf-8').read()
            lines = src.split('\n')
            for m in CALL.finditer(src):
                line_no = src[:m.start()].count('\n') + 1
                if lines[line_no - 1].lstrip().startswith('//'):
                    continue
                checked += 1
                key, is_plural = m.group(2), bool(m.group(1))
                if key not in en:
                    errors.append(
                        f'{path}:{line_no}: t("{key}") — no such key in the EN '
                        f'base; renders the raw key to the user')
                    continue
                msg = en[key]
                want = set()
                for v in ([msg] if isinstance(msg, str) else msg.values()):
                    want |= set(SLOT.findall(v))
                got, seg = arg_names(src, m.end())
                if is_plural:
                    got.add('n')  # t_plural always supplies the count
                if '&[' not in seg:
                    continue      # args come from a variable — can't judge
                if got != want:
                    errors.append(
                        f'{path}:{line_no}: {key} — template interpolates '
                        f'{sorted(want)}, call passes {sorted(got)}')

    if errors:
        print('i18n-callsite-check: FAIL', file=sys.stderr)
        for e in errors:
            print(f'  {e}', file=sys.stderr)
        sys.exit(1)
    print(f'i18n-callsite-check: OK — {checked} literal call site(s) resolve '
          'against the EN base, slots match')


if __name__ == '__main__':
    main()
