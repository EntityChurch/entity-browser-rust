//! Embedded-app windows — the JS-apps platform surface.
//!
//! One generic [`AppWindow`] drives **two** registered window types over the
//! same machinery, differing only by their **app-set** (`paths`):
//! - **Games** (`games` set) — canvas games.
//! - **Apps** (`apps` set) — non-game tools (calculator, calendar, …).
//!
//! Both list a set's apps in a launcher grid and run the selected one in a
//! **sandboxed iframe** (the entity-apps host contract).
//!
//! State model (all entity-backed):
//! - **catalog + bundles** live in the tree under `/{peer}/apps/{set}/…`
//!   ([`crate::apps`]); populated from a registered origin / publish ingest.
//!   With no apps present the launcher shows the empty state. (A baked demo
//!   token is seeded only under the e2e-only `demo-apps` feature — see
//!   [`Token`].)
//! - **which app is open** is window view-state ([`AppViewState`]) at the
//!   per-window state path — so it survives rebuilds and reload.
//! - **per-app save-state** is written by the host loop (`dom::games`) under
//!   `app_paths::app_save_path` (keyed by set, so ids don't collide).

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use crate::apps::format::{AppBundle, AppCatalog};
#[cfg(feature = "demo-apps")]
use crate::apps::format::{AppEntry, APP_CATALOG_TYPE};
use crate::apps::paths;
use crate::window_watch::WindowWatch;
use entity_entity::Entity;

/// `WindowEvent` name a launcher tile / back button emits. Value = the app id
/// to open, or `""` to return to the grid. Defined here (not in the wasm-only
/// `dom` module) so the native `handle_action` can match on it.
pub const SELECT_EVENT: &str = "select_game";

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
    /// Launcher-card emoji (empty = letter fallback).
    glyph: &'static str,
    html: &'static str,
}

/// The minimal baked token(s) for a set (e2e-only; see [`Token`]). One small
/// game / one small app; other sets bake nothing.
#[cfg(feature = "demo-apps")]
fn demo_tokens(set: &str) -> &'static [Token] {
    match set {
        paths::GAMES_SET => &[Token {
            id: "war",
            name: "War",
            description: "Flip the higher card to capture the pile — the classic luck game.",
            saves: true,
            glyph: "🃏",
            html: include_str!("fixtures/war.html"),
        }],
        paths::APPS_SET => &[Token {
            id: "calculator",
            name: "Calculator",
            description: "A standard four-function calculator: +, −, ×, ÷, %, ±. Tap or type.",
            saves: false,
            glyph: "🧮",
            html: include_str!("fixtures/calculator.html"),
        }],
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
                glyph: (!t.glyph.is_empty()).then(|| t.glyph.to_string()),
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

/// Per-window view-state: which app is currently open (`""` = the grid).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppViewState {
    pub selected: String,
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
                if k.as_text() == Some("selected") {
                    out.selected = v.as_text().unwrap_or("").to_string();
                }
            }
        }
        out
    }

    pub fn to_entity(&self) -> Entity {
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![(
            entity_ecf::Value::Text("selected".into()),
            entity_ecf::text(&self.selected),
        )]));
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

/// The grid title + empty-state message for a set.
fn set_labels(set: &str) -> (&'static str, &'static str) {
    match set {
        paths::APPS_SET => ("Apps", "No apps available yet."),
        _ => ("Games", "No games available yet."),
    }
}

