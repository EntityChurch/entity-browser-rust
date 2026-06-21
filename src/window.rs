//! Window system — multi-instance window manager.
//!
//! Windows are independent instances, each with their own state.
//! The command palette spawns new instances; closing removes them.
//! Multiple windows of the same type can coexist.

use crate::action::Action;
use crate::peers::Peers;

/// Unique identifier for a window instance.
pub type WindowId = u64;

/// Repaint callback — call this to request a frame redraw. Produced only
/// on wasm (the render loop), but the type alias is defined for all
/// targets so app-tier code (e.g. the content-site resolver's late-bound
/// repaint cell) compiles + unit-tests natively.
pub type RepaintFn = std::rc::Rc<dyn Fn()>;

/// Storage for closures that must be kept alive between DOM rebuilds.
/// Cleared at the start of each render cycle, which drops old closures
/// after their DOM elements have been removed.
#[cfg(target_arch = "wasm32")]
pub type ClosureVec = std::rc::Rc<std::cell::RefCell<Vec<wasm_bindgen::JsValue>>>;

/// Create a new empty ClosureVec.
#[cfg(target_arch = "wasm32")]
pub fn new_closure_vec() -> ClosureVec {
    std::rc::Rc::new(std::cell::RefCell::new(Vec::new()))
}

/// A window view that renders into web-sys DOM.
#[allow(dead_code)]
pub trait WindowView {
    /// Display title (may include instance context, e.g., current path).
    fn title(&self) -> String;

    /// Type name for the command palette (e.g., "Entity Browser").
    fn type_name(&self) -> &'static str;

    /// The peer this window is bound to. Used for state cleanup on close.
    fn peer_id(&self) -> &str { "" }

    /// Subscription-driven dirty flag for this window. The DOM renderer
    /// uses [`WindowWatch::take_dirty`] to decide whether to rebuild
    /// the section: dirty → rebuild and clear; clean → skip entirely
    /// (preserves DOM-side state like input contents and scroll
    /// position).
    ///
    /// Implementations subscribe their watch to the tree paths their
    /// render reads — see `views/*/mod.rs` for examples.
    fn watch(&self) -> &crate::window_watch::WindowWatch;

    /// Handle an action targeted at this window.
    /// Peers provides tree access for entity-backed state.
    fn handle_action(&mut self, action: &Action, peers: &Peers);

    /// Render into a DOM container. This is THE rendering method.
    /// Every window implements this — build DOM elements, attach event
    /// handlers, set innerHTML, whatever the window needs.
    ///
    /// Use `ctx` helpers for event handlers:
    /// - `ctx.on_window_event(el, "click", "event_name", "value")` — static WindowEvent
    /// - `ctx.on_select_change(el, "event_name")` — <select> change → WindowEvent
    /// - `ctx.on_action(el, "click", Action::Foo)` — push any Action
    /// - `ctx.listen(el, "click", |e| { ... })` — custom handler
    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        state: &Peers,
        ctx: &crate::dom::util::DomCtx,
    );
}

/// User-facing menu grouping for a window type. **Orthogonal to
/// [`WindowScope`]**: scope decides which *peer* a window binds to (an internal
/// concern), category decides which *menu group* it appears under (what the user
/// sees). A category can mix scopes — e.g. `System` holds both the system-scoped
/// Settings and the peer-scoped Peer Connections.
///
/// The roster→category mapping lives in one place ([`crate::window_registry`]),
/// so re-sorting groups is a single-file edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowCategory {
    /// Everyday surfaces — what people actually open the app for. Expanded on
    /// first paint. Games, Apps, Content Site, Site Editor, Knowledge Base.
    AppsContent,
    /// The "control panel" — manage your peers, identity, storage, settings.
    /// User-facing but occasional; collapsed by default.
    System,
    /// Power/inspection tools (consoles, shell, taps, raw tree). Collapsed by
    /// default; not hidden — developers can find them.
    Developer,
}

impl WindowCategory {
    /// Section heading shown in the command palette.
    pub fn label(self) -> &'static str {
        match self {
            WindowCategory::AppsContent => "Apps & Content",
            WindowCategory::System => "System",
            WindowCategory::Developer => "Developer",
        }
    }

    /// One-line "what's in here" blurb — shown in the first-run empty-state so
    /// a new user knows what each menu section offers before opening anything.
    pub fn description(self) -> &'static str {
        match self {
            WindowCategory::AppsContent => "Browse sites, play games, run apps",
            WindowCategory::System => "Peers, keys, connections, settings, storage",
            WindowCategory::Developer => "Entity tree, query & execute consoles, shell, logs",
        }
    }

    /// Whether this group's disclosure starts open. Only the everyday group is
    /// expanded on first paint, so a fresh user lands on Games/Apps/Sites.
    pub fn open_by_default(self) -> bool {
        matches!(self, WindowCategory::AppsContent)
    }

    /// Stable key used to persist this group's open/closed toggle across rebuilds.
    pub fn key(self) -> &'static str {
        match self {
            WindowCategory::AppsContent => "apps",
            WindowCategory::System => "system",
            WindowCategory::Developer => "developer",
        }
    }

    /// All categories, in display order.
    pub fn all() -> [WindowCategory; 3] {
        [
            WindowCategory::AppsContent,
            WindowCategory::System,
            WindowCategory::Developer,
        ]
    }
}

