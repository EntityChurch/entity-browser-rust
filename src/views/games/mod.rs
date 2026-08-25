//! The Apps window — the JS-apps platform surface.
//!
//! **One** window over **both** app-sets. The `games` / `apps` split is a
//! *storage* partition (`/{peer}/apps/{set}/…`, decided at ingest by
//! [`paths::set_for_type`]) and it stays exactly that; it stopped being a
//! *window* partition when Games and Apps merged here. A person looking for
//! something to open should not have to know which of two windows a publisher's
//! `type` field routed it to — they get one launcher and a row of category
//! chips ([`crate::apps::category`]).
//!
//! The launcher lists every set's apps in one grid and runs the selected one in
//! a **sandboxed iframe** (the entity-apps host contract).
//!
//! State model (all entity-backed):
//! - **catalog + bundles** live in the tree under `/{peer}/apps/{set}/…`
//!   ([`crate::apps`]), one catalog per set, each with its own source
//!   resolution ([`app_source`]); populated from a registered origin / publish
//!   ingest. With no apps present the launcher shows the empty state. (A baked
//!   demo token is seeded only under the e2e-only `demo-apps` feature — see
//!   [`Token`].)
//! - **which app is open, and which filter is selected** are window view-state
//!   ([`AppViewState`]) at the per-window state path — so both survive rebuilds
//!   and reload.
//! - **per-app save-state** is written by the host loop (`dom::games`) under
//!   `app_paths::app_save_path` (keyed by set, so ids don't collide).
//!
//! **A selection is `(set, id)`, never a bare id.** Ids are only unique *within*
//! a set — which is why `app_save_path` is set-keyed — so with both catalogs in
//! one window a bare id can name two different apps with two different saves.
//! It travels through the DOM event as `"{set}/{id}"` ([`parse_selection`]).

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use crate::apps::category;
use crate::apps::format::{AppBundle, AppCatalog, AppEntry};
#[cfg(feature = "demo-apps")]
use crate::apps::format::APP_CATALOG_TYPE;
use crate::apps::paths;
use crate::window_watch::WindowWatch;
use entity_entity::Entity;

/// `WindowEvent` name a launcher tile / back button emits. Value = `"{set}/{id}"`
/// naming the app to open, or `""` to return to the grid. Defined here (not in
/// the wasm-only `dom` module) so the native `handle_action` can match on it.
pub const SELECT_EVENT: &str = "select_game";

/// `WindowEvent` name a category chip emits. Value = the chip key
/// ([`crate::apps::category`]), `"all"` for no filter.
pub const FILTER_EVENT: &str = "filter_apps";

/// Open (`"1"`) or leave (`""`) the Saves panel.
pub const SAVES_PANEL_EVENT: &str = "apps_saves";
/// Expand one save's backup list. Value = `"{set}/{id}"`, `""` to collapse.
pub const SAVES_FOCUS_EVENT: &str = "apps_saves_focus";
/// Snapshot a save. Value = `"{set}/{id}"`.
pub const SAVES_BACKUP_EVENT: &str = "apps_saves_backup";
/// Copy a snapshot back over the live save. Value = `"{set}/{id}/{stamp}"`.
pub const SAVES_RESTORE_EVENT: &str = "apps_saves_restore";
/// Delete a snapshot. Value = `"{set}/{id}/{stamp}"`.
pub const SAVES_DROP_EVENT: &str = "apps_saves_drop";
/// Choose the peer that send / import talks to. Value = its peer-id.
pub const SAVES_TARGET_EVENT: &str = "apps_saves_target";
/// Offer a save to the chosen peer. Value = `"{set}/{id}"`.
pub const SAVES_SEND_EVENT: &str = "apps_saves_send";
/// Ask the chosen peer what saves it is offering. Value unused.
pub const SAVES_SCAN_EVENT: &str = "apps_saves_scan";
/// Pull one of those and file it. Value = the offer id.
pub const SAVES_IMPORT_EVENT: &str = "apps_saves_import";

/// Split a `"{set}/{id}/{stamp}"` backup reference. The id is taken from the
/// LEFT of the last `/` so an id containing a slash cannot swallow the stamp.
pub fn parse_backup_ref(value: &str) -> Option<(&str, &str, u64)> {
    let (set, rest) = value.split_once('/')?;
    let (id, stamp) = rest.rsplit_once('/')?;
    if set.is_empty() || id.is_empty() {
        return None;
    }
    Some((set, id, stamp.parse().ok()?))
}

/// Epoch milliseconds. Backup stamps are the only clock this window reads.
fn now_ms() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// The Saves panel's cross-peer half — **in memory, deliberately**.
///
/// Which peer you are sending to, and what a scan just found, are
/// action-in-progress rather than facts about the tree: they mean nothing after
/// a reload, and a tree-backed copy would be one more mirror to keep honest
/// (the same reasoning as `dial_markers`). The panel's durable state — which
/// panel is open, which row is expanded — stays in [`AppViewState`].
#[derive(Default)]
pub struct SavesUi {
    /// Peer chosen for send / import. Empty = the first connected one.
    pub target: String,
    /// What the last attempt said, already written for a person.
    pub status: String,
    /// `(offer id, bundle)` pairs the last scan found.
    pub found: Vec<(String, crate::apps::saves::SaveBundle)>,
    /// A list/pull is in flight — the button says so rather than looking inert.
    pub busy: bool,
}

/// Split a `"{set}/{id}"` selection. Returns `None` for `""` (the grid).
///
/// A **bare id with no `/`** is a selection persisted by the pre-merge Games or
/// Apps window; its set is unknown here and is resolved against the live
/// catalogs by the caller. Returning `("", id)` rather than refusing keeps a
/// returning user on the app they left open instead of bouncing them to the
/// grid for a reason they cannot see.
pub fn parse_selection(value: &str) -> Option<(&str, &str)> {
    if value.is_empty() {
        return None;
    }
    match value.split_once('/') {
        Some((set, id)) if !id.is_empty() => Some((set, id)),
        _ => Some(("", value)),
    }
}

/// A baked demo token for one app-set — an **e2e-only test fixture**
/// (`#[cfg(feature = "demo-apps")]`, off by default). It lets the
/// launcher→sandboxed-player e2e (Phase 2h.2) render + launch a deterministic
/// app without a live origin. Production builds do NOT bake these: a real
/// deployment serves apps off a registered origin (or ingests them at publish),
/// and with none present the launcher shows the honest empty state rather than
/// fake placeholders. The old ~730 KB "bake every game" seed was dropped,
/// and the per-set demo token was demoted to this e2e-only gate.
#[cfg(feature = "demo-apps")]
struct Token {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    saves: bool,
    /// The published **fine** category (`cards`, `utility`, …; empty = none).
    /// Set to what the real corpus publishes for this app, so the e2e exercises
    /// the actual chip fold ([`crate::apps::category`]) rather than a fixture
    /// that all lands in `other`.
    category: &'static str,
    /// Launcher-card emoji (empty = letter fallback).
    glyph: &'static str,
    html: &'static str,
    /// entity-apps `type` (empty = ordinary `srcdoc` app). [`paths::APP_TYPE_L5`]
    /// marks an L5 app the host delivers via `src` (`?app-host={id}`); its `html`
    /// is an unused placeholder (the payload is browser-rust in stripped mode).
    app_type: &'static str,
}

