# `locales/` — overlay message catalogs

Each `*.json` file is **one locale**; the file stem is the locale id (`es.json`
→ `es`). These are the overlay catalogs that layer over the **compiled-in `en`
base** (`src/i18n.rs` `EN`). `build.rs` parses them and codegens `&'static`
Rust literals (`$OUT_DIR/embedded_locales.rs`), so the catalogs ship as
compiled-in data with **no runtime JSON parser**.

- **Default build bakes every file here** (`I18N_LOCALES_ROOT` defaults to this
  dir). Set `I18N_LOCALES_ROOT=""` for a lean `en`-only build; set it to another
  path to bake a different set. `en.json` here is ignored — `en` is the base.
- **Partial is fine.** A locale need only carry the keys it translates; any key
  it omits falls back to the `en` template (never a raw key or a blank — D13).
  Start with the shared **base vocabulary**; extend per-surface over time.
- A locale must appear in `LOCALES` (`src/i18n.rs`) to be pickable in Settings —
  the roster row also declares its `dir` (`ltr`/`rtl`).

## Format

```json
{
  "btn.save": "Guardar",
  "peer.count": { "one": "{n} par", "other": "{n} pares" }
}
```

- A **string** value → a `Simple` message. `{named}` slots are interpolated and
  **bidi-isolated by construction** (FSI/PDI) by `t()` — never wrap args yourself.
- An **object** value → a `Plural` message keyed by CLDR **cardinal** category
  (`zero` / `one` / `two` / `few` / `many` / `other`). Provide exactly the
  categories your language uses — the selector (`i18n::plural_category`,
  CLDR-pinned) picks one by the count `n`:
  - `es`: `one`, `other`
  - `he`: `one`, `two`, `other`
  - `ar`: `zero`, `one`, `two`, `few`, `many`, `other`
- An unknown category name, a non-string form, or a key not present in `en`
  fails the build / the validity test loudly (orphan/typo guard).

## ⚠️ Translation status — INITIAL, PENDING NATIVE REVIEW

**Coverage: complete — all 31 non-pseudo locales.** Every catalog here is 1:1
with the full `EN` base (**512 keys** as of 2026-07-22 — window titles, menu
categories, tooltips, placeholders, status chips, prose, and the shared base
vocabulary; verified by the `overlay`/orphan tests and by
`tools/i18n_locale_check.py`). The full roster (30 overlays + the `en`
base = 31 languages): **`es he ar` · `fr de it pt nl sv da no fi ru zh` ·
`ja ko vi th id` · `uk pl cs ro` · `el hu tr` · `hi bn fa ur`**. Every language
is pickable, flips `dir` (**4 RTL**: `ar he fa ur`, plus the `en-XA` pseudo),
and carries its CLDR-correct `peer.count` plural forms. No pending catalogs
remain.

Every catalog is a **machine-assisted first pass, not yet reviewed by native
speakers.** Before this ships as a user-facing localization, each must be
reviewed by a fluent speaker. Known judgment calls a reviewer should weigh:

- **`peer.count` plural forms** — Arabic's six CLDR forms and Hebrew's
  one/two/other, where counted-noun grammar is subtle (Arabic's dual `نظيران`
  vs. the numeral-prefixed singular currently used, etc.).
- **RTL "back" arrows** — `btn.back` / `kb.back_to_list` use a right-pointing
  `→` in all four RTL locales (`he ar fa ur` — the glyph isn't auto-mirrored by
  the bidi algorithm); confirm that reads correctly in context.
- **Counted-noun grammar in the plural-invariant languages** — `ja ko vi th id`
  (other-only) and `hu tr fa` keep the noun uninflected after a numeral, so both
  `peer.count` forms read alike; a reviewer should confirm the counter/measure
  word (e.g. Japanese `個`, Korean `개`) and any classifier reads naturally.
- **Loanword vs. native term for `Peer`** — kept as a Latin/transliterated
  loanword in most catalogs (`pl/cs/ro/hu/tr` "peer", `ru` Пир, `uk` Пір,
  `hi/bn` पीयर/পিয়ার, `fa` همتا, `ur` پیئر, `el` ομότιμος); a reviewer should
  confirm the register the audience expects.
