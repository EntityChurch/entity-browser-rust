#!/usr/bin/env python3
"""i18n prose scanner — the `raw` metric for tools/i18n-lint.sh.

WHY this exists (2026-07-19): the previous `raw` metric counted un-extracted
literals in only FIVE anchor positions (card/field/button/set_text_content/
title-attr). Every other DOM-emit path — checkbox/radio labels, subheadings,
table headers, td_text, loading/empty/error, collapsible headers, field HINTS,
`format!`-assembled sentences, the status bar — was invisible, so "raw=0" never
meant "no untranslated UI prose." See docs/plans/AUDIT-I18N-COVERAGE-GAP-2026-07-19.md.

This replaces the opt-in anchor count with an **opt-out** detector: a string
literal in src/dom + src/views + the top-level render helpers is counted as
translatable prose UNLESS it is excluded (CSS / URI / tree-path / i18n key /
log-or-diagnostic / value-render / test region) or explicitly suppressed:

  - a line carrying `// i18n-ignore`         → that line's literals are skipped
  - a file carrying `//! i18n-ignore-file`   → the whole file is skipped
    (use for wholly dev-facing surfaces: shell/console command output, value
    renderings, protocol dumps — English by design, design §6)

Output: one `path raw=N` line per file with N>0, LC_ALL=C sorted — the same
baseline-ratchet contract as before (must EQUAL the baseline; ratchet DOWN as
surfaces are extracted). Deterministic: no clock, no randomness, stable order.
"""
import re, sys, glob

STR = re.compile(r'"((?:[^"\\]|\\.)*)"')
LOG_CTX = re.compile(
    r'tracing::|(?:^|\s)(?:warn|error|info|debug|trace)!|console|\.log\('
    r'|log_line|log_param|__entity_browser_log|web_sys::console|panic!'
    r'|unreachable!|todo!|\.expect\(|debug_assert')
CSS_HINT = re.compile(
    r'(?:\d(?:px|em|rem|vh|vw|fr|%)|var\(--|#[0-9a-fA-F]{3,8}\b|rgba?\(|calc\('
    r'|:\s*(?:flex|grid|block|inline|none|absolute|relative|fixed|sticky|center'
    r'|column|row|wrap|nowrap|bold|normal|auto|hidden|visible|pointer|middle'
    r'|start|end|baseline|stretch|space-between|space-around)\b'
    r'|translate|scale\(|rotate\(|cubic-bezier|linear-gradient|[0-9]+ms\b'
    r'|!important)')
# debug/value renderings & format specs: `Foo({})`, `{:?}`, `{e:?}`, `{n:>4}`
VALUE_RENDER = re.compile(r'\{:?[^}]*:[^}]*\}|\{:[^}]*\}|\{\}[)\]]|\w+\(\{\}|\(\{\} ')

# KeyboardEvent.key values and modifier names — string-compared in event
# handlers (`if key == "ArrowUp"`), never rendered as UI text.
KEYNAMES = {
    'Enter', 'Tab', 'Escape', 'Backspace', 'ArrowUp', 'ArrowDown', 'ArrowLeft',
    'ArrowRight', 'PageUp', 'PageDown', 'Shift', 'Control', 'Alt', 'Meta',
    'CapsLock',
}


def is_pathy(s):
    if '://' in s or s.startswith('/') or s.startswith('app/') or s.startswith('system/'):
        return True
    if re.search(r'\{[a-z_]+\}/', s):
        return True
    if s.startswith('entity') and '/' in s:
        return True
    return False


def is_keyish(s):
    return bool(re.fullmatch(r'[a-z0-9_.\-]+', s))


def is_css(s):
    if CSS_HINT.search(s):
        return True
    if s.count(':') >= 1 and ';' in s:
        return True
    if re.fullmatch(r'[a-z]+(?:-[a-z]+)+', s):
        return True
    # a CSS class list: every token is class-like and at least one is hyphenated
    # ("peer-badge system", "gm-stage-area gm-expanded") — never UI copy.
    toks = s.split()
    if len(toks) >= 2 and all(re.fullmatch(r'[a-z][a-z0-9]*(?:-[a-z0-9]+)*', t) for t in toks) \
            and any('-' in t for t in toks):
        return True
    return False