/// The minimal baked token(s) for a set (e2e-only; see [`Token`]). One small
/// game / one small app; other sets bake nothing.
#[cfg(feature = "demo-apps")]
fn demo_tokens(set: &str) -> &'static [Token] {
    match set {
        paths::GAMES_SET => &[Token {
            id: "war",
            name: "War", // i18n-ignore — e2e-only demo fixture, not in production builds
            description: "Flip the higher card to capture the pile — the classic luck game.", // i18n-ignore — e2e-only demo fixture
            saves: true,
            category: "cards", // the real corpus label for war/blackjack/solitaire
            glyph: "🃏",
            html: include_str!("fixtures/war.html"),
            app_type: "",
        }],
        paths::APPS_SET => &[
            Token {
                id: "calculator",
                name: "Calculator", // i18n-ignore — e2e-only demo fixture, not in production builds
                description: "A standard four-function calculator: +, −, ×, ÷, %, ±. Tap or type.", // i18n-ignore — e2e-only demo fixture
                saves: false,
                category: "utility", // the real corpus label for calculator/clock/timer
                glyph: "🧮",
                html: include_str!("fixtures/calculator.html"),
                app_type: "",
            },
            // L5 delivery smoke: browser-rust booted in stripped `?app-host=ping`
            // mode inside the sandboxed iframe, proving a WASM-peer payload can
            // round-trip `state` to the host (review G1/§4). `html` is an unused
            // placeholder — delivery is `src`, not `srcdoc`.
            Token {
                id: "ping",
                name: "Ping (L5)", // i18n-ignore — e2e-only demo fixture, not in production builds
                description: "L5 delivery smoke: a stripped browser-rust peer in a sandboxed iframe.", // i18n-ignore — e2e-only demo fixture
                saves: true,
                category: "", // no published category — exercises the `other` fallback
                glyph: "🛰",
                html: "<!doctype html><title>l5-placeholder</title>",
                app_type: paths::APP_TYPE_L5,
            },
            // NOTE: the built-in COMPUTE programs (Life / Snake / Asteroids) are
            // NOT baked here anymore. They are the production `EMBEDDED_PROGRAMS`
            // reachable from the Programs launcher — the one honest "run a program"
            // surface (D21). Duplicating them as Apps demo tokens meant the same
            // `?app-host=<key>` payload had two launch surfaces AND put a "Life"
            // card in two windows at once; the e2e L5 phases (2h.2c/d/e) launch
            // them through the Programs window instead. Only `ping` (the L5
            // delivery smoke) stays as an Apps fixture.
        ],
        _ => &[],
    }
}

/// Seed a set's demo token(s) into the peer's tree if absent (or if the catalog
/// carries an older type tag). Arm-aware via [`Peers::seed_write`]. No-op for a
/// set with no baked token. E2e-only ([`Token`]) — not compiled into release
/// builds, so production launchers are never seeded with fixtures.
#[cfg(feature = "demo-apps")]
pub fn ensure_demo_set(peers: &Peers, peer_id: &str, set: &str) {
    let tokens = demo_tokens(set);
    if tokens.is_empty() {
        return;
    }
    let catalog = AppCatalog {
        entries: tokens
            .iter()
            .map(|t| AppEntry {
                id: t.id.to_string(),
                name: t.name.to_string(),
                description: t.description.to_string(),
                saves: t.saves,
                category: (!t.category.is_empty()).then(|| t.category.to_string()),
                glyph: (!t.glyph.is_empty()).then(|| t.glyph.to_string()),
                app_type: (!t.app_type.is_empty()).then(|| t.app_type.to_string()),
                ..Default::default()
            })
            .collect(),
    };
    let entity = catalog.to_entity();
    // Reseed when the stored catalog is absent, a stale type tag, OR baked
    // demo content drifted (e.g. a fixture gained a glyph) — comparing the
    // content data, not just the type, so presentation tweaks take effect.
    // It's still a no-op once the store matches (content-addressed dedup).
    let up_to_date = peers
        .get_entity(peer_id, &paths::catalog_path(peer_id, set))
        .map(|e| e.entity_type == APP_CATALOG_TYPE && e.data == entity.data)
        .unwrap_or(false);
    if up_to_date {
        return;
    }
    peers.seed_write(peer_id, paths::catalog_path(peer_id, set), entity);
    for t in tokens {
        peers.seed_write(
            peer_id,
            paths::bundle_path(peer_id, set, t.id),
            AppBundle::new(t.html).to_entity(),
        );
    }
}

/// Seed the demo token of every app-set ([`paths::APP_SETS`]). E2e-only
/// ([`Token`]) — not compiled into release builds. (Bare publishes no longer
/// seed fixtures; an app-less publish emits no apps, matching the empty-state
/// contract.)
#[cfg(all(not(target_arch = "wasm32"), feature = "demo-apps"))]
pub fn ensure_demo_apps(peers: &Peers, peer_id: &str) {
    for set in paths::APP_SETS {
        ensure_demo_set(peers, peer_id, set);
    }
}

/// Resolve which peer's apps to display and the origin (if any) to fetch them
/// from, for a given set — the **live-consumer** source resolution.
///
/// **Foreign-first.** A deployment registers the publish peer's origin, and the
/// published apps live under THAT peer (`/{publish}/apps/{set}/…`), not under
/// the freshly-minted system peer. So a registered foreign origin wins over the
/// locally-baked demo token: we prefer one whose catalog we already hold (a
/// stable choice across frames), else the first registered origin (its catalog
/// will fetch). With **no** origins registered (plain dev / `make serve`) we
/// fall back to the local peer's baked token. `peer == me` origins are owned
/// content (already local) and skipped.
///
/// Returns `(apps_peer, Some(origin))` when fetching is possible, or
/// `(me, None)` for the local baked set.
pub fn app_source(peers: &Peers, me: &str, set: &str) -> (String, Option<String>) {
    let origins = crate::content_site::origins::list_origins(peers, me);
    // A foreign origin whose catalog we already hold → stable across frames.
    for (peer, origin) in &origins {
        if peer == me {
            continue;
        }
        if peers.get_entity(me, &paths::catalog_path(peer, set)).is_some() {
            return (peer.clone(), Some(origin.clone()));
        }
    }
    // Else the first foreign origin (its catalog fetches on first render).
    for (peer, origin) in origins {
        if peer != me {
            return (peer, Some(origin));
        }
    }
    (me.to_string(), None)
}

/// Per-window view-state: which app is open (`""` = the grid) and which
/// category chip is selected (`""`/`"all"` = no filter).
///
/// `selected` is a `"{set}/{id}"` pair ([`parse_selection`]) — see the module
/// doc on why a bare id is not enough once both sets share a window. A state
/// written by the pre-merge windows holds a bare id and still decodes; the
/// render resolves its set against the live catalogs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppViewState {
    pub selected: String,
    pub filter: String,
    /// `"saves"` = the Saves panel is up; `""` = the grid / player. A third
    /// view, not a modal: it replaces the window body, so it survives a reload
    /// like every other structural choice here.
    pub panel: String,
    /// Which save's backup list is expanded in the panel, `"{set}/{id}"`.
    pub focus: String,
}

/// Entity type for the embedded-app window view-state (app/state/ prefix).
pub const APP_VIEW_TYPE: &str = "app/state/games_view";

impl AppViewState {
    pub fn from_entity(entity: &Entity) -> Self {
        let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        let mut out = Self::default();
        if let Some(map) = value.as_map() {
            for (k, v) in map {
                match k.as_text() {
                    Some("selected") => out.selected = v.as_text().unwrap_or("").to_string(),
                    // Absent on a state written before the merge → `""`, which
                    // `category::passes` reads as "show everything".
                    Some("filter") => out.filter = v.as_text().unwrap_or("").to_string(),
                    Some("panel") => out.panel = v.as_text().unwrap_or("").to_string(),
                    Some("focus") => out.focus = v.as_text().unwrap_or("").to_string(),
                    _ => {}
                }
            }
        }
        out
    }

    pub fn to_entity(&self) -> Entity {
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![
            (
                entity_ecf::Value::Text("selected".into()),
                entity_ecf::text(&self.selected),
            ),
            (
                entity_ecf::Value::Text("filter".into()),
                entity_ecf::text(&self.filter),
            ),
            (
                entity_ecf::Value::Text("panel".into()),
                entity_ecf::text(&self.panel),
            ),
            (
                entity_ecf::Value::Text("focus".into()),
                entity_ecf::text(&self.focus),
            ),
        ]));
        Entity::new(APP_VIEW_TYPE, data).unwrap()
    }
}

/// What a live-consumer fetch targets — the catalog, or one bundle by id.
#[cfg(target_arch = "wasm32")]
enum FetchWhat {
    Catalog,
    Bundle(String),
}

