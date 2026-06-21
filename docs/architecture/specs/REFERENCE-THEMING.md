# Theming — Closeout Reference

**The single authoritative doc for how theming works, how to change a
color, and how to add a theme.** The appearance arc is closed for now:
the in-app **chrome** and the **content-site overlay** both theme, with a
two-control Settings UI (System appearance + Site appearance). Fonts and
per-site/custom themes are deliberately deferred (see §9).

This supersedes the build-time notes scattered in handoffs. The companion
`REFERENCE-THEMING-SURVEY.md` is the *pre-build inventory* (the
raw color/font survey); read it only if you need the original audit. For
*how it works today*, this doc is canonical.

> **It is colors, nothing more (today).** A theme is a flat
> `token → value` map. No markup/theme DSL. Fonts are tokens too
> (`--font-ui`/`--font-mono`/`--fs-base`) but are not yet exposed as a
> control — they ride whatever the active theme sets.

---

## 1. The one mechanism (read this first)

Theming works through **CSS custom properties** (`--token`) defined once
on the document `:root`, in a `<style>` element in `<head>`. **Switching a
theme rewrites that one element's text — no DOM rebuild, no re-render.**

The architectural fact that makes this cheap: there is **exactly one
shadow root** in the whole app — `#dom-layer` (`src/dom/mod.rs`). The
status bar, the `#site-layer` content-site overlay, the loading screen,
and every runtime banner are **light DOM**. CSS custom properties are
*inherited* properties, so a value set on `:root` (`documentElement`)
cascades **through** the single shadow boundary into everything. One
`:root` block drives the entire app.

There are **two independent token families**, each its own `<style>`
element, each installed the same way:

| Family | `<style id>` | Drives | Source of truth |
|---|---|---|---|
| **Chrome** `--*` | `theme-vars` | windows, panels, palette, settings, status bar, the 15 views, banners | `src/theme_tokens.rs` `THEMES` (`DARK`, `LIGHT`) |
| **Site overlay** `--site-*` | `site-theme-vars` | the Content Site overlay + window directory rail (`content_site.rs`, `site_directory.rs`) | `src/theme_tokens.rs` `SITE_TOKENS` |

Both layers live in `src/theme_tokens.rs` (crate root — **not** under
`src/dom/`, which is `#[cfg(target_arch="wasm32")]`; native `model.rs` and
tests need the data).

Every style string references a token as **`var(--token, #literal)`** —
the literal fallback is the original pre-theming hex, so a missing/unset
token is invisible (renders the original color), never a blank.

---

## 2. Chrome theme layer (`--*`)

`src/theme_tokens.rs`:

- **`Theme { name, label, vars }`** — `vars` is an ordered
  `&[(token, value)]`. `name` is the persisted id (`"dark"`); `label` is
  the Settings dropdown text (`"Dark"`).
- **`DARK`** — ~33 tokens, captured **byte-identical** from the
  pre-theming hardcoded palette (surfaces, text, borders, accents, peer
  badges, the harmonized `--status-*` family, fonts).
- **`LIGHT`** — same token keys, re-tuned for dark-text-on-light
  (accents/status darkened for contrast).
- **`THEMES: &[Theme] = &[DARK, LIGHT]`** — the registry. `DARK` is
  first = the default. **Adding a theme is one entry here** (§7).
- **`lookup(name) -> &Theme`** — unknown id → `DARK`.
- **`root_block(theme) -> String`** — builds `:root{ … }`.
- **`install_root(name)`** *(wasm)* — inject/rewrite `<style id="theme-vars">`
  in `<head>` (idempotent: reuses the element).
- **`boot_choice() -> String`** *(wasm)* — read the localStorage mirror
  `entity_theme`, else `"dark"`. Lets the right theme paint on frame one
  (no flash) before peers boot.
- **`apply_and_persist(name)`** *(wasm)* — write the LS mirror **and**
  `install_root` (live recolor). The durable record is the tree
  (`SettingsState.theme`); this is the appearance side.
- **`STATUS_OK/ERR/INFO/WARN`** consts — the semantic status family,
  referenced instead of raw `#0f0`/`#f66`/… so every window's
  success/error glyphs share one family that retunes per theme.

Native builds get no-op stubs for the wasm fns; the pure data
(`THEMES`, `root_block`, `lookup`) is available natively for tests and
`views/settings/model.rs`.

---

## 3. Site overlay layer (`--site-*`)

The Content Site overlay (`#site-layer`) and the Content Site **window's**
directory rail carry their own palette — they never receive
`dom::style::DOM_STYLES`, and by design the site surface reads
*independently* of the chrome theme. So they are a second token family.

