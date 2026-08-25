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

The `es` / `he` / `ar` catalogs here now cover the **full extracted shell-chrome
catalog** (every `en` key — window titles, menu categories, tooltips,
placeholders, and the shared base vocabulary; 1:1 with `EN`, verified by the
`overlay`/orphan tests). They are a **machine-assisted first pass, not yet
reviewed by native speakers.** Before this ships as a user-facing localization,
each catalog must be reviewed by a fluent speaker. Known judgment calls a
reviewer should weigh:

- **`peer.count` plural forms** — Arabic's six CLDR forms and Hebrew's
  one/two/other, where counted-noun grammar is subtle (Arabic's dual `نظيران`
  vs. the numeral-prefixed singular currently used, etc.).
- **RTL "back" arrows** — `btn.back` / `kb.back_to_list` use a right-pointing
  `→` in `he`/`ar` (the glyph isn't auto-mirrored by the bidi algorithm);
  confirm that reads correctly in context.
- **Product/jargon terms** — whether to translate vs. keep: `Shell`
  (מעטפת / الصدفة), `Wire Recorder`, `Path Tap`, `Chain Trace`, and whether
  `Entity` (the core concept) should localize (`Entidad` / ישויות / كيانات).
- **Register/dialect** — Spanish `Ajustes` vs. `Configuración`, `backend`
  loanword vs. translation, Hebrew `סטטוס` vs. `מצב`.

Treat the current strings as a solid first draft, not final copy.