/// Whether a window type is infrastructure or peer-scoped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowScope {
    /// Infrastructure window — always binds to the system peer.
    /// Event Log, Peers, Peer Connections, Key Manager, Settings.
    System,
    /// Peer-scoped window — binds to the user-selected peer.
    /// Entity Tree, Execute Console, Query Console.
    Peer,
}

/// Factory info for spawning windows from the command palette.
///
/// `Clone` is cheap — every field is `Copy` (`&'static str`, an enum, a fn
/// pointer) — which lets a single source ([`crate::window_registry`]) build the
/// list once and hand clones to both the registrar and the settings UI.
#[derive(Clone)]
pub struct WindowType {
    pub name: &'static str,
    #[allow(dead_code)]
    pub description: &'static str,
    /// System vs Peer scope. Read by the command palette (System-scoped spawn
    /// buttons bind the system peer; Peer-scoped bind the selected peer) and by
    /// the startup-surface settings control, which filters the window-type
    /// dropdown by scope against the chosen peer (non-system peers see only
    /// Peer-scoped types — handoff §4.4).
    pub scope: WindowScope,
    /// Factory: receives (window_id, peer_id, peer_manager).
    pub create: fn(WindowId, &str, &Peers) -> Box<dyn WindowView>,
}

/// A living window instance.
pub struct WindowInstance {
    pub id: WindowId,
    pub open: bool,
    pub view: Box<dyn WindowView>,
}

/// Manages all window instances and available types.
pub struct WindowManager {
    pub windows: Vec<WindowInstance>,
    next_id: WindowId,
    pub types: Vec<WindowType>,
}

impl WindowManager {
    pub fn new() -> Self {
        Self {
            windows: Vec::new(),
            next_id: 1,
            types: Vec::new(),
        }
    }

    /// Register a window type that can be spawned from the command palette.
    pub fn register_type(&mut self, window_type: WindowType) {
        self.types.push(window_type);
    }

    /// Spawn a new window instance of the given type bound to a peer. Returns its ID.
    pub fn spawn(&mut self, type_name: &str, peer_id: &str, peers: &Peers) -> Option<WindowId> {
        let factory = self.types.iter().find(|t| t.name == type_name)?;
        let id = self.next_id;
        self.next_id += 1;
        let view = (factory.create)(id, peer_id, peers);
        self.windows.push(WindowInstance {
            id,
            open: true,
            view,
        });
        Some(id)
    }

    /// Find an open window of the given type bound to the given peer.
    /// Used by single-instance ("singleton") window mode to focus an
    /// existing window instead of spawning a duplicate. Identity is the
    /// `(type_name, peer_id)` pair — a window type opened for two
    /// different peers is two distinct windows.
    pub fn find_open(&self, type_name: &str, peer_id: &str) -> Option<WindowId> {
        self.windows
            .iter()
            .find(|w| w.open && w.view.type_name() == type_name && w.view.peer_id() == peer_id)
            .map(|w| w.id)
    }

    /// Close a specific window instance.
    pub fn close(&mut self, id: WindowId) {
        if let Some(win) = self.windows.iter_mut().find(|w| w.id == id) {
            win.open = false;
        }
    }

    /// Remove all closed windows.
    pub fn gc_closed(&mut self) {
        self.windows.retain(|w| w.open);
    }

    /// Get a window by ID.
    #[allow(dead_code)]
    pub fn get(&self, id: WindowId) -> Option<&WindowInstance> {
        self.windows.iter().find(|w| w.id == id)
    }

    /// Get a mutable window by ID.
    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut WindowInstance> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    /// Number of open windows.
    pub fn open_count(&self) -> usize {
        self.windows.iter().filter(|w| w.open).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyView {
        name: String,
        watch: crate::window_watch::WindowWatch,
    }

    impl WindowView for DummyView {
        fn title(&self) -> String {
            self.name.clone()
        }
        fn type_name(&self) -> &'static str {
            "Dummy"
        }
        fn watch(&self) -> &crate::window_watch::WindowWatch {
            &self.watch
        }
        fn handle_action(&mut self, _action: &Action, _peers: &Peers) {}
    }

    fn dummy_type() -> WindowType {
        WindowType {
            name: "Dummy",
            description: "Test window",
            scope: WindowScope::System,
            create: |_id, _peer_id, _pm| Box::new(DummyView {
                name: "Dummy".into(),
                watch: crate::window_watch::WindowWatch::new(),
            }),
        }
    }

    fn test_peers() -> Peers {
        Peers::new_direct()
    }