#[cfg(target_arch = "wasm32")]
impl FetchWhat {
    /// In-flight de-dup key (scoped by set + kind).
    fn key(&self, set: &str) -> String {
        match self {
            FetchWhat::Catalog => format!("{set}:catalog"),
            FetchWhat::Bundle(id) => format!("{set}:bundle:{id}"),
        }
    }
}

/// The window's stable English type identifier — an identity string used for
/// registry lookup / persistence, never rendered as UI text. The display label
/// resolves through `window.apps`.
pub const TYPE_NAME: &str = "Apps"; // i18n-ignore — stable type identifier, not UI text

/// One app-set as the launcher sees it: where its catalog came from, and what
/// is in it. Each set resolves its source independently ([`app_source`]) — two
/// sets can legitimately come from two different publishing peers.
pub struct SetView {
    pub set: &'static str,
    pub apps_peer: String,
    pub origin: Option<String>,
    pub catalog: AppCatalog,
}

/// Resolve every set's source and read its catalog. Native-callable (no DOM),
/// so the merge, the filter and the legacy-selection resolution are all covered
/// by `make test` rather than only by the browser.
pub fn resolve_sets(peers: &Peers, me: &str) -> Vec<SetView> {
    paths::APP_SETS
        .iter()
        .map(|set| {
            let (apps_peer, origin) = app_source(peers, me, set);
            let catalog = peers
                .get_entity(me, &paths::catalog_path(&apps_peer, set))
                .map(|e| AppCatalog::from_entity(&e))
                .unwrap_or_default();
            SetView {
                set,
                apps_peer,
                origin,
                catalog,
            }
        })
        .collect()
}

/// Every set's entries as one list, tagged with the set they came from — the
/// grid's input. Set order is [`paths::APP_SETS`] and within a set the catalog's
/// own order, so the grid is stable across renders.
pub fn merged_entries(sets: &[SetView]) -> Vec<(&'static str, &AppEntry)> {
    sets.iter()
        .flat_map(|sv| sv.catalog.entries.iter().map(move |e| (sv.set, e)))
        .collect()
}

/// Resolve a persisted `selected` against the live catalogs.
///
/// Handles both forms: the `"{set}/{id}"` this window writes, and the **bare
/// id** written by the pre-merge Games / Apps windows — for which the set is
/// recovered by looking the id up, in [`paths::APP_SETS`] order. Returns `None`
/// (the grid) when nothing matches, which is also what a selection for an app
/// that has since been unpublished does.
pub fn resolve_selected<'a>(
    sets: &'a [SetView],
    selected: &str,
) -> Option<(&'a SetView, &'a AppEntry)> {
    let (want_set, want_id) = parse_selection(selected)?;
    for sv in sets {
        if !want_set.is_empty() && sv.set != want_set {
            continue;
        }
        if let Some(entry) = sv.catalog.entries.iter().find(|e| e.id == want_id) {
            return Some((sv, entry));
        }
    }
    None
}

/// The Apps window: one launcher over every app-set ([`paths::APP_SETS`]).
pub struct AppWindow {
    window_id: WindowId,
    peer_id: String,
    watch: WindowWatch,
    /// The Saves panel's transient half. Shared with the spawned send / scan /
    /// import tasks, which mark [`Self::watch`] themselves after mutating it —
    /// an in-memory render input owns its own dirty signal, or the panel shows
    /// a stale answer until something unrelated repaints it (AP21).
    saves_ui: std::rc::Rc<std::cell::RefCell<SavesUi>>,
    /// Whether a write under the save/backup prefixes may rebuild this window.
    /// Closed for exactly as long as a player is mounted: a running app writes
    /// its own save every few seconds, and a rebuild replaces its `<iframe>`,
    /// which restarts it at the start screen. Set in [`Self::render_dom`] — the
    /// one place that knows which of the three views is up. See
    /// [`crate::window_watch::RebuildGate`].
    saves_gate: crate::window_watch::RebuildGate,
    /// The host `message` listener for the current frame, owned for its
    /// lifetime; removed on rebuild / window drop so listeners don't stack.
    #[cfg(target_arch = "wasm32")]
    listener: std::cell::RefCell<Option<crate::dom::games::HostListener>>,
    /// Keys of in-flight live-consumer fetches so the render loop never
    /// re-spawns a fetch that's already running. Cleared on completion.
    #[cfg(target_arch = "wasm32")]
    fetching: std::rc::Rc<std::cell::RefCell<std::collections::HashSet<String>>>,
    /// Catalog refreshes already kicked this window-session. The catalog is
    /// re-fetched from the origin ONCE per open even when a cached copy exists,
    /// so a returning user whose durable store holds an OLDER catalog still
    /// sees newly-published apps. One-shot (not per-render) to avoid a storm.
    #[cfg(target_arch = "wasm32")]
    refreshed: std::cell::RefCell<std::collections::HashSet<String>>,
}

