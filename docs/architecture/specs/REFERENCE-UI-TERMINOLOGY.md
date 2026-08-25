# REFERENCE — UI Terminology & Copy Style

Canonical words and copy rules for every user-facing string in entity-browser-rust.
**Check here before you name a window, write a label, or word a message** — so we
say the same thing the same way everywhere, and so 30 translations aren't built on
incoherent English. Established 2026-07-19 during the i18n coverage audit
(`docs/plans/AUDIT-I18N-COVERAGE-GAP-2026-07-19.md`), which found ~500 user-facing
strings and the inconsistencies catalogued at the end of this file.

Authoritative for wording. Design/layout rules live in `REFERENCE-UI-DESIGN.md`;
theming in `REFERENCE-THEMING.md`; the i18n mechanics in `DESIGN-I18N-L10N.md`.

---

## 1. Core concept glossary (the words that carry meaning)

| Term | Means | NOT |
|---|---|---|
| **peer** | An entity-system node — a software identity that owns a tree and routes dispatch. Local or remote. | a machine (that's a *device*) |
| **device** | A physical remote machine you pair and connect to. Hardware. | a peer (a device *hosts* peers) |
| **System peer** | The in-app kernel/system entity node (governs auth, share). | the native process |
| **System backend** | The native desktop process + its on-disk store, linked to the System peer over IPC. | the System peer |
| **site** | A content-addressed collection of pages, browsable in Site Mode. | a page |
| **page** | One content entity within a site. | a site |
| **article** | One Knowledge Base entry. | a page (KB uses *article*) |
| **grant** | The authorization record a device holds (what it may do). | the act (that's *authorize*) |
| **authorize** | The action that creates a grant. | the record (that's a *grant*) |
| **share** | The folder/tree a peer exposes for a connected device to browse & pull. | — |

**Peer vs device — the rule:** a user *pairs* and *connects* to **devices**; the
system *routes to* and *acts as* **peers**. "A remote **peer** called this
**device**" is correct — the software identity (peer) reached the hardware (device).

**Peer kinds** (the create-peer picker) — keep the badge/description pairing and the
parallel structure:

| Badge | Full description | Storage |
|---|---|---|
| This tab · temporary | Main thread of this tab, in-memory. Temporary — cleared when you reload. | — |
| This tab · saved | Main thread of this tab, saved to IndexedDB. Survives reload. | IndexedDB |
| Background · temporary | A background Web Worker, in-memory. Temporary — cleared when you reload. | — |
| Background · saved | A background Web Worker, saved to OPFS. Survives reload. | OPFS |
| Native app · saved | A separate native desktop process with its own on-disk store. Saved. | SQLite |

> User-facing arm words are **"this tab"** (main thread) and **"background"** (Web
> Worker) and **"native app"** (desktop). `Direct` / `Worker` are *internal* arm
> names — don't surface them to end users.

---

## 2. Canonical window names (Title Case — proper names)

Source of truth: `window_registry::window_groups()`. These are keyed
`window.<slug>` and localized via `i18n::window_title`. **Never invent a variant.**

- **Apps & Content:** Games · Apps · Site Browser · Site Creator · Knowledge Base
- **System:** System Overview · Settings · Theme Editor · Peers · Peer Connections ·
  File Transfer · Key Manager · Storage · Access Log
- **Developer:** Entity Tree · Shell · Execute Console · Query Console · Event Log ·
  Chain Trace · Path Tap · Wire Recorder · Content Stream

---

## 3. Copy style rules

1. **Casing.** *Title Case* only for window titles and proper nouns (Site Mode).
   **Sentence case** for everything else — buttons, section headers, labels, hints,
   messages. ("Save page", "Add peer", "New article", "Refresh disk usage".)
2. **Ellipsis is `…` (U+2026), never `...`.** In-progress states: "Loading…",
   "Scanning…", "Starting camera…".
3. **Create verbs:**
   - **New X** — the affordance that *starts* creating a top-level item (opens a
     form/editor): New site, New article, New theme.
   - **Create X** / **Save** — the button that *commits* it.
   - **Add X** — insert into an existing collection (no creation form): Add peer,
     Add page, Add folder.
4. **Plurals go through a plural message key — never `(s)` or `capabilit{}`.**
   English has plural rules too; use `peer.count`-style keys so every language
   (incl. English) inflects correctly. (`{} peer(s)` → reuse `peer.count`.)
5. **Acronyms are kept verbatim, untranslated, in every language:** IndexedDB,
   OPFS, SQLite, IDB, QR, URI, JSON, DOM, HTML, WASM. You don't translate "OPFS".
6. **Voice:** second person, active, present. "your peers", "this device". A
   control says exactly what it does ("Publish" → toast "Published").
7. **Errors** state what went wrong and the way forward, no apology, no vagueness:
   "Couldn't reach peer '{}' — no route is registered for it." Keep them
   translatable (a user hitting an unreadable error in the wrong language is worse
   than the error).
8. **Dev-facing exceptions stay English** (not translated): shell/console command
   output, value-type renderings, internal `{e:?}` diagnostics, the crash overlay,
   the pre-WASM recovery page. Mark them `// i18n-ignore`.

---

## 4. Coherency defects found (fix during extraction)

The audit surfaced these inconsistencies. Fix each as its string is routed through
`t()` (unless already fixed — see §5):

- **Casing:** `New Article` / `Edit Article` (KB editor headers) → sentence case
  `New article` / `Edit article`, matching `+ New article`.
- **Create verbs:** confirm each site/page/theme flow follows §3.3 (New = open,
  Create/Save = commit, Add = insert). `New site` (disclosure) + `Create site`
  (submit) is correct under the rule; leave it.
- **Ad-hoc plurals → plural keys:** `{} peer(s)` (peer-mgmt, system-peers) reuse
  `peer.count`; `1 boot + {} dedicated worker(s)` needs a `worker.count` key;
  `{} — {} capabilit{} observed` needs a `capability.count` key.
- **Acronym casing:** always `IndexedDB`, `OPFS`, `SQLite` (not `Opfs`, `Sqlite`).

---

## 5. Fixed in this pass

- **Ellipsis normalized** across the `scanner.*` keys (`...` → `…`) in `EN` and all
  30 overlays — a shipped inconsistency (the rest of the catalog already used `…`).