# HTML attribute rel-values and other markup constants that read like prose.
MARKUP_CONST = {'noopener noreferrer', 'noopener', 'noreferrer'}


def is_code(s):
    if '<' in s and '>' in s:              # HTML tags: <p>…</p>
        return True
    if '&&' in s or '=>' in s or '.parentNode' in s or '.remove(' in s \
            or '||' in s:                   # inline JS
        return True
    if s in MARKUP_CONST:
        return True
    # a single PascalCase identifier with ≥2 internal capitals — a Rust Debug
    # struct name (ContentSiteModel/EntityTreeModel), never a UI label.
    if ' ' not in s and len(re.findall(r'[A-Z]', s)) >= 3 and re.fullmatch(r'[A-Za-z]+', s):
        return True
    return False


def looks_prose(s):
    s2 = s.strip()
    if len(s2) < 2:
        return False
    if not re.search(r'[A-Za-z]', s2):
        return False
    if VALUE_RENDER.search(s2):
        return False
    if '::' in s2:  # Rust path in a diagnostic/error string, never UI prose
        return False
    if is_code(s2):  # HTML / inline JS / markup const / Debug struct name
        return False
    if s2[:1] in '◆☰✗✓•':  # demo glyphs / console status-led diagnostic lines
        return False
    if is_pathy(s2) or is_keyish(s2) or is_css(s2):
        return False
    # single token: keep only a Capitalized alpha label (e.g. "Connected"),
    # but NOT an all-caps DOM node name (BODY/HTML) or a KeyboardEvent.key /
    # modifier constant used in an event comparison (never UI text).
    if ' ' not in s2:
        if s2.isupper():
            return False
        if s2 in KEYNAMES:
            return False
        return bool(re.fullmatch(r'[A-Z][A-Za-z]{2,}', s2))
    # multi-token: reject an all-lowercase short token list with no sentence
    # words (likely a class/attr list, not prose)
    letters = re.sub(r'[^A-Za-z ]', '', s2)
    if letters and letters == letters.lower() and len(s2.split()) <= 3 and not any(
            w in s2.lower() for w in (
                'the', ' to', ' on', ' a ', 'no ', 'not', ' is', 'are', 'use',
                'show', 'peer', 'device', 'window', 'file', 'site', 'save',
                'open', 'close', 'this', 'your', 'from')):
        return False
    return True


def join_continued_literals(lines):
    """Collapse Rust `\\`-at-end-of-line string continuations onto the literal's
    opening line, blanking the consumed lines so every later line number is
    preserved.

    WHY (2026-07-21): the scan below is line-by-line and `STR` is not DOTALL, so
    a literal written as

        "Live wire frames for this peer (newest first; ring buffer). Only \\
         populates when cross-peer traffic flows."

    matched NOTHING — the regex cannot close the quote on either physical line.
    Every multi-line literal was therefore invisible to the gate, and multi-line
    is exactly how the longest prose is written: window hints, empty states,
    help text, the durability banners. `raw=24` was reported as "done" while ~20
    user-facing strings sat unseen. Same failure mode as the audit's original
    finding, one level down. See AUDIT-I18N-COVERAGE-GAP-2026-07-19.md.

    An `i18n-ignore` marker anywhere in the literal's span is carried onto the
    joined line, so a marker may sit on the opening OR the closing line.
    """
    out = list(lines)
    i = 0
    while i < len(out):
        line = out[i]
        # An in-string continuation is a backslash immediately before the
        # newline, with an odd number of unescaped quotes so far on the line.
        stripped = line.rstrip('\n')
        if not stripped.endswith('\\') or stripped.lstrip().startswith(('//', '*', '/*')):
            i += 1
            continue
        if _unescaped_quote_count(stripped) % 2 == 0:
            i += 1  # backslash, but not inside an open literal
            continue
        # Consume following lines until the literal closes.
        j, merged, ignored = i, stripped[:-1], 'i18n-ignore' in line
        while j + 1 < len(out):
            j += 1
            nxt = out[j].rstrip('\n')
            ignored = ignored or 'i18n-ignore' in out[j]
            merged += nxt.lstrip()
            if _unescaped_quote_count(nxt) % 2 == 1:
                break  # this line closed the literal
            if not nxt.endswith('\\'):
                break  # malformed / not a continuation after all
            merged = merged[:-1]
        if ignored and 'i18n-ignore' not in merged:
            merged += ' // i18n-ignore'
        out[i] = merged + '\n'
        for k in range(i + 1, j + 1):
            out[k] = '\n'
        i = j + 1
    return out