- **Product/jargon terms** — whether to translate vs. keep: `Shell`
  (מעטפת / الصدفة), `Wire Recorder`, `Path Tap`, `Chain Trace`, and whether
  `Entity` (the core concept) should localize (`Entidad` / ישויות / كيانات).
- **Register/dialect** — Spanish `Ajustes` vs. `Configuración`, `backend`
  loanword vs. translation, Hebrew `סטטוס` vs. `מצב`.
- **Theme names** (2026-07-22) — the standard split: descriptive words are
  translated, scheme proper nouns are not. `Solarized Dark` → `Solarized
  oscuro`; `Nord` / `Dracula` / `Gruvbox` / `Monokai` stay as written, the way
  a typeface name would. Each built-in has its own key
  (`theme_tokens::display_label` resolves `theme.<name>`), so a locale that
  would rather transliterate — e.g. `ドラキュラ` — can, without touching code.
- **Technical tokens** (2026-07-22) — split by whether the token is a *word*.
  `IndexedDB` and `OPFS` are product names and stay in every locale; `main
  thread`, `in-memory`, `native store` are words and are translated. A reviewer
  should confirm the translated forms read as technical register rather than
  literal (e.g. `hilo principal`, `fil d'exécution principal`).

- **`pt` is Brazilian** (2026-07-22, operator's call). The file had been a mix:
  `arquivo`×12 vs `ficheiro`×4, `registro`×5 vs `registo`×4, `salvar`×3 vs
  `guardar`×1, `tela` vs `ecrã`, and one European progressive (`A carregar…`)
  against 21 gerunds. The European forms were a scattered 22-key minority, not a
  coherent register, so the mix served neither audience; they were normalized to
  the Brazilian majority (`aba` not `separador`, `aplicativo` not `aplicação`,
  `área de trabalho`, `ao vivo`, `planejado`, `conectar` not `ligar`, and `“ ”`
  rather than `« »`). We do not ship sub-dialects — one register per locale.
  A reviewer for Portugal should expect BR copy, not a bug.
- **Divergences that are deliberate** are recorded in `CONSISTENCY_OK`
  (`tools/i18n_locale_check.py`) with the reason, because the checker otherwise
  requires one EN string to render one way per locale. Today: Greek gender
  agreement on "All" (masc. `Όλοι` for sites, neut. `Όλα` for log entries);
  theme-name-vs-render-mode on "Light"/"Dark" in `el`/`ko`; Arabic
  definite/indefinite "Device"; Indonesian transitive/intransitive "Stop".
- **Two divergences are marked `PENDING` there** and want a native decision,
  not a guess: `ur` renders "All" as both `سب` and `تمام`, and `cs` declines the
  loanword *peer* two ways (`ID peera` / `ID peeru`, animate vs. inanimate).
  They are pinned at today's value so they cannot drift further.

### What these catalogs do NOT cover

- **The pre-WASM boot HTML (`index.html`) is English-only** — ~600 words, and
  invisible to the prose scanner, which globs only `.rs`. It runs *before* the
  WASM binary, so `t()` and these catalogs don't exist yet; localizing it needs
  a build-time step emitting a JS locale map (the `entity_language` localStorage
  mirror is already there to key off). Deliberately deferred — ~6 short strings
  are on the default boot path (`Loading Entity Browser…` and friends, ~500 ms)
  and the other ~469 words are the `?systemrecovery=1` diagnostic console.
  Detail + measurements: `docs/plans/AUDIT-I18N-COVERAGE-GAP-2026-07-19.md`.
- **The demo-site markdown** (`raw=24`) — content addressed into the tree, not
  interface text. Localizing it means building a localized demo site.

Treat the current strings as a solid first draft, not final copy.

### Do not hand-edit without running the checker

`python3 tools/i18n_locale_check.py` (also in `make lint`) enforces parity,
slot preservation, plural-category agreement with each locale's own
`peer.count`, whitespace, cross-key consistency, and — the one no human catches
— **Cyrillic/Greek letters inside a Latin-script locale.** A homoglyph like
`paritа` (Cyrillic `а`) is identical on screen to `parita` and breaks nothing
visibly; two were live in `fi` until the checker was written.

Parity is measured against the **`EN` base in `src/i18n.rs`**, not against a
sibling catalog. It used to compare overlays to `es`, which meant a key added
to `EN` and to no overlay was invisible — the reference lacked it too. If you
add a key to `EN`, this checker is what tells you all 30 files need it.