    #[test]
    fn spawn_creates_instance() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(dummy_type());
        let id = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        assert_eq!(mgr.open_count(), 1);
        assert!(mgr.get(id).is_some());
    }

    #[test]
    fn spawn_unknown_type_returns_none() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        assert!(mgr.spawn("NonExistent", peers.primary_peer_id(), &peers).is_none());
    }

    #[test]
    fn spawn_multiple_instances() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(dummy_type());
        let id1 = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        let id2 = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        let id3 = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_eq!(mgr.open_count(), 3);
    }

    #[test]
    fn close_marks_not_open() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(dummy_type());
        let id = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        mgr.close(id);
        assert!(!mgr.get(id).unwrap().open);
        assert_eq!(mgr.open_count(), 0);
    }

    #[test]
    fn gc_removes_closed() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(dummy_type());
        let id1 = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        let _id2 = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        mgr.close(id1);
        mgr.gc_closed();
        assert_eq!(mgr.windows.len(), 1);
        assert!(mgr.get(id1).is_none());
    }

    #[test]
    fn find_open_matches_type_and_peer() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(dummy_type());
        let pid = peers.primary_peer_id();
        let id = mgr.spawn("Dummy", pid, &peers).unwrap();

        // DummyView::type_name() == "Dummy", peer_id() == "" (default).
        assert_eq!(mgr.find_open("Dummy", ""), Some(id));
        assert_eq!(mgr.find_open("Other", ""), None);
        assert_eq!(mgr.find_open("Dummy", "some-other-peer"), None);

        // A closed window is not "open" for focus purposes.
        mgr.close(id);
        assert_eq!(mgr.find_open("Dummy", ""), None);
    }

    #[test]
    fn ids_are_unique_and_increasing() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(dummy_type());
        let id1 = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        let id2 = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        assert!(id2 > id1);
    }

    #[test]
    fn close_and_respawn_gets_new_id() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(dummy_type());
        let id1 = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        mgr.close(id1);
        mgr.gc_closed();
        let id2 = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        assert_ne!(id1, id2);
        assert!(id2 > id1);
    }

    #[test]
    fn multiple_types_registered() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(WindowType {
            name: "TypeA",
            description: "",
            scope: WindowScope::System,
            create: |_, _, _| Box::new(DummyView { name: "A".into(), watch: crate::window_watch::WindowWatch::new() }),
        });
        mgr.register_type(WindowType {
            name: "TypeB",
            description: "",
            scope: WindowScope::Peer,
            create: |_, _, _| Box::new(DummyView { name: "B".into(), watch: crate::window_watch::WindowWatch::new() }),
        });
        let a = mgr.spawn("TypeA", peers.primary_peer_id(), &peers).unwrap();
        let b = mgr.spawn("TypeB", peers.primary_peer_id(), &peers).unwrap();
        assert_eq!(mgr.get(a).unwrap().view.title(), "A");
        assert_eq!(mgr.get(b).unwrap().view.title(), "B");
        assert_eq!(mgr.open_count(), 2);
    }

    #[test]
    fn get_mut_allows_modification() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(dummy_type());
        let id = mgr.spawn("Dummy", peers.primary_peer_id(), &peers).unwrap();
        mgr.get_mut(id).unwrap().open = false;
        assert_eq!(mgr.open_count(), 0);
    }

    #[tokio::test]
    async fn handle_action_dispatches_to_correct_window() {
        let peers = test_peers();
        let mut mgr = WindowManager::new();
        mgr.register_type(
            crate::views::entity_tree::EntityTreeWindow::window_type(),
        );
        let id1 = mgr.spawn("Entity Tree", peers.primary_peer_id(), &peers).unwrap();
        let id2 = mgr.spawn("Entity Tree", peers.primary_peer_id(), &peers).unwrap();

        // Navigate window 1 only.
        let action = Action::Navigate(id1, "docs/test".into());
        if let Some(win) = mgr.get_mut(id1) {
            win.view.handle_action(&action, &peers);
        }
        // Window state writes go through L1 dispatch; let the spawned
        // put task complete before reading the tree.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // Window 1 navigated (state in tree), window 2 unchanged.
        let pid = peers.primary_peer_id();
        let e1 = peers.get_entity(pid, &crate::app_paths::window_state_path(crate::app_paths::APP_ID, pid, id1));
        let e2 = peers.get_entity(pid, &crate::app_paths::window_state_path(crate::app_paths::APP_ID, pid, id2));
        assert!(e1.is_some());
        assert!(e2.is_some());
        // Window 1 state should contain the navigated path.
        let data1 = crate::format::format_entity_data(&e1.unwrap().data);
        assert!(data1.contains("docs/test"), "window 1 should have navigated path");
        // Window 2 state should not contain a current_path.
        let data2 = crate::format::format_entity_data(&e2.unwrap().data);
        assert!(!data2.contains("current_path"), "window 2 should have no current_path");
    }
}