impl AppWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        Self {
            window_id,
            peer_id,
            watch: WindowWatch::new(),
            saves_ui: std::rc::Rc::new(std::cell::RefCell::new(SavesUi::default())),
            saves_gate: crate::window_watch::RebuildGate::open(),
            #[cfg(target_arch = "wasm32")]
            listener: std::cell::RefCell::new(None),
            #[cfg(target_arch = "wasm32")]
            fetching: std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashSet::new())),
            #[cfg(target_arch = "wasm32")]
            refreshed: std::cell::RefCell::new(std::collections::HashSet::new()),
        }
    }

    /// This window's view-state path.
    fn state_path(&self) -> String {
        crate::app_paths::window_state_path(
            crate::app_paths::APP_ID,
            &self.peer_id,
            self.window_id,
        )
    }

    /// This window's persisted view-state, or the default.
    fn view_state(&self, peers: &Peers) -> AppViewState {
        peers
            .get_entity(&self.peer_id, &self.state_path())
            .map(|e| AppViewState::from_entity(&e))
            .unwrap_or_default()
    }

    /// The peer send / import talks to: the explicit choice while it is still
    /// connected, else the first connected peer. Same resolution as File
    /// Transfer's `effective_target`, and for the same reason — a selection
    /// that silently survives its peer going away sends into the void.
    fn saves_target(&self, peers: &Peers) -> String {
        let chosen = self.saves_ui.borrow().target.clone();
        let connected = crate::connections::read_connections(peers);
        if !chosen.is_empty() && connected.iter().any(|p| p.remote_pid == chosen) {
            return chosen;
        }
        connected
            .first()
            .map(|p| p.remote_pid.clone())
            .unwrap_or_default()
    }

    /// Say something on the panel and repaint. Every cross-peer path ends here,
    /// including the failures: a button that reports nothing is one the user
    /// presses again.
    fn say(&self, message: impl Into<String>) {
        self.saves_ui.borrow_mut().status = message.into();
        self.watch.mark_dirty();
    }

    /// Snapshot the live save under a fresh timestamp.
    fn backup_save(&self, peers: &Peers, set: &str, id: &str) {
        let Some(writer) = peers.writer_handle_for(&self.peer_id) else {
            return;
        };
        let live = crate::app_paths::app_save_path(
            crate::app_paths::APP_ID,
            &self.peer_id,
            set,
            id,
        );
        let Some(ent) = peers.get_entity(&self.peer_id, &live) else {
            self.say(crate::i18n::t("saves.err_no_save", &[("app", id)]));
            return;
        };
        // The same bytes under a second timestamp is two rows differing only by
        // a clock, and one more presence binding nothing reclaims. Say so
        // rather than silently doing nothing, which reads as a dead button.
        if crate::apps::saves::already_backed_up(peers, &self.peer_id, set, id) {
            self.say(crate::i18n::t("saves.unchanged", &[("app", id)]));
            return;
        }
        writer.put(
            crate::app_paths::app_backup_path(
                crate::app_paths::APP_ID,
                &self.peer_id,
                set,
                id,
                now_ms(),
            ),
            ent,
        );
        self.say(crate::i18n::t("saves.backed_up", &[("app", id)]));
    }

    /// Copy a snapshot back over the live save.
    ///
    /// The backup entity is written to the live path **unchanged**, so the two
    /// share one content blob rather than the restore minting a second copy of
    /// bytes the store already has.
    fn restore_save(&self, peers: &Peers, set: &str, id: &str, stamp: u64) {
        let Some(writer) = peers.writer_handle_for(&self.peer_id) else {
            return;
        };
        let from = crate::app_paths::app_backup_path(
            crate::app_paths::APP_ID,
            &self.peer_id,
            set,
            id,
            stamp,
        );
        let Some(ent) = peers.get_entity(&self.peer_id, &from) else {
            // The backup list is rendered from the tree it lives in, so this is
            // reachable only if it went away between paint and click — say the
            // same thing as a missing save rather than inventing a second
            // sentence for a case a person will read once.
            self.say(crate::i18n::t("saves.err_no_save", &[("app", id)]));
            return;
        };
        writer.put(
            crate::app_paths::app_save_path(crate::app_paths::APP_ID, &self.peer_id, set, id),
            ent,
        );
        self.say(crate::i18n::t("saves.restored", &[("app", id)]));
    }

    /// Forget a snapshot, and reclaim its bytes if nothing else binds them.
    fn drop_backup(&self, peers: &Peers, set: &str, id: &str, stamp: u64) {
        let Some(writer) = peers.writer_handle_for(&self.peer_id) else {
            return;
        };
        let path = crate::app_paths::app_backup_path(
            crate::app_paths::APP_ID,
            &self.peer_id,
            set,
            id,
            stamp,
        );
        // Reclaim is **binding-safe** (`content_remove_if_unbound`), which is
        // what makes it safe to call when the live save was restored from this
        // very backup and still points at the same blob: the live binding keeps
        // it alive, and only a genuinely unreferenced blob goes.
        let hash = peers.get_entity(&self.peer_id, &path).map(|e| e.content_hash);
        writer.remove(path);
        if let Some(h) = hash {
            writer.content_remove(h);
        }
        self.say(crate::i18n::t("saves.backup_dropped", &[]));
    }

    /// The display name the catalogs give an app, else its id. Advisory — it
    /// rides along in a bundle so the far end can show a word rather than a
    /// slug, and is never what the far end files by.
    fn app_display_name(&self, peers: &Peers, set: &str, id: &str) -> String {
        resolve_sets(peers, &self.peer_id)
            .iter()
            .filter(|sv| sv.set == set)
            .find_map(|sv| {
                sv.catalog
                    .entries
                    .iter()
                    .find(|e| e.id == id)
                    .map(|e| e.name.clone())
            })
            .unwrap_or_else(|| id.to_string())
    }

    /// Offer this save to the chosen peer, who pulls it.
    ///
    /// **Push is not on the table**: writing a save into a stranger's tree
    /// would need them to have granted us a write, and a save arriving
    /// unasked-for could overwrite a game in progress. So this publishes an
    /// offer on OUR side and the other end takes it — the same posture as every
    /// other file this browser serves.
    #[cfg(target_arch = "wasm32")]
    fn send_save(&self, peers: &Peers, value: &str) {
        let Some((set, id)) = parse_selection(value) else {
            return;
        };
        let target = self.saves_target(peers);
        if target.is_empty() {
            self.say(crate::i18n::t("saves.err_no_peer", &[]));
            return;
        }
        let live =
            crate::app_paths::app_save_path(crate::app_paths::APP_ID, &self.peer_id, set, id);
        let Some(ent) = peers.get_entity(&self.peer_id, &live) else {
            self.say(crate::i18n::t("saves.err_no_save", &[("app", id)]));
            return;
        };
        let Some(dispatch) = peers.dispatch_handle(&self.peer_id) else {
            return;
        };
        let bundle = crate::apps::saves::SaveBundle {
            set: set.to_string(),
            id: id.to_string(),
            app_name: self.app_display_name(peers, set, id),
            state: crate::apps::format::AppSave::from_entity(&ent).state,
            saved_at_ms: now_ms(),
        };
        // The other end has to be able to REACH us to pull this, and a browser
        // peer has no listener — being reachable is something it does, not
        // something it is (`reach_keeper`). Registering the intent here is what
        // makes an offer to a peer we merely remember actually collectable.
        crate::reach_keeper::global().want(&self.peer_id, &target);

        let ui = self.saves_ui.clone();
        let dirty = self.watch.flag();
        let name = bundle.file_name();
        let app = bundle.app_name.clone();
        let bytes = bundle.to_bytes();
        ui.borrow_mut().busy = true;
        dirty.mark();
        wasm_bindgen_futures::spawn_local(async move {
            let said = match crate::file_offer::offer_file(&dispatch, &name, &bytes).await {
                Ok(_) => crate::i18n::t("saves.offered", &[("app", &app)]),
                Err(e) => crate::i18n::t("saves.err_offer", &[("why", &e)]),
            };
            let mut ui = ui.borrow_mut();
            ui.busy = false;
            ui.status = said;
            drop(ui);
            dirty.mark();
        });
    }

    /// Ask the chosen peer what saves it is offering, and decode them.
    ///
    /// The scan **pulls** each candidate rather than listing names: a name is
    /// the offerer's word for what a file is, and filing a save against the
    /// wrong app on that word is exactly the failure [`SaveBundle`] exists to
    /// prevent. Candidates are pre-filtered by suffix so an ordinary shared
    /// file is not fetched just to be discarded; anything that fails to decode
    /// is dropped quietly, because a peer may legitimately offer other things.
    #[cfg(target_arch = "wasm32")]
    fn scan_peer_saves(&self, peers: &Peers) {
        let target = self.saves_target(peers);
        if target.is_empty() {
            self.say(crate::i18n::t("saves.err_no_peer", &[]));
            return;
        }
        let Some(dispatch) = peers.dispatch_handle(&self.peer_id) else {
            return;
        };
        crate::reach_keeper::global().want(&self.peer_id, &target);
        let ui = self.saves_ui.clone();
        let dirty = self.watch.flag();
        ui.borrow_mut().busy = true;
        dirty.mark();
        wasm_bindgen_futures::spawn_local(async move {
            let mut found = Vec::new();
            let said = match crate::file_offer::list_offers(&dispatch, &target).await {
                Ok(offers) => {
                    for offer in offers
                        .iter()
                        .filter(|o| o.name.ends_with(crate::apps::saves::BUNDLE_SUFFIX))
                    {
                        // The pull is keyed by the blob hash the manifest names;
                        // `id()` is that hash's hex, which is what the DOM
                        // carries and what `import_save` looks a bundle up by.
                        if let Ok(raw) =
                            crate::file_offer::pull_offer(&dispatch, &target, &offer.blob).await
                        {
                            if let Some(b) = crate::apps::saves::SaveBundle::from_bytes(&raw) {
                                found.push((offer.id(), b));
                            }
                        }
                    }
                    // An empty answer is an answer, and it must be said: a
                    // silent no-op here is indistinguishable from a scan that
                    // never ran.
                    crate::i18n::t("saves.scanned", &[("n", &found.len().to_string())])
                }
                Err(e) => crate::i18n::t("saves.err_scan", &[("why", &e)]),
            };
            let mut ui = ui.borrow_mut();
            ui.busy = false;
            ui.found = found;
            ui.status = said;
            drop(ui);
            dirty.mark();
        });
    }

    /// File a scanned bundle as this peer's save for that app.
    ///
    /// **It backs up whatever it replaces first.** Importing is the one action
    /// here that destroys a save without naming it — you are thinking about the
    /// incoming one — so the outgoing one is snapshotted on the way past and
    /// stays in the backup list.
    fn import_save(&self, peers: &Peers, offer_id: &str) {
        let Some(bundle) = self
            .saves_ui
            .borrow()
            .found
            .iter()
            .find(|(id, _)| id == offer_id)
            .map(|(_, b)| b.clone())
        else {
            return;
        };
        let Some(writer) = peers.writer_handle_for(&self.peer_id) else {
            return;
        };
        let live = crate::app_paths::app_save_path(
            crate::app_paths::APP_ID,
            &self.peer_id,
            &bundle.set,
            &bundle.id,
        );
        let replaced = peers.get_entity(&self.peer_id, &live).is_some();
        if replaced {
            self.backup_save(peers, &bundle.set, &bundle.id);
        }
        writer.put(
            live,
            crate::apps::format::AppSave::new(&bundle.state).to_entity(),
        );
        self.say(crate::i18n::t(
            if replaced {
                "saves.imported_replacing"
            } else {
                "saves.imported"
            },
            &[("app", &bundle.app_name)],
        ));
    }

    /// Assemble and draw the Saves panel.
    ///
    /// The save list comes from the SAVE prefix, then names are joined in from
    /// the catalogs — never the other way round. A save whose app is no longer
    /// published still has a row (labelled by its id): an origin going away
    /// must not look like the user's data going away.
    #[cfg(target_arch = "wasm32")]
    fn render_saves(
        &self,
        container: &web_sys::Element,
        peers: &Peers,
        ctx: &crate::dom::DomCtx,
        view: &AppViewState,
        sets: &[SetView],
        back_label: &str,
    ) {
        let name_of = |set: &str, id: &str| -> String {
            sets.iter()
                .filter(|sv| sv.set == set)
                .find_map(|sv| {
                    sv.catalog
                        .entries
                        .iter()
                        .find(|e| e.id == id)
                        .map(|e| e.name.clone())
                })
                .unwrap_or_else(|| id.to_string())
        };

        let saves: Vec<_> = crate::apps::saves::list_all_saves(peers, &self.peer_id)
            .into_iter()
            .map(|row| {
                let name = name_of(&row.set, &row.id);
                (row, name)
            })
            .collect();

        let backups = match parse_selection(&view.focus) {
            Some((set, id)) if !set.is_empty() => {
                crate::apps::saves::list_backups(peers, &self.peer_id, set, id)
            }
            _ => Vec::new(),
        };

        let target = self.saves_target(peers);
        let peer_options: Vec<(String, String)> = crate::connections::read_connections(peers)
            .iter()
            .map(|p| {
                (
                    p.remote_pid.clone(),
                    crate::views::display_name(peers, &p.remote_pid),
                )
            })
            .collect();

        let ui = self.saves_ui.borrow();
        let found: Vec<_> = ui
            .found
            .iter()
            .map(|(offer_id, b)| {
                (
                    offer_id.clone(),
                    b.app_name.clone(),
                    b.set.clone(),
                    b.id.clone(),
                )
            })
            .collect();

        crate::dom::app_saves::render(
            container,
            ctx,
            &crate::dom::app_saves::SavesView {
                saves,
                focus: &view.focus,
                backups,
                peers: peer_options,
                target: &target,
                found,
                status: &ui.status,
                busy: ui.busy,
                back_label,
            },
        );
    }

    /// Native builds have no async runtime for these WASM-only UI paths.
    #[cfg(not(target_arch = "wasm32"))]
    fn send_save(&self, _peers: &Peers, _value: &str) {}

    #[cfg(not(target_arch = "wasm32"))]
    fn scan_peer_saves(&self, _peers: &Peers) {}

    /// The Apps window type — games and tools in one launcher. The former
    /// `Games` type is a legacy alias resolved by
    /// [`crate::window::canonical_window_type`], so a persisted workspace, a
    /// saved startup surface, or a shell verb naming `Games` still opens this.
    pub fn apps_window_type() -> WindowType {
        WindowType {
            name: TYPE_NAME, // i18n-ignore — identity key; display via window.apps
            description: "Run embedded self-contained HTML games and tools in a sandbox", // i18n-ignore — dead_code
            scope: crate::window::WindowScope::Peer,
            create: create_apps,
        }
    }

    /// Kick a live-consumer fetch (browser only) for one set's `catalog` or a
    /// `bundle`, caching the fetched entity into MY store at the **foreign
    /// peer's natural path** (`/{apps_peer}/apps/{set}/…`) — the same
    /// cache-at-natural-path shape as `precache_origin_sites`. In-flight guarded
    /// per `(set, kind)`, so the two sets' catalog fetches never de-dup together.
    #[cfg(target_arch = "wasm32")]
    fn ensure_fetched(
        &self,
        peers: &Peers,
        set: &'static str,
        apps_peer: &str,
        origin: &str,
        what: FetchWhat,
    ) {
        let key = what.key(set);
        if self.fetching.borrow().contains(&key) {
            return;
        }
        // Foreign content caches into MY store (the system peer's), at the
        // foreign path — route the writer by `self.peer_id`, never the
        // (unrouted) foreign peer.
        let Some(writer) = peers.writer_handle_for(&self.peer_id) else {
            tracing::warn!(peer = %self.peer_id, "apps: no writer handle — fetch skipped");
            return;
        };
        self.fetching.borrow_mut().insert(key.clone());
        let fetching = self.fetching.clone();
        // A clonable handle to THIS window's dirty flag. On a successful fetch we
        // mark it directly rather than relying on the store write firing a
        // watched-prefix subscription — the factory only watches the foreign
        // prefixes that existed at window-open, so a write to a peer whose origin
        // registered later would otherwise land silently with no re-render.
        let dirty = self.watch.flag();
        let origin = origin.to_string();
        let apps_peer = apps_peer.to_string();
        wasm_bindgen_futures::spawn_local(async move {
            use crate::content_site::http_poll::{self, FetchBinSource};
            let src = FetchBinSource;
            // Bounded backoff retry. The render loop is **dirty-gated**: a failed
            // fetch writes nothing, fires no subscription, and so never flips the
            // window dirty — without a retry the grid/app stays wedged until the
            // user happens to reopen the window (the live-proven "sometimes loads,
            // sometimes not" bug). Localhost never fails so it hid; a real CDN
            // hiccups. Retry transient failures here (capped exponential), and on
            // final give-up log loudly (D13) rather than fail silent.
            const MAX_ATTEMPTS: u32 = 5;
            let mut attempt: u32 = 0;
            loop {
                attempt += 1;
                let fetched = match &what {
                    FetchWhat::Catalog => {
                        http_poll::fetch_app_catalog(&src, &origin, &apps_peer, set)
                            .await
                            .map(|ent| (paths::catalog_path(&apps_peer, set), ent))
                    }
                    FetchWhat::Bundle(id) => {
                        http_poll::fetch_app_bundle(&src, &origin, &apps_peer, set, id)
                            .await
                            .map(|ent| (paths::bundle_path(&apps_peer, set, id), ent))
                    }
                };
                match fetched {
                    Ok((path, ent)) => {
                        writer.put(path, ent);
                        dirty.mark();
                        break;
                    }
                    Err(_) if attempt < MAX_ATTEMPTS => {
                        // 600ms, 1.2s, 2.4s, 4.8s — ~9s of coverage for a blip.
                        let backoff = 600u32.saturating_mul(1 << (attempt - 1)).min(5000);
                        sleep_ms(backoff as i32).await;
                    }
                    Err(e) => {
                        tracing::warn!(
                            peer = %apps_peer, key = %key, attempts = attempt, error = ?e,
                            "apps: live fetch failed after retries — reopen the window to retry"
                        );
                        break;
                    }
                }
            }
            fetching.borrow_mut().remove(&key);
        });
    }
}