`src/theme_tokens.rs`:

- **`SITE_TOKENS: &[(site_token, default_hex, app_token)]`** — ~29 rows.
  `default_hex` is the overlay's original color (the `var()` fallback, and
  the strict-override fallback if a theme lacks the app token).
  `app_token` is the **chrome token this site token aliases to** in
  `"system"`/strict modes. Example:
  `("--site-bg", "#101018", "--overlay-bg")`.
- **The "Site appearance" setting picks what defines `--site-*`:**

  | Mode value | Meaning | What's installed |
  |---|---|---|
  | `"site"` *(default)* | the overlay's **own** theme | **nothing** — the `var(--site-X, #literal)` fallbacks render the original palette (byte-identical) |
  | `"system"` | follow the chrome theme | `--site-X: var(--app-token)` live aliases — re-resolve on a chrome flip with **no re-install** |
  | `"<theme-name>"` (e.g. `"light"`) | strict override to a specific theme | each `--site-X` **frozen** to that theme's value for its app token (stays put when chrome changes) |

- **`site_appearance_catalog() -> Vec<(&'static str, String)>`** — the
  dropdown options in order: `("site","Site's theme")`,
  `("system","Match system theme")`, then `("<name>","Always <Label>")`
  per registered theme. Adding a theme adds an "Always X" override for
  free.
- **`site_root_block(mode) -> Option<String>`** — `None` for `"site"`
  (inject nothing); `Some(:root{…})` of `var()` aliases for `"system"`;
  `Some(:root{…})` of frozen literals for a named theme (unknown → `DARK`).
  Pure — native-testable.
- **`install_site_root(mode)`** *(wasm)* — inject/rewrite
  `<style id="site-theme-vars">`. For `"site"` it **empties** any existing
  element (so the CSS fallbacks resume) rather than removing it.
- **`site_appearance_boot_choice()`** *(wasm)* — LS mirror
  `entity_site_appearance`, else `"site"`.
- **`apply_site_appearance(mode)`** *(wasm)* — LS write + `install_site_root`.

Because `--site-*` is defined on `:root` (head), it inherits into **both**
the light-DOM overlay and the shadow-DOM window rail — one block, both
surfaces.

---

## 4. The two Settings controls

Settings → **Appearance** (`src/dom/settings.rs render_appearance`,
registry-driven):

1. **Theme** (chrome) — dropdown `select[name^="theme-"]`, one
   `<option>` per `THEMES` entry → event `set_theme`.
2. **Site appearance** — dropdown `select[name^="site-appearance-"]`, one
   `<option>` per `site_appearance_catalog()` entry → event
   `set_site_appearance`.

Both are `<select>` dropdowns (registry-driven), so adding a theme
populates both automatically.

---

## 5. Data flow (one change, three sinks)

A theme/appearance change writes to **three** places, each with a job:

```
dropdown change
  → Action::WindowEvent { event, value }
  → views/settings/mod.rs handler
  → SettingsModel::set_theme / set_site_appearance
       ├─ write_state  → the TREE  (SettingsState, durable record)
       └─ theme_tokens::apply_and_persist / apply_site_appearance
            ├─ localStorage mirror (entity_theme / entity_site_appearance)  ← no-flash boot
            └─ install_root / install_site_root  ← live recolor (rewrite the <style>)
```

- **Tree** (`SettingsState.theme`, `.site_appearance`) — the durable
  record; survives reload via the IDB/OPFS substrate, reconciles across
  devices once peers boot.
- **localStorage mirror** — readable *synchronously at first paint*,
  before peers exist, so the chosen theme/appearance paints on frame one
  (no dark flash). Boot calls `install_root(boot_choice())` +
  `install_site_root(site_appearance_boot_choice())` in
  `main.rs start()` **before** the first DOM render / fast-paint.
- **The `<style>` element** — the live page recolor.

`SettingsState` (`views/settings/model.rs`) round-trips both fields in
CBOR; an old persisted entity without `site_appearance` decodes to the
`"site"` default (forward/backward compatible).

---

## 6. How to change a color

1. Find the token for the surface in `theme_tokens.rs` (`DARK`/`LIGHT` for
   chrome, `SITE_TOKENS` for the overlay).
2. Change its value(s). Done — every `var(--token)` reference picks it up.

If a surface still has a **raw hex** (no `var()`), tokenize it: replace
`#hex` with `var(--token, #hex)` at the call site, and ensure the token
exists. **The fallback literal must equal the original hex** (byte-identical
default). For a new chrome role, add a `(token, value)` pair to *both*
`DARK` and `LIGHT`. For a new overlay role, add a
`(site_token, default_hex, app_token)` row to `SITE_TOKENS` — pick the
`app_token` whose dark/light values give good contrast in both themes.

