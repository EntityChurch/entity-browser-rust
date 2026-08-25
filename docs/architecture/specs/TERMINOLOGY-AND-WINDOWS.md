# Terminology & Window Taxonomy — canonical names, one home per concept

Status: **canonical** (2026-07-13). This is the single source of truth for what
we *call* the two system peers and for *where* a window/feature belongs. If a UI
string or a new window disagrees with this doc, the doc wins — fix the string.

Motivation: we had **four-plus** names for the one native backend peer
(`Native backend peer`, `Native system peer`, `system (native)`,
`System Backend`, `System backend — native store`) and three for the frontend
one. That is a bug. One name each, everywhere.

---

## 1. Peer terminology (the two system peers)

Tori runs **two** always-on system peers. They have exactly one user-facing name
each. Use these verbatim in every window, hint, card, label, and log line the
operator reads.

| Canonical name | What it is | Runtime | Storage |
|---|---|---|---|
| **System peer** | This app's own system peer — the in-app one you're looking at. | Main thread (or worker), in the WebView/browser. | IndexedDB (or in-memory / OPFS per build). |
| **System backend** | The native-process system peer that runs in the desktop backend and shares the device's files. | Native process (Tauri desktop only). | Native filesystem store. |

**Rule:** never coin a third phrasing. In particular these are **deprecated
aliases — do not use in UI**:

- ~~Native backend peer~~, ~~Native system peer~~, ~~system (native)~~,
  ~~System Peer (Native)~~ → **System backend**
- ~~System peer (this app)~~, ~~in-app system peer~~ → **System peer**

Disambiguation is carried by the two distinct names, so "System peer" needs no
qualifier — the backend has its own name.

### Where the names come from in code

- Friendly labels are produced in **one** place per surface, not sprinkled:
  `views/access_log/model.rs::label_for` (Access Log) and
  `views/system_peers/model.rs` → `dom/system_peers.rs` (the System Overview
  window's peer cards). Both return the strings in the table above.
- `peer_display::PeerDescriptor::role_name()` still returns the terse
  `"system"` / `"system (native)"` — that is a **descriptor/log** string for
  *other* peers, not a user-facing name for these two (the surfaces above
  intercept them with the canonical names). Leave it; don't surface it raw.
- Internal identifiers that are **not** user-facing and are load-bearing for
  back-compat keep their existing spelling: the backend label constant
  `SYSTEM_BACKEND_LABEL = "system-backend"` and the legacy window key
  `"System Backend"` (now only an alias arm — see §2).

### Module map (so the code matches the names)

The source modules were realigned to the terminology (no more "System Backend
window" module that's actually System Overview):

| Module | Is | Types |
|---|---|---|
| `views/system_overview/` + `dom/system_overview.rs` | **The** System Overview governance window. | `SystemOverviewWindow` / `SystemOverviewModel` / `SystemOverviewOutput` |
| `views/system_peers/` + `dom/system_peers.rs` | The system-peer identity cards + posture folded into that window's top. | `SystemPeersModel` / `SystemPeersOutput` |
| `SystemBackend*` names (`SYSTEM_BACKEND_LABEL`, `ensure_system_backend`, `SystemBackendConnect`, `drain_system_backend_connect`) | The **System backend peer** and its IPC/connection — legitimately "backend". | (unchanged) |

Rule of thumb: `system_overview` = the *window*; `system_peers` = the *cards*;
`system_backend` (in a name) = the *backend peer*, never the window.

---

## 2. Window taxonomy — where a window belongs

A window has a `WindowScope` (`src/window.rs`). This is the deciding question for
*where a feature goes*:

| Scope | Binds to | Purpose | Windows |
|---|---|---|---|
| **System** | Always the **System peer** (infra). | **Govern the system** — the System peer + the System backend, connections, keys, settings, audit. Not about one user peer's data. | System Overview, Peers, Peer Connections, Key Manager, Settings, Event Log, Access Log, Storage, File Transfer |
| **Peer** | The **user-selected peer**. | Operate on *that peer's* data/tree. | Entity Tree, Execute Console, Query Console, Shell, Chain Trace, Path Tap, Wire Recorder, Content Stream, Content Site, Site Editor, Knowledge Base |

**Placement rule for a new feature:**

1. Is it about **one selected peer's data** (its tree, its dispatches, its
   content)? → **Peer** scope.
2. Is it about **the system as a whole** — the two system peers, the links
   between devices, credentials, app config, or a cross-peer audit? →
   **System** scope, bound to the System peer.

**System windows are not peer-specific.** "System Overview" governs *both* the
System peer and the System backend; the Access Log shows *every* peer's audit
rows with a **Peer** column/dropdown to pick whose log you're reading (the
System peer's own dispatches vs who reached into the System backend). These are
governance surfaces, not a per-peer view.

### The one governance window

- **Title + type key:** both `"System Overview"`. The key was renamed from the
  legacy `"System Backend"` on 2026-07-13. Per-window state keys on the numeric
  window id (`window_state_path`), not the type name, so nothing was stranded;
  any stale **persisted reference** to the old key (a boot-surface `window_type`,
  a deployment config) still resolves through `window::canonical_window_type`, the
  back-compat alias. **How to rename a window type key:** update `window_type().name`
  + `WindowView::type_name()` + the `window_registry` category list, then add one
  arm to `canonical_window_type`. That's it — it is not permanent debt.

---

## 3. Open questions (decide before building on them)

- **File Transfer scope.** Currently **System**-scoped, but it's really a
  peer-to-peer capability ("send this peer a file"), and arguably belongs with
  developer/peer tooling rather than system governance. Flagged, not resolved —
  pick a home before the next file-transfer change and record it here.
- **Access Log durability.** Ephemeral per session today; a durable,
  subscribable audit is the documented next step. When it lands, it stays one
  window with the Peer axis — do not fork per-peer log windows.

---

## 4. The ratchet

Any new window or user-facing peer reference: use §1's names and §2's placement
rule. If you find a fifth name for a system peer, it's a regression — fix it and,
if a new distinction is genuinely needed, add a row here first.