/// One embedded-app window, parameterized by its app-set. Games and Apps are
/// both this type with a different `set`.
pub struct AppWindow {
    window_id: WindowId,
    peer_id: String,
    /// The app-set this window shows (`games` / `apps`).
    set: &'static str,
    watch: WindowWatch,
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
    pub fn new(window_id: WindowId, peer_id: String, set: &'static str) -> Self {
        Self {
            window_id,
            peer_id,
            set,
            watch: WindowWatch::new(),
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

    /// The Games window type (the `games` set).
    pub fn games_window_type() -> WindowType {
        WindowType {
            name: "Games",
            description: "Play embedded self-contained HTML games in a sandbox",
            scope: crate::window::WindowScope::Peer,
            // `create` is a bare fn pointer (can't capture `set`), so each set
            // gets its own non-capturing factory delegating to `create_set`.
            create: |id, peer_id, pm| create_set(id, peer_id, pm, paths::GAMES_SET),
        }
    }

    /// The Apps window type (the `apps` set — non-game tools).
    pub fn apps_window_type() -> WindowType {
        WindowType {
            name: "Apps",
            description: "Run embedded self-contained HTML apps (tools) in a sandbox",
            scope: crate::window::WindowScope::Peer,
            create: |id, peer_id, pm| create_set(id, peer_id, pm, paths::APPS_SET),
        }
    }

    /// Kick a live-consumer fetch (browser only) for this set's `catalog` or a
    /// `bundle`, caching the fetched entity into MY store at the **foreign
    /// peer's natural path** (`/{apps_peer}/apps/{set}/…`) — the same
    /// cache-at-natural-path shape as `precache_origin_sites`. In-flight guarded.
    #[cfg(target_arch = "wasm32")]
    fn ensure_fetched(&self, peers: &Peers, apps_peer: &str, origin: &str, what: FetchWhat) {
        let key = what.key(self.set);
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
        let set = self.set;
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

/// Shared window factory for both sets: build the window and register its
/// watches (own set prefix + window state + each routable foreign peer's set
/// prefix for the live-consumer re-render). Apps populate from a registered
/// origin / publish ingest; with none present the launcher shows the empty
/// state. (Under `demo-apps` — e2e only — a baked fixture is seeded first so
/// the launcher→player test has something to launch.)
fn create_set(
    id: WindowId,
    peer_id: &str,
    pm: &Peers,
    set: &'static str,
) -> Box<dyn WindowView> {
    #[cfg(feature = "demo-apps")]
    ensure_demo_set(pm, peer_id, set);
    let mut window = AppWindow::new(id, peer_id.to_string(), set);
    pm.watch_prefix(
        &mut window.watch,
        &window.peer_id,
        paths::set_prefix(&window.peer_id, set),
    );
    pm.watch_prefix(
        &mut window.watch,
        &window.peer_id,
        crate::app_paths::window_state_path(crate::app_paths::APP_ID, &window.peer_id, id),
    );
    // Live consumer: apps published under a registered origin land in MY store at
    // the foreign peer's natural `/{foreign}/apps/{set}/` path. Watch each
    // routable foreign peer's set prefix so a fetched catalog/bundle flips dirty
    // and re-renders. An origin registered AFTER this window opens needs a re-open
    // — same bound as the content-site window.
    for (foreign, _origin) in crate::content_site::origins::list_origins(pm, &window.peer_id) {
        if foreign == window.peer_id {
            continue;
        }
        pm.watch_prefix(
            &mut window.watch,
            &window.peer_id,
            paths::set_prefix(&foreign, set),
        );
    }
    Box::new(window)
}

impl WindowView for AppWindow {
    fn title(&self) -> String {
        set_labels(self.set).0.to_string()
    }

    fn type_name(&self) -> &'static str {
        set_labels(self.set).0
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, peers: &Peers) {
        if let Action::WindowEvent { event, value, .. } = action {
            if event == SELECT_EVENT {
                // Persist which app is open (or "" to return to the grid).
                let st = AppViewState {
                    selected: value.clone(),
                };
                peers.seed_write(&self.peer_id, self.state_path(), st.to_entity());
                self.watch.mark_dirty();
            }
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

        // Drop any stale listener before (re)building the section.
        if let Some(old) = self.listener.borrow_mut().take() {
            crate::dom::games::remove_listener(&old);
        }

        let (grid_title, empty_msg) = set_labels(self.set);

        // Which peer's apps to show + where to fetch them from (foreign-first;
        // local baked token when no origins). Reads route by `self.peer_id` (MY
        // store, where foreign content is cached) at the resolved peer's path.
        // Saves stay under `self.peer_id`.
        let (apps_peer, origin) = app_source(peers, &self.peer_id, self.set);

        let catalog_ent =
            peers.get_entity(&self.peer_id, &paths::catalog_path(&apps_peer, self.set));
        // Refresh the catalog from the origin ONCE per window-open, even if a
        // cached copy already renders. A returning user's durable store may hold
        // an OLDER catalog (e.g. an earlier publish's smaller set); this used to
        // fetch only when absent, so apps added after the first visit NEVER
        // appeared. The fetch overwrites the cached copy and flips the watch
        // dirty → re-render if it changed. One-shot per open (the `refreshed`
        // set), so it can't storm the render loop. Absent caches still fetch
        // here too (insert returns true the first time regardless).
        if let Some(o) = &origin {
            let refresh_key = FetchWhat::Catalog.key(self.set);
            if self.refreshed.borrow_mut().insert(refresh_key) {
                self.ensure_fetched(peers, &apps_peer, o, FetchWhat::Catalog);
            }
        }
        let catalog = catalog_ent
            .map(|e| AppCatalog::from_entity(&e))
            .unwrap_or_default();
        let selected = peers
            .get_entity(&self.peer_id, &self.state_path())
            .map(|e| AppViewState::from_entity(&e).selected)
            .unwrap_or_default();

        // The launcher grid unless an app is selected AND its bundle is present;
        // fetch the bundle on click-through when it isn't yet cached locally.
        let bundle = if selected.is_empty() {
            None
        } else {
            let b = peers
                .get_entity(&self.peer_id, &paths::bundle_path(&apps_peer, self.set, &selected))
                .map(|e| AppBundle::from_entity(&e));
            if b.is_none() {
                if let Some(o) = &origin {
                    self.ensure_fetched(peers, &apps_peer, o, FetchWhat::Bundle(selected.clone()));
                }
            }
            b
        };

        let Some(bundle) = bundle else {
            crate::dom::games::render_grid(container, ctx, &catalog.entries, grid_title, empty_msg);
            return;
        };

        let entry = catalog.entries.iter().find(|e| e.id == selected);
        let name = entry
            .map(|e| e.name.clone())
            .unwrap_or_else(|| selected.clone());
        let size = entry.and_then(|e| e.size);
        // Read the live save once: its parsed state seeds the app, its content
        // hash seeds the retention ring (so the host loop's first reclaim drops
        // the prior session's superseded blob — see `apps::save_retention`).
        let save_ent = peers.get_entity(
            &self.peer_id,
            &crate::app_paths::app_save_path(
                crate::app_paths::APP_ID,
                &self.peer_id,
                self.set,
                &selected,
            ),
        );
        let init_save_hash = save_ent.as_ref().map(|e| e.content_hash);
        let init_state = save_ent
            .map(|e| AppSave::from_entity(&e).state)
            .unwrap_or_default();

        let cfg = crate::dom::games::GamesHostConfig {
            peer_id: self.peer_id.clone(),
            set: self.set.to_string(),
            set_label: grid_title.to_string(),
            // The app's preferred-size hint (catalog `size`), or None → the
            // per-set default (games square-capped, tools fill).
            size,
            game_id: selected.clone(),
            game_name: name,
            bundle_html: bundle.html,
            init_state,
            init_save_hash,
        };
        let listener = crate::dom::games::render_player(container, peers, ctx, &cfg);
        *self.listener.borrow_mut() = listener;
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
    fn games_and_apps_window_types_are_peer_scoped() {
        let g = AppWindow::games_window_type();
        assert_eq!(g.name, "Games");
        assert!(matches!(g.scope, crate::window::WindowScope::Peer));
        let a = AppWindow::apps_window_type();
        assert_eq!(a.name, "Apps");
        assert!(matches!(a.scope, crate::window::WindowScope::Peer));
    }

    #[test]
    fn view_state_round_trips() {
        let s = AppViewState {
            selected: "chess".into(),
        };
        assert_eq!(AppViewState::from_entity(&s.to_entity()), s);
        assert_eq!(s.to_entity().entity_type, APP_VIEW_TYPE);
    }

    /// Default (no `demo-apps`): opening a launcher window must NOT seed fake
    /// apps — with no origin/ingest the tree stays empty so the UI shows the
    /// honest empty state.
    #[cfg(not(feature = "demo-apps"))]
    #[tokio::test]
    async fn factory_does_not_seed_fake_apps() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let g = AppWindow::games_window_type();
        let _ = (g.create)(1, &pid, &peers);
        let a = AppWindow::apps_window_type();
        let _ = (a.create)(2, &pid, &peers);
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
    async fn games_factory_seeds_token_and_apps_factory_seeds_its_own() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();

        let g = AppWindow::games_window_type();
        let _ = (g.create)(1, &pid, &peers);
        let gcat = peers
            .get_entity(&pid, &paths::catalog_path(&pid, paths::GAMES_SET))
            .map(|e| AppCatalog::from_entity(&e))
            .expect("games catalog seeded");
        assert!(gcat.entries.iter().any(|e| e.id == "war"));
        assert!(peers
            .get_entity(&pid, &paths::bundle_path(&pid, paths::GAMES_SET, "war"))
            .is_some());

        let a = AppWindow::apps_window_type();
        let _ = (a.create)(2, &pid, &peers);
        let acat = peers
            .get_entity(&pid, &paths::catalog_path(&pid, paths::APPS_SET))
            .map(|e| AppCatalog::from_entity(&e))
            .expect("apps catalog seeded");
        assert!(acat.entries.iter().any(|e| e.id == "calculator"));
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