---

## 7. How to add a theme

1. Add one `pub const FOO: Theme = Theme { name, label, vars: &[…] }` to
   `theme_tokens.rs`. The simplest path: copy `DARK`/`LIGHT` and retune
   values; **keep the same token keys** (a `:root` block fully overrides
   only the keys it lists; any key you omit falls back to the `var()`
   literal, which is dark — so list them all).
2. Add it to `THEMES`.

That's it. The chrome dropdown, the Site-appearance dropdown's strict
"Always X" override, `lookup`, and the e2e all pick it up from the
registry. No renderer or wiring changes.

---

## 8. Surface map — tokenized vs. intentionally raw

**Tokenized (themes):**
- Chrome: `src/dom/style.rs`, `src/dom/theme.rs` consts, `index.html`
  `<style>`, and ~19 view/banner files (`var(--token, #literal)`).
- Overlay: `src/dom/content_site.rs` (`RESPONSIVE_CSS` + every inline
  style) and `src/dom/site_directory.rs` (the directory rail rows).

**Intentionally raw (NOT bugs — documented decisions):**
- **Emitted static-export CSS** — `src/content_site/static_export.rs`
  `PAGE_CSS` + the demo SVG. Published sites carry their **own** theme to
  *other people's* browsers; they don't use our runtime `--site-*` layer.
  Deferred (own palette, publish-time concern).
- **Semantic icon accents** — the directory rail's bookmark gold
  (`#e8c34a`) and keep-offline green (`#5fc27e`) **on-states**; their
  off-states *are* tokenized (`--site-text-faint-2`). Like syntax colors,
  these read on both themes and are conventionally theme-agnostic.
- **The fast-paint "connecting…" toast** (`src/boot_fast_paint.rs`) — a
  fixed translucent badge with its **own** dark `rgba(0,0,0,.55)`
  background, so its `#bbb` text reads on any page theme. (The fast-paint
  *content* render uses the tokenized `content_site::render` path; the
  feature is also gated off today.)
- **Window-internal accents left raw last session** (chrome pass): severity
  banner hues (update/storage/watchdog), knowledge-base syntax colors,
  shell `#9ac`/`#cb8`/`#bbb`. A future "tokenize the long tail" pass.
- **QR codes** (`#000`/`#fff`) — never theme (scannability).

---

## 9. Tests & verification

- **Native** (`theme_tokens.rs` tests): registry default, `root_block`
  well-formed, every token has a value; `SITE_TOKENS` well-formed + every
  `app_token` resolves in **both** themes; `site_root_block` per mode
  (`"site"`→None, `"system"`→`var()` aliases, strict→frozen literals,
  unknown→dark); catalog shape. `views/settings/model.rs`:
  `site_appearance` default + round-trip + `set_site_appearance` persists +
  dropdown selection.
- **e2e** (`tests/e2e_worker.rs` Phase 3): drives the chrome theme dropdown
  to "light" **and** the Site-appearance dropdown to "system", then asserts
  `#site-theme-vars` contains `--site-bg:var(--overlay-bg)` — the full
  delivery path (dropdown → action → model → install → live DOM).
- Verified green this arc: native (526+) · clippy · wasm(-release) ·
  e2e-worker 11/11.

---

## 10. Deferred / future (the seams are left open)

- **Fonts as a control** — the tokens exist (`--font-ui`/`--font-mono`/
  `--fs-base`); no UI to change them independent of the theme yet.
- **Per-site themes** — a site shipping its own CSS via its manifest. The
  `--site-*` layer is exactly the seam: a future renderer could populate
  `--site-*` from a site manifest instead of the appearance setting.
- **Custom / user themes** — `THEMES` is a static registry today; a
  user-defined theme would be a tree-persisted `Theme` merged into the
  registry. The "Always X" strict-override catalog already generalizes to
  any number of registered themes.
- **Dev-guide "Theming" pattern note** — a short section in
  `DEVELOPER-GUIDE.md` so new windows use `var(--token)` /
  `theme_tokens::STATUS_*`, never raw hex. (Small, pending.)
- **Boot tree→theme reconcile** — the LS mirror covers the normal flow; a
  cross-browser / imported profile shows the default until re-selected.
  Optional: read `SettingsState.theme`/`.site_appearance` once peers boot
  and re-install.
- **Tokenize the window-internal long tail** (§8) — the raw accents left
  in individual windows.
