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
- **`THEMES: &[Theme] = &[DARK, LIGHT, SEPIA, NEON]`** — the **built-in**
  registry. `DARK` is first = the default; `SEPIA` (warm paper) and
  `NEON` (green-phosphor terminal, mono UI font) are worked examples of
  §7's retune-every-key rule (a test now pins every builtin to DARK's
  exact key set + order). **Adding a built-in is one entry here** (§7).
  Beside it sits the **user-theme runtime registry** (§7.1): user
  themes are `Box::leak`ed on registration into true `&'static Theme`s
  (bounded — one leak per explicit Save), so the whole `&'static`
  resolution layer (`site_token_value`, `doc_css::Frozen`, the exporter)
  is untouched. **Iterate `all_themes()`** (built-ins + user, sorted),
  never `THEMES` directly, or user themes are skipped.
- **`lookup(name) -> &Theme`** — built-in or user; unknown id → `DARK`.
- **`root_block(theme) -> String`** — builds `:root{ … }`. Besides the
  token values it emits two plain properties: `color-scheme` (native
  `<select>` popup / scrollbars / caret render in the theme's scheme —
  the WebKitGTK dropdown trap) and `accent-color: var(--accent)` (native
  **checkbox/radio glyphs** follow the theme accent instead of the UA
  default blue). Both are inherited, so `:root` covers the shadow root.
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

### 3.1 Manifest site theme — the container-scope precedence rule (S-T2)

A site's manifest may declare its **own** theme: `"theme": "<registered
name>"` (`PUBLISH-INGEST-FORMAT.md` §2). It only ever defines *what
"Site's theme" means for that site* — the three modes above keep exactly
their meaning, and the user's explicit choice always wins:

| Mode | Without manifest theme | With manifest theme |
|---|---|---|
| `"site"` *(default)* | the `var()` fallbacks | **that theme's frozen `--site-*` values** (unknown name → warn once, fallbacks) |
| `"system"` / strict | as above | **unchanged — the manifest is ignored** |

Mechanics (`DESIGN-MANIFEST-SITE-THEME.md` is the full rationale):