/// Await a `setTimeout(ms)` — the async sleep the bounded-retry backoff needs.
/// The resolve callback is owned by the JS timer queue until it fires, so it
/// outlives the executor without an explicit `Closure` to keep alive.
#[cfg(target_arch = "wasm32")]
async fn sleep_ms(ms: i32) {
    use wasm_bindgen::JsCast;
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        if let Some(win) = web_sys::window() {
            let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(
                resolve.unchecked_ref(),
                ms,
            );
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

/// The window factory: build the window and register its watches — **every**
/// set's prefix under our own peer, the window state, and every set's prefix
/// under each routable foreign peer (the live-consumer re-render). Apps populate
/// from a registered origin / publish ingest; with none present the launcher
/// shows the empty state. (Under `demo-apps` — e2e only — baked fixtures are
/// seeded first so the launcher→player test has something to launch.)
///
/// Watching **both** sets is not a convenience: on the Worker arm a read lands
/// in the cache mirror only for a subscribed prefix, so a set this window reads
/// but does not watch is unreadable — the merged grid would silently show one
/// set. (`AGENTS.md`, the cache-mirror gotcha.)
fn create_apps(id: WindowId, peer_id: &str, pm: &Peers) -> Box<dyn WindowView> {
    #[cfg(feature = "demo-apps")]
    for set in paths::APP_SETS {
        ensure_demo_set(pm, peer_id, set);
    }
    let mut window = AppWindow::new(id, peer_id.to_string());
    for set in paths::APP_SETS {
        pm.watch_prefix(
            &mut window.watch,
            &window.peer_id,
            paths::set_prefix(&window.peer_id, set),
        );
    }
    pm.watch_prefix(
        &mut window.watch,
        &window.peer_id,
        crate::app_paths::window_state_path(crate::app_paths::APP_ID, &window.peer_id, id),
    );
    // Save-state and its backups. The player has always READ the live save at
    // render time without watching it — it worked because the write that put it
    // there seeded the mirror in the same session, and would have read empty
    // after a reload on the Worker arm. The Saves panel makes that latent hole
    // load-bearing (it lists the prefix, having written nothing), so both are
    // watched here.
    //
    // **Gated**, and that is not an optimization. This window is the only
    // WRITER of the save prefix as well as a reader: a running app persists on
    // every move, and marking dirty on that write rebuilds the section, which
    // runs `render_player`, which replaces the `<iframe>` — restarting the app
    // at its start screen about a second after every move. The subscription
    // stays live either way (the mirror keeps filling, which is what the Saves
    // panel needs on the Worker arm); only the rebuild is suppressed, and only
    // while a player is mounted. Backups are gated with them for symmetry:
    // nothing writes one while a player is up today, and an autosave-backup
    // would otherwise arrive at exactly this bug wearing a new name.
    for set in paths::APP_SETS {
        pm.watch_prefix_gated(
            &mut window.watch,
            &window.peer_id,
            crate::app_paths::app_saves_prefix(crate::app_paths::APP_ID, &window.peer_id, set),
            Some(window.saves_gate.clone()),
        );
        pm.watch_prefix_gated(
            &mut window.watch,
            &window.peer_id,
            crate::app_paths::app_backups_prefix(crate::app_paths::APP_ID, &window.peer_id, set),
            Some(window.saves_gate.clone()),
        );
    }
    // Live consumer: apps published under a registered origin land in MY store at
    // the foreign peer's natural `/{foreign}/apps/{set}/` path. Watch each
    // routable foreign peer's set prefixes so a fetched catalog/bundle flips dirty
    // and re-renders. An origin registered AFTER this window opens needs a re-open
    // — same bound as the content-site window.
    for (foreign, _origin) in crate::content_site::origins::list_origins(pm, &window.peer_id) {
        if foreign == window.peer_id {
            continue;
        }
        for set in paths::APP_SETS {
            pm.watch_prefix(
                &mut window.watch,
                &window.peer_id,
                paths::set_prefix(&foreign, set),
            );
        }
    }
    Box::new(window)
}

impl WindowView for AppWindow {
    fn title(&self) -> String {
        crate::i18n::t("window.apps", &[])
    }

    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, peers: &Peers) {
        let Action::WindowEvent { event, value, .. } = action else {
            return;
        };
        match event.as_str() {
            // -- view state: four fields, ONE entity --------------------------
            //
            // Each of these edits the same state entity, so each must carry the
            // others forward — writing only the field that changed would reset
            // the filter every time an app is opened, drop the open app every
            // time a chip is pressed, and collapse the backup list on every
            // action taken inside it.
            SELECT_EVENT | FILTER_EVENT | SAVES_PANEL_EVENT | SAVES_FOCUS_EVENT => {
                let mut st = self.view_state(peers);
                match event.as_str() {
                    SELECT_EVENT => st.selected = value.clone(),
                    FILTER_EVENT => st.filter = value.clone(),
                    SAVES_PANEL_EVENT => {
                        st.panel = if value.is_empty() { String::new() } else { "saves".into() };
                        // Leaving the panel forgets which row was expanded;
                        // coming back to a half-open list you did not leave open
                        // reads as the panel remembering the wrong thing.
                        if st.panel.is_empty() {
                            st.focus.clear();
                        }
                    }
                    _ => st.focus = value.clone(),
                }
                peers.seed_write(&self.peer_id, self.state_path(), st.to_entity());
                self.watch.mark_dirty();
            }

            // -- local save management ---------------------------------------
            SAVES_BACKUP_EVENT => {
                if let Some((set, id)) = parse_selection(value) {
                    self.backup_save(peers, set, id);
                }
            }
            SAVES_RESTORE_EVENT => {
                if let Some((set, id, stamp)) = parse_backup_ref(value) {
                    self.restore_save(peers, set, id, stamp);
                }
            }
            SAVES_DROP_EVENT => {
                if let Some((set, id, stamp)) = parse_backup_ref(value) {
                    self.drop_backup(peers, set, id, stamp);
                }
            }

            // -- cross-peer --------------------------------------------------
            SAVES_TARGET_EVENT => {
                let mut ui = self.saves_ui.borrow_mut();
                ui.target = value.clone();
                // A different peer's offers are a different answer; keeping the
                // old list beside a new target is how someone imports a save
                // from a peer they just switched away from.
                ui.found.clear();
                ui.status.clear();
                drop(ui);
                self.watch.mark_dirty();
            }
            SAVES_SEND_EVENT => self.send_save(peers, value),
            SAVES_SCAN_EVENT => self.scan_peer_saves(peers),
            SAVES_IMPORT_EVENT => self.import_save(peers, value),

            _ => {}
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        peers: &Peers,
        ctx: &crate::dom::DomCtx,
    ) {
        use crate::apps::format::AppSave;

        // No player is mounted from here until one is, so save writes may
        // rebuild again. Opening it BEFORE the listener drops is deliberate:
        // dropping the listener flushes any pending save, and that write should
        // reach whichever view we are about to render (the grid, or the Saves
        // panel that is about to list it). Re-closed at the bottom if this
        // render mounts a player.
        self.saves_gate.set_open(true);

        // Drop any stale listener before (re)building the section.
        if let Some(old) = self.listener.borrow_mut().take() {
            crate::dom::games::remove_listener(&old);
        }

        let grid_title = crate::i18n::t("window.apps", &[]);
        let empty_msg = crate::i18n::t("apps.empty", &[]);

        // Every set's catalog, each with its own source resolution (foreign-first;
        // local baked token when no origins). Reads route by `self.peer_id` (MY
        // store, where foreign content is cached) at the resolved peer's path.
        // Saves stay under `self.peer_id`.
        let sets = resolve_sets(peers, &self.peer_id);

        // Refresh each set's catalog from its origin ONCE per window-open, even
        // if a cached copy already renders. A returning user's durable store may
        // hold an OLDER catalog (e.g. an earlier publish's smaller set); this
        // used to fetch only when absent, so apps added after the first visit
        // NEVER appeared. The fetch overwrites the cached copy and flips the
        // watch dirty → re-render if it changed. One-shot per open (the
        // `refreshed` set), so it can't storm the render loop. Absent caches
        // still fetch here too (insert returns true the first time regardless).
        for sv in &sets {
            if let Some(o) = &sv.origin {
                let refresh_key = FetchWhat::Catalog.key(sv.set);
                if self.refreshed.borrow_mut().insert(refresh_key) {
                    self.ensure_fetched(peers, sv.set, &sv.apps_peer, o, FetchWhat::Catalog);
                }
            }
        }

        let view = self.view_state(peers);

        // The Saves panel is the window's third view and takes precedence over
        // a selected app: you reach it from the grid, and while it is up no
        // player is mounted (so nothing is writing a save under the panel that
        // is showing it).
        if view.panel == "saves" {
            self.render_saves(container, peers, ctx, &view, &sets, &grid_title);
            return;
        }

        // The launcher grid unless an app is selected AND its bundle is present;
        // fetch the bundle on click-through when it isn't yet cached locally.
        let picked = resolve_selected(&sets, &view.selected);
        let bundle = picked.and_then(|(sv, entry)| {
            let b = peers
                .get_entity(
                    &self.peer_id,
                    &paths::bundle_path(&sv.apps_peer, sv.set, &entry.id),
                )
                .map(|e| AppBundle::from_entity(&e));
            if b.is_none() {
                if let Some(o) = &sv.origin {
                    self.ensure_fetched(
                        peers,
                        sv.set,
                        &sv.apps_peer,
                        o,
                        FetchWhat::Bundle(entry.id.clone()),
                    );
                }
            }
            b
        });

        let (Some(bundle), Some((sv, entry))) = (bundle, picked) else {
            let merged = merged_entries(&sets);
            let chips = category::chips_for(merged.iter().map(|(s, e)| (*s, *e)));
            crate::dom::games::render_grid(
                container,
                ctx,
                &crate::dom::games::GridView {
                    entries: &merged,
                    chips: &chips,
                    filter: &view.filter,
                    saves_entry: true,
                    title: &grid_title,
                    empty_msg: &empty_msg,
                },
            );
            return;
        };

        // Read the live save once: its parsed state seeds the app, its content
        // hash seeds the retention ring (so the host loop's first reclaim drops
        // the prior session's superseded blob — see `apps::save_retention`).
        let save_ent = peers.get_entity(
            &self.peer_id,
            &crate::app_paths::app_save_path(
                crate::app_paths::APP_ID,
                &self.peer_id,
                sv.set,
                &entry.id,
            ),
        );
        let init_save_hash = save_ent.as_ref().map(|e| e.content_hash);
        let init_state = save_ent
            .map(|e| AppSave::from_entity(&e).state)
            .unwrap_or_default();

        // Delivery: an L5 app (WASM-entity-peer payload) can't inline its wasm as
        // `srcdoc`, so it loads browser-rust in stripped mode via `src`
        // (`index.html?app-host={id}`, the app id as the program), same-origin
        // under `dist/`. Everything else stays self-contained `srcdoc`. (review G1)
        let delivery = if paths::is_l5_app(entry.app_type.as_deref()) {
            crate::dom::games::AppDelivery::Src(format!("index.html?app-host={}", entry.id))
        } else {
            crate::dom::games::AppDelivery::Srcdoc
        };

        let cfg = crate::dom::games::GamesHostConfig {
            peer_id: self.peer_id.clone(),
            set: sv.set.to_string(),
            // One window now, so the back button reads "← Apps" whichever set
            // the running app came from.
            back_label: grid_title.clone(),
            // The app's preferred-size hint (catalog `size`), or None → the
            // per-set default (games square-capped, tools fill).
            size: entry.size,
            game_id: entry.id.clone(),
            game_name: entry.name.clone(),
            bundle_html: bundle.html,
            delivery,
            init_state,
            init_save_hash,
        };
        let listener = crate::dom::games::render_player(container, peers, ctx, &cfg);
        *self.listener.borrow_mut() = listener;

        // A player is live: from here the save prefix is write-only to this
        // window, and a rebuild would replace the iframe we just mounted. The
        // running app's own saves must not do that.
        self.saves_gate.set_open(false);
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for AppWindow {
    fn drop(&mut self) {
        if let Some(l) = self.listener.borrow_mut().take() {
            crate::dom::games::remove_listener(&l);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_apps_window_type_is_peer_scoped() {
        let a = AppWindow::apps_window_type();
        assert_eq!(a.name, TYPE_NAME);
        assert!(matches!(a.scope, crate::window::WindowScope::Peer));
    }

    /// The merge removed the `Games` window type. Anything holding that key —
    /// a persisted workspace, a baked startup surface, a shell verb — must
    /// still land on the launcher rather than resolving to no factory and
    /// silently opening nothing.
    #[test]
    fn the_retired_games_key_still_opens_the_launcher() {
        assert_eq!(crate::window::canonical_window_type("Games"), TYPE_NAME);
        assert!(crate::window_registry::standard_window_types()
            .iter()
            .any(|t| t.name == TYPE_NAME));
        assert!(
            !crate::window_registry::standard_window_types()
                .iter()
                .any(|t| t.name == "Games"),
            "Games must no longer be registered — the alias is the only path"
        );
    }

    #[test]
    fn view_state_round_trips() {
        let s = AppViewState {
            selected: "games/chess".into(),
            filter: "games".into(),
            panel: "saves".into(),
            focus: "games/chess".into(),
        };
        assert_eq!(AppViewState::from_entity(&s.to_entity()), s);
        assert_eq!(s.to_entity().entity_type, APP_VIEW_TYPE);
    }

    /// A state written by the pre-merge windows carries `selected` and no
    /// `filter`. It must decode rather than reset to the grid, and the absent
    /// filter must read as "no filter" — not as a chip key nothing matches.
    #[test]
    fn a_pre_merge_view_state_decodes_with_no_filter() {
        let legacy = Entity::new(
            APP_VIEW_TYPE,
            entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![(
                entity_ecf::Value::Text("selected".into()),
                entity_ecf::text("chess"),
            )])),
        )
        .unwrap();
        let st = AppViewState::from_entity(&legacy);
        assert_eq!(st.selected, "chess");
        assert_eq!(st.filter, "");
        assert!(category::passes(&st.filter, paths::GAMES_SET, Some("cards")));
    }

    #[test]
    fn a_backup_reference_names_a_set_an_id_and_a_stamp() {
        assert_eq!(
            parse_backup_ref("games/chess/1755630000000"),
            Some(("games", "chess", 1_755_630_000_000))
        );
        assert_eq!(parse_backup_ref("games/chess"), None);
        assert_eq!(parse_backup_ref("games/chess/not-a-stamp"), None);
        assert_eq!(parse_backup_ref(""), None);
    }

    /// Drive backup → restore → delete through `handle_action` — the real entry
    /// point, not the helpers underneath it — because the panel's buttons are
    /// the only way a user reaches any of this.
    #[tokio::test]
    async fn a_save_can_be_snapshotted_rolled_back_and_the_snapshot_dropped() {
        use crate::apps::format::AppSave;
        use crate::apps::saves;

        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let a = AppWindow::apps_window_type();
        let mut w = (a.create)(1, &pid, &peers);

        let save_path =
            crate::app_paths::app_save_path(crate::app_paths::APP_ID, &pid, paths::GAMES_SET, "chess");
        let fire = |w: &mut Box<dyn WindowView>, peers: &Peers, event: &str, value: &str| {
            w.handle_action(
                &Action::WindowEvent {
                    window_id: 1,
                    event: event.to_string(),
                    value: value.to_string(),
                },
                peers,
            );
        };

        // Move one: a position worth keeping.
        peers.seed_write(&pid, save_path.clone(), AppSave::new("position-A").to_entity());
        fire(&mut w, &peers, SAVES_BACKUP_EVENT, "games/chess");
        let backups = saves::list_backups(&peers, &pid, paths::GAMES_SET, "chess");
        assert_eq!(backups.len(), 1, "one snapshot: {backups:?}");
        let stamp = backups[0].stamp_ms;

        // Move two: a position worth undoing.
        peers.seed_write(&pid, save_path.clone(), AppSave::new("position-B").to_entity());
        assert_eq!(
            AppSave::from_entity(&peers.get_entity(&pid, &save_path).unwrap()).state,
            "position-B"
        );

        fire(
            &mut w,
            &peers,
            SAVES_RESTORE_EVENT,
            &format!("games/chess/{stamp}"),
        );
        assert_eq!(
            AppSave::from_entity(&peers.get_entity(&pid, &save_path).unwrap()).state,
            "position-A",
            "restore must put the snapshot back over the live save"
        );
        // …and the snapshot is still there: restoring is a copy, not a move, so
        // it can be done twice.
        assert_eq!(
            saves::list_backups(&peers, &pid, paths::GAMES_SET, "chess").len(),
            1
        );

        fire(
            &mut w,
            &peers,
            SAVES_DROP_EVENT,
            &format!("games/chess/{stamp}"),
        );
        assert!(saves::list_backups(&peers, &pid, paths::GAMES_SET, "chess").is_empty());
        // Dropping a backup must not touch the live save — even when the live
        // save was restored FROM it and the two share one content blob.
        assert_eq!(
            AppSave::from_entity(&peers.get_entity(&pid, &save_path).unwrap()).state,
            "position-A",
            "deleting the snapshot must not take the live save's bytes with it"
        );
    }

    /// The panel is a third view and its openness is durable. A reload must put
    /// the user back in it, and the four state fields must not overwrite each
    /// other — each event edits one field of one entity.
    #[tokio::test]
    async fn the_panel_and_the_filter_survive_each_other() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let a = AppWindow::apps_window_type();
        let mut w = (a.create)(1, &pid, &peers);
        let state_path =
            crate::app_paths::window_state_path(crate::app_paths::APP_ID, &pid, 1);
        let read = |peers: &Peers| {
            AppViewState::from_entity(&peers.get_entity(&pid, &state_path).unwrap())
        };
        let fire = |w: &mut Box<dyn WindowView>, peers: &Peers, event: &str, value: &str| {
            w.handle_action(
                &Action::WindowEvent {
                    window_id: 1,
                    event: event.to_string(),
                    value: value.to_string(),
                },
                peers,
            );
        };

        fire(&mut w, &peers, FILTER_EVENT, "games");
        fire(&mut w, &peers, SAVES_PANEL_EVENT, "1");
        fire(&mut w, &peers, SAVES_FOCUS_EVENT, "games/chess");
        let st = read(&peers);
        assert_eq!((st.filter.as_str(), st.panel.as_str()), ("games", "saves"));
        assert_eq!(st.focus, "games/chess");

        // Leaving drops the expanded row but keeps the filter you chose.
        fire(&mut w, &peers, SAVES_PANEL_EVENT, "");
        let st = read(&peers);
        assert_eq!(st.panel, "");
        assert_eq!(st.focus, "", "the expanded row is not remembered across a leave");
        assert_eq!(st.filter, "games", "the chip choice is");
    }

    #[test]
    fn a_selection_names_a_set_and_an_id() {
        assert_eq!(parse_selection("games/chess"), Some(("games", "chess")));
        assert_eq!(parse_selection(""), None);
        // The pre-merge form: no set, resolved against the live catalogs.
        assert_eq!(parse_selection("chess"), Some(("", "chess")));
    }

    /// Default (no `demo-apps`): opening a launcher window must NOT seed fake
    /// apps — with no origin/ingest the tree stays empty so the UI shows the
    /// honest empty state.
    #[cfg(not(feature = "demo-apps"))]
    #[tokio::test]
    async fn factory_does_not_seed_fake_apps() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let a = AppWindow::apps_window_type();
        let _ = (a.create)(1, &pid, &peers);
        for set in paths::APP_SETS {
            assert!(
                peers
                    .get_entity(&pid, &paths::catalog_path(&pid, set))
                    .is_none(),
                "set {set} must not be seeded with a fake catalog"
            );
        }
    }

    /// Under `demo-apps` (e2e only): opening each launcher seeds its baked
    /// fixture so the launcher→player e2e has a deterministic app.
    #[cfg(feature = "demo-apps")]
    #[tokio::test]
    async fn the_one_factory_seeds_every_set() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();

        let a = AppWindow::apps_window_type();
        let _ = (a.create)(1, &pid, &peers);

        let gcat = peers
            .get_entity(&pid, &paths::catalog_path(&pid, paths::GAMES_SET))
            .map(|e| AppCatalog::from_entity(&e))
            .expect("games catalog seeded");
        assert!(gcat.entries.iter().any(|e| e.id == "war"));
        assert!(peers
            .get_entity(&pid, &paths::bundle_path(&pid, paths::GAMES_SET, "war"))
            .is_some());

        let acat = peers
            .get_entity(&pid, &paths::catalog_path(&pid, paths::APPS_SET))
            .map(|e| AppCatalog::from_entity(&e))
            .expect("apps catalog seeded");
        assert!(acat.entries.iter().any(|e| e.id == "calculator"));
    }

    /// The merge in one assertion: ONE window, and the grid it renders holds
    /// entries from BOTH sets. Before the merge no single window could.
    #[cfg(feature = "demo-apps")]
    #[tokio::test]
    async fn one_window_shows_both_sets_and_tags_each_entry_with_its_set() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let a = AppWindow::apps_window_type();
        let _ = (a.create)(1, &pid, &peers);

        let sets = resolve_sets(&peers, &pid);
        let merged = merged_entries(&sets);
        let war = merged
            .iter()
            .find(|(_, e)| e.id == "war")
            .expect("the games-set fixture is in the merged grid");
        let calc = merged
            .iter()
            .find(|(_, e)| e.id == "calculator")
            .expect("the apps-set fixture is in the merged grid");
        assert_eq!(war.0, paths::GAMES_SET);
        assert_eq!(calc.0, paths::APPS_SET);

        // …and the chips fold those into the coarse row, counts included.
        let chips = category::chips_for(merged.iter().map(|(s, e)| (*s, *e)));
        let keys: Vec<&str> = chips.iter().map(|c| c.key).collect();
        assert_eq!(
            keys,
            vec![category::ALL, category::GAMES, category::TOOLS, category::OTHER],
            "war -> Games, calculator -> Tools, ping (no category) -> Other"
        );
    }

    /// A selection is resolved against the live catalogs — including the bare
    /// id the pre-merge windows persisted, whose set has to be recovered by
    /// lookup. Without this a returning user is bounced to the grid.
    #[cfg(feature = "demo-apps")]
    #[tokio::test]
    async fn a_pre_merge_bare_id_still_resolves_to_its_app() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let a = AppWindow::apps_window_type();
        let _ = (a.create)(1, &pid, &peers);
        let sets = resolve_sets(&peers, &pid);

        let (sv, entry) = resolve_selected(&sets, "war").expect("bare id resolves");
        assert_eq!((sv.set, entry.id.as_str()), (paths::GAMES_SET, "war"));

        let (sv, entry) = resolve_selected(&sets, "apps/calculator").expect("qualified resolves");
        assert_eq!((sv.set, entry.id.as_str()), (paths::APPS_SET, "calculator"));

        // A set-qualified id that does not exist in THAT set is not silently
        // served from the other one.
        assert!(resolve_selected(&sets, "apps/war").is_none());
        // An unpublished app falls back to the grid rather than a blank player.
        assert!(resolve_selected(&sets, "games/gone").is_none());
    }

    #[test]
    fn app_source_is_local_when_no_origins() {
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        assert_eq!(app_source(&peers, &me, paths::GAMES_SET), (me.clone(), None));
        assert_eq!(app_source(&peers, &me, paths::APPS_SET), (me, None));
    }

    #[test]
    fn app_source_prefers_a_registered_foreign_origin() {
        use crate::content_site::origins;
        let peers = Peers::new_direct();
        let me = peers.primary_peer_id().to_string();
        origins::set_origin(&peers, &me, "PUBPEER", "http://pub.example");
        assert_eq!(
            app_source(&peers, &me, paths::APPS_SET),
            ("PUBPEER".to_string(), Some("http://pub.example".to_string()))
        );
    }

    #[cfg(feature = "demo-apps")]
    #[test]
    fn ensure_demo_apps_seeds_each_set() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        ensure_demo_apps(&peers, &pid);
        for set in paths::APP_SETS {
            assert!(
                peers
                    .get_entity(&pid, &paths::catalog_path(&pid, set))
                    .is_some(),
                "set {set} seeded"
            );
        }
    }
}