def _unescaped_quote_count(s):
    """Count `"` not preceded by an odd run of backslashes."""
    n, k = 0, 0
    while k < len(s):
        if s[k] == '\\':
            k += 2
            continue
        if s[k] == '"':
            n += 1
        k += 1
    return n


def test_region_lines(lines):
    in_test = set()
    i, n = 0, len(lines)
    while i < n:
        if re.search(r'#\[cfg\(test\)\]|#\[test\]|#\[tokio::test|\bmod tests\b', lines[i]):
            depth, started, j = 0, False, i
            while j < n:
                depth += lines[j].count('{') - lines[j].count('}')
                if '{' in lines[j]:
                    started = True
                in_test.add(j + 1)
                if started and depth <= 0:
                    break
                j += 1
            i = j + 1
            continue
        i += 1
    return in_test


# Atom-defining / machinery files: literal UI text is their raw material (they
# hold the default labels the atoms emit), exactly as the old anchor metric's
# ALLOW skipped them. Same rationale as ui-lint's allow-list.
ALLOW = re.compile(r'^src/dom/(?:components|theme|style|util)\.rs$')


def scan(path):
    if ALLOW.match(path):
        return 0
    with open(path, encoding='utf-8') as fh:
        lines = fh.readlines()
    if any('i18n-ignore-file' in ln for ln in lines[:20]):
        return 0
    test_lines = test_region_lines(lines)
    # Collapse `\`-continued literals BEFORE the line loop (see the function's
    # docstring): without this every multi-line string is structurally invisible.
    lines = join_continued_literals(lines)
    count = 0
    log_depth = 0  # >0 while inside a multi-line log/diagnostic macro call
    for ln, line in enumerate(lines, 1):
        stripped = line.lstrip()
        # comment lines (doc `///`, `//!`, line `//`, block-continuation `*`)
        if stripped.startswith(('//', '*', '/*')):
            continue
        if log_depth > 0:
            log_depth += line.count('(') - line.count(')')
            if log_depth < 0:
                log_depth = 0
            continue
        if ln in test_lines or 'i18n-ignore' in line:
            continue
        if LOG_CTX.search(line):
            # a log macro may span lines — enter skip mode until parens balance
            net = line.count('(') - line.count(')')
            if net > 0:
                log_depth = net
            continue
        work = re.sub(r'\bt\(\s*"(?:[^"\\]|\\.)*"', 't(', line)  # drop t() keys
        for m in STR.finditer(work):
            if looks_prose(m.group(1)):
                count += 1
    return count


def files():
    out = sorted(set(
        glob.glob('src/dom/**/*.rs', recursive=True) +
        glob.glob('src/views/**/*.rs', recursive=True)))
    # top-level files that emit UI text outside the two render dirs — status
    # bar / durability banner (app, storage_durability), peer-mode & storage
    # display labels (peer_display), byte/size formatting (format), and the
    # authorization profile labels + scope summaries rendered in the grant
    # picker and Access Log (backend_auth — enum→&'static str display methods).
    # Also `watchdog.rs` (the frozen-frame overlay: its message + Reload/Dismiss
    # buttons) and `session_config.rs` (the "this tab can't save" copy) — both
    # write straight to the DOM from outside the two render dirs, and both were
    # invisible purely because this list did not name them (2026-07-21).
    for extra in ('src/app.rs', 'src/window.rs', 'src/peer_display.rs',
                  'src/storage_durability.rs', 'src/format.rs',
                  'src/backend_auth.rs', 'src/watchdog.rs',
                  'src/session_config.rs'):
        out.append(extra)
    return sorted(set(out))


def main():
    rows = []
    for f in files():
        try:
            c = scan(f)
        except FileNotFoundError:
            continue
        if c > 0:
            rows.append((f, c))
    # LC_ALL=C-style byte sort on path for stable, locale-independent order
    rows.sort(key=lambda r: r[0].encode())
    for f, c in rows:
        print(f'{f} raw={c}')


if __name__ == '__main__':
    main()