- **`registered(name) -> Option<&Theme>`** — strict registry lookup; unlike
  `lookup` it does **not** fall back to `DARK` (a manifest name is outside
  input; unknown must degrade to today's look, loudly, never restyle).
- **`site_token_value(theme, site_token)`** — the ONE resolution rule
  (theme's value for the aliased app token, else the site default) shared
  by the strict-override `:root` block, the manifest container block, and
  the static exporter — the three can never disagree on a color.
- **`site_container_block(name) -> Option<String>`** — bare `--site-X:v;…`
  declarations for a registered name; `None` + a **once-per-session warn**
  for an unknown one.
- The block installs as **inline custom properties on the site's own
  wrapper element**, *never* `:root` — container properties override
  inherited `:root` values, which is exactly why the install is **gated on
  mode `"site"`** and why the gate lives in the *render output*
  (`SiteRenderOutput::site_theme_css`), not the renderer: the overlay
  rebuilds only on output-equality change, so the mode must be part of the
  output or a Settings flip would leave stale container vars defeating the
  strict override. `site_appearance_current()` (the LS mirror, read per
  frame) supplies the mode.
- Container scope also means two differently-themed sites coexist, the
  window's directory rail keeps the default palette, and cleanup is
  structural (the wrapper is rebuilt per render).
- **Static export parity:** the exporter freezes the site's *effective own*
  palette — the manifest theme's values when declared & registered, else
  the `SITE_TOKENS` defaults — through the same S-T1 rule table
  (`doc_css::PaletteMode::Frozen(Option<&Theme>)`). Index pages (cross-site
  surfaces) keep the default. User appearance modes never export.
- Site-supplied CSS / raw palette values stay **out** (security posture,
  §10): the manifest string is only a *key into the app's own table* — no
  site-supplied byte ever reaches a stylesheet.

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
"Always X" override, `lookup`, the **manifest site-theme field** (a site
can declare `"theme": "foo"` the moment `FOO` is registered — §3.1), the
static exporter's frozen palette, and the e2e all pick it up from the
registry. No renderer or wiring changes.

## 7.1 User-defined themes (the Theme Editor)

Landed 2026-07-15 — `DESIGN-USER-THEMES.md` is the decision record.

- **In-app path:** the **Theme Editor** window (System menu group):
  duplicate any registered theme under a new name, edit every token in
  grouped tables (hex values get a native color-picker swatch synced into
  the authoritative text field; fonts are three more rows — the fonts
  control rider), **live preview** on every keystroke, then Save. Delete
  is refused with the reason while the theme is the current chrome theme
  or site override.
- **Mechanics:** an owned `UserThemeSpec` is validated
  (`validate_theme_name`: `[a-z0-9-]`, ≤40 chars, and **reserved**:
  built-in names + the appearance modes `site`/`system`) and leaked into
  the runtime registry (`register_user_theme` / `unregister_user_theme` /
  `user_theme_names` / `all_themes`). Everything downstream — dropdowns,
  "Always X", manifest addressability, static export — flows from the
  registry exactly as §7 promises.
- **Persistence:** one entity per theme at
  `app/entity-browser/themes/{name}` (`user_themes.rs`: CBOR round-trip,
  `save_theme`/`delete_theme`), written L1 on the system peer. The
  registry is a **rebuildable projection** of that prefix: the app holds
  one lifetime watch (`UserThemes::sync`, a per-frame atomic check) that
  reconciles register/unregister on change, warn-and-skips malformed
  entities, and then `reinstall_current()` re-derives both live surfaces
  (edited theme recolors live; a theme deleted elsewhere falls back dark).
- **No-flash boot:** a user theme isn't registered until the tree syncs,
  so `apply_and_persist`/`apply_site_appearance` also mirror the
  **computed CSS** to localStorage (`entity_theme_css` /
  `entity_site_theme_css`); `install_root`/`install_site_root` install
  the mirror verbatim for an unregistered name, and the first registry
  sync self-heals it. The mirror is a paint hint, never the record.
- **Live preview** never touches the registry or the tree:
  `install_preview` rebuilds `#theme-vars` from the DOM draft
  (`root_block_from_pairs`); save/revert/load re-install the real theme.
- Editor **edit buffers live in the DOM** (`data-token` inputs, read back
  at preview/save); only structural state (loaded theme, status, draft
  revision) persists at the window-state path.

### 7.2 Where themes live — built-in vs user (the storage model)

Two kinds, two homes, deliberately NOT merged:

| | Built-in (`dark`/`light`/`sepia`/`neon`) | User-defined |
|---|---|---|
| Storage | **compiled into the app** (`THEMES` in `theme_tokens.rs`) | **tree entities** on the system peer (`app/entity-browser/themes/{name}`) |
| Scope | every profile, every deployment, identical | this profile only (travels with the peer's store) |
| Updates | app update replaces them | user edits them |
| Delete / rename | impossible (names reserved) | Theme Editor |
| Boot availability | frame one, always | after the themes-prefix sync (CSS mirror covers the gap) |

Built-ins are **never seeded into the tree** — on purpose. Seeding
defaults as tree entities would fork them per profile (an app update
couldn't fix a shipped palette), invite edit-the-builtin drift, and
create a migration/version-skew problem for zero gain. "Customize a
built-in" is instead **duplicate-and-edit** in the Theme Editor: the
copy is a normal user theme, owned by the profile, and the pristine
built-in stays selectable beside it. The `scheme` field is the one
non-color: browsers render native widgets in exactly two modes
(dark/light), so every theme — built-in or user — declares which mode
its palette sits closest to.

---

## 8. Surface map — tokenized vs. intentionally raw

**Tokenized (themes):**
- Chrome: `src/dom/style.rs`, `src/dom/theme.rs` consts, `index.html`
  `<style>`, and ~19 view/banner files (`var(--token, #literal)`).
- Overlay: `src/dom/content_site.rs` (`RESPONSIVE_CSS` + every inline
  style) and `src/dom/site_directory.rs` (the directory rail rows).

**Intentionally raw (NOT bugs — documented decisions):**
- **Emitted static-export CSS** — `src/content_site/static_export.rs`
  `page_css()` + the demo SVG. Published sites carry their **own** palette
  to *other people's* browsers with no runtime token layer — but since the
  S-T1 reconciliation the emitted literals are **derived** from the same
  sources the app uses (`doc_css` frozen form + `SITE_TOKENS` defaults via
  `doc_css::frozen`), not a parallel hand-maintained sheet. Only the
  live-mirror banner keeps its own fixed palette (a distinct notice
  surface, self-contained by design).
- **Semantic icon accents** — the directory rail's bookmark gold
  (`#e8c34a`) and keep-offline green (`#5fc27e`) **on-states**; their
  off-states *are* tokenized (`--site-text-faint-2`). Like syntax colors,
  these read on both themes and are conventionally theme-agnostic.
- **The fast-paint "connecting…" toast** (`src/boot_fast_paint.rs`) — a
  fixed translucent badge with its **own** dark `rgba(0,0,0,.55)`
  background, so its `#bbb` text reads on any page theme. (The fast-paint
  *content* render uses the tokenized `content_site::render` path; the
  feature is also gated off today.)
- **Pre-boot / self-contained surfaces** (`index.html`): the recovery
  page, the crash screen, and the update banner carry their **own** fixed
  dark palette (own bg + own text, so they read on any theme) — they can
  render before the theme system exists, like the fast-paint toast.
- **QR codes** (`#000`/`#fff`) — never theme (scannability).

The window-internal "long tail" (shell prompt/listing colors, scanner,
event/chain/wire log text, peer badges, peer-table grays) was tokenized in
the 2026-07 theming-reconciliation pass — `tools/ui-lint.sh` holds raw hex
at the documented survivors above; anything new fails `make lint`.

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
- **Manifest site theme (S-T2)** — native: `site_container_block`
  registered/unknown + value-identity with the strict `:root` block
  (`theme_tokens.rs`); mode gating (`views/content_site/model.rs
  site_theme_css_gates_on_registry_and_mode`); exporter per-site freeze +
  index default + unknown fallback (`static_export.rs`); themed frozen
  doc rules structural-equal to live (`doc_css.rs`); ingest carries
  registered and unknown names verbatim (`ingest.rs`). e2e (Phase 19c-t,
  the bundled demo-notes site declares `"theme": "light"`): light paints
  the companion's wrapper while the primary demo stays default, strict
  "Always Dark" wins live over the open themed site, "Site's theme"
  restores it — with clean-panic asserts throughout.
- **User themes (§7.1)** — native: registry register/replace/unregister +
  reserved-name validation + `site_token_value` through a user theme
  (`theme_tokens.rs`); entity round-trip (order-preserving CBOR array),
  boot-load / deleted-drop / malformed-skip sync, delete-refused-while-
  in-use (`user_themes.rs`); editor lifecycle create→save→delete, packed
  save parse, in-use flag, revision bump (`views/theme_editor/model.rs`).
  e2e (Phase 26.8): editor create → live preview rewrites `#theme-vars`
  from the draft → save → the Settings dropdown offers + applies it →
  the CSS boot mirror is written → **reload** (registry re-syncs from the
  tree on the Worker arm; selection + palette survive) → load in a fresh
  editor → delete propagates to every registry-driven dropdown.
- Verified green (S-T2 landing): native (788) · clippy + ui-lint · wasm ·
  e2e-worker 13/13.

---

## 10. Deferred / future (the seams are left open)

- **Fonts as a control** — the tokens exist (`--font-ui`/`--font-mono`/
  `--fs-base`); no UI to change them independent of the theme yet.
- ~~**Per-site themes**~~ — **landed** (S-T2, 2026-07-15): the manifest
  `theme` field names a registered theme, applied container-scoped in
  "Site's theme" mode — §3.1 is the precedence rule;
  `docs/architecture/reviews/DESIGN-MANIFEST-SITE-THEME.md` the rationale.
  Site-supplied CSS stays out permanently. The still-open extension is
  shape (b) of that design — site-supplied token *values* — which slots in
  behind the same field without breaking any published site.
- ~~**Custom / user themes**~~ — **landed** (2026-07-15): tree-persisted
  themes merged into the registry via leak-on-save, with a Theme Editor
  window — §7.1; `DESIGN-USER-THEMES.md` is the rationale. The "Always X"
  catalog generalized exactly as anticipated.
- ~~**Dev-guide "Theming" pattern note**~~ — landed: DEVELOPER-GUIDE
  "Styling & theming a window" points here; the boundary rule is §11.
- **Boot tree→theme reconcile** — the LS mirror covers the normal flow; a
  cross-browser / imported profile shows the default until re-selected.
  Optional: read `SettingsState.theme`/`.site_appearance` once peers boot
  and re-install. *(Partially absorbed by §7.1: for user themes the
  registry sync re-installs + re-mirrors after every tree change; the
  built-in-theme half of the gap remains.)*
- ~~**Tokenize the window-internal long tail**~~ — done in the 2026-07
  theming-reconciliation pass (§8); `tools/ui-lint.sh` holds the line.

---

## 11. The style-system boundary — where a style lives (the ONE rule)

Four styling homes exist **by design**; the ambiguity between them was
itself the reinvention driver (UI audit G2). This section is the boundary.
When adding or changing any style, it goes in exactly one place:

| Home | Owns | Never holds |
|---|---|---|
| **`src/theme_tokens.rs`** | color/font/scheme **values**: the `THEMES` registry, `SITE_TOKENS`, `STATUS_*`. The only file where a themable value is *defined*. | selectors, layout, widget styles |
| **`src/dom/style.rs`** (`DOM_STYLES`) | chrome **layout classes** for the shadow root: window frames, palette, grid/flex scaffolding, responsive rules — things addressed by *class*. | color literals (reference `var(--token, #literal)`), per-widget one-offs |
| **`src/dom/theme.rs`** | shared **widget-level inline-style consts** (`BTN_*`, `INPUT`, `SELECT`, `TREE_*`, the `SP_*` spacing scale) consumed by `dom/components.rs` atoms and views. | new colors (tokens only), window-specific styles |
| **`src/content_site/doc_css.rs`** | the **content-document** rules (`.cs-doc` family) — one table rendering the live overlay/preview form AND the exporter's frozen form. | chrome styles of any kind |

**Views hold only bespoke, fully-tokenized one-offs** — a style that is
genuinely singular to that window, with every color a
`var(--token, #literal)` and every spacing step from `theme::SP_*`. The
moment a second window wants it, it is promoted to `theme.rs` /
`components.rs` (the `tree_row` rule: never a third copy).
`tools/ui-lint.sh` ratchets the tail (`make lint` fails on new raw atoms,
style literals above baseline, or raw hex).

Decision path for "where does this style go?":

1. A **color/font value** → a token (`theme_tokens.rs`), then reference it.
2. Styling **rendered markdown/content** → `doc_css.rs`.
3. A **widget look** (button, input, row, chip) → an existing atom
   (`components.rs` + `theme.rs`); extend the atom, don't fork it.
4. Chrome **layout** addressed by class → `style.rs`.
5. Truly window-specific → inline in the view, fully tokenized, ready for
   promotion.
