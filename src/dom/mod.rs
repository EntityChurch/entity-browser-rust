//! DOM renderer — web-sys HTML renderer with Shadow DOM encapsulation.
//!
//! Renders a command palette + dynamic window sections matching the
//! WindowManager's active instances.

pub mod chain_trace;
pub mod content_site;
pub mod content_stream;
pub mod entity_tree;
pub mod path_tap;
pub mod event_log;
#[cfg(target_arch = "wasm32")]
pub mod games;
pub mod execute_console;
pub mod key_manager;
pub mod knowledge_base;
pub mod peer_connections;
pub mod peer_management;
pub mod query_console;
pub mod scanner;
pub mod settings;
pub mod site_directory;
pub mod site_editor;
pub mod site_overlay;
pub mod shell;
pub mod storage;
pub mod style;
pub mod theme;
pub mod util;
pub mod wire_recorder;

pub use util::DomCtx;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use wasm_bindgen::JsCast;

use crate::action::Action;
use crate::peers::Peers;
use crate::window::{
    ClosureVec, RepaintFn, WindowId, WindowManager, WindowScope, WindowType,
};

use web_sys::{Element, ShadowRoot, ShadowRootInit, ShadowRootMode};

/// Per-window section state — tracked across renders so each window's
/// section can be updated independently.
///
/// The section element is **stable** across renders. When a window's
/// content_hash changes, only its section's contents are rebuilt
/// (header + body cleared and re-rendered into the same section
/// element). When the hash is unchanged, the section is left
/// completely alone — its DOM, event listeners, and any DOM-side
/// state (input/textarea contents, scroll position, focus) are
/// preserved.
///
/// Each section has its own `closures` vec so the closures for a
/// rebuilt section can be dropped without affecting other sections'
/// listeners.
struct WindowSectionState {
    section_el: Element,
    /// The header's maximize/restore button. Held so its glyph + title
    /// can be reconciled **per-frame** alongside the `.maximized` class
    /// (see [`DomRenderer::render`]) — a maximize toggle must NOT mark the
    /// window dirty, because rebuilding the section tears down and recreates
    /// live DOM-side state the tree can't reconstruct (most importantly the
    /// Games/Apps sandboxed `<iframe>`, whose internal app state would reset).
    max_btn: Element,
    /// Closures kept alive for this section's listeners. Cleared
    /// when the section is rebuilt. Dropping the section state
    /// drops these (which dereferences the JS-side closures).
    closures: ClosureVec,
    /// In-flight input drafts (keyed by field_id) for `tracked_input`/
    /// `tracked_textarea` consumers. Lifetime tied to the section
    /// state — closing the window evaporates the typing buffer.
    /// Persists across section *rebuilds* (the whole point) so
    /// async-completion rebuilds don't clobber typing.
    drafts: util::DraftsMap,
}

/// DOM renderer with Shadow DOM encapsulation.
pub struct DomRenderer {
    #[allow(dead_code)]
    shadow_root: ShadowRoot,
    palette: Element,
    window_area: Element,
    pending_actions: Rc<RefCell<Vec<Action>>>,
    /// Per-group disclosure open/closed state, keyed by
    /// [`WindowCategory::key`]. Seeded from `open_by_default()` and updated by
    /// each group's `<details>` toggle so rebuilds preserve what the user
    /// expanded/collapsed.
    group_open: Rc<RefCell<HashMap<&'static str, bool>>>,
    /// Mobile only: whether the whole palette is expanded. On small screens the
    /// palette collapses behind a single `☰ Menu` toggle so it doesn't hog the
    /// viewport; default closed. Desktop ignores this (CSS keeps the body shown).
    /// Preserved across rebuilds so an unrelated palette rebuild can't snap it
    /// shut mid-use; spawn buttons reset it so the menu auto-closes after a pick.
    shell_open: Rc<RefCell<bool>>,
    /// Mobile only: whether the "Open Windows" panel is expanded. Independent of
    /// `shell_open` so the active-windows list has its own one-tap toggle in the
    /// split menu bar (☰ Menu | Open Windows) — you can pop your windows without
    /// digging through the spawn menu. Desktop ignores it (CSS always shows it).
    windows_open: Rc<RefCell<bool>>,
    /// Selected peer for spawning peer-scoped windows.
    /// Defaults to system peer. Updated by the peer selector dropdown.
    selected_peer: Rc<RefCell<String>>,
    repaint: RepaintFn,
    /// Last palette signature — when this changes, the palette is
    /// rebuilt. Includes things that affect palette rendering (window
    /// list, peer list, log/connection counts) but NOT individual
    /// window content.
    last_palette_signature: String,
    /// Closures owned by the command palette. Cleared and refilled
    /// when the palette is rebuilt; otherwise stable across frames.
    palette_closures: ClosureVec,
    /// Per-window section state. Each entry has its own section
    /// element + closures + last content hash. Sections are added
    /// when a window opens, removed when it closes, and updated
    /// in-place when their content hash changes.
    window_sections: HashMap<WindowId, WindowSectionState>,
    /// First-run / all-windows-closed hint shown in the otherwise-blank window
    /// area. Present only while no window is open; removed when one opens and
    /// re-added when the last one closes. `None` when not currently shown.
    empty_state: Option<Element>,
    /// Diagnostics: track rebuild frequency.
    rebuild_count: u64,
    last_rebuild_log: f64,
    rebuilds_since_log: u64,
}

impl DomRenderer {
    /// Initialize by attaching Shadow DOM to #dom-layer.
    pub fn new(repaint: RepaintFn) -> Option<Self> {
        match Self::try_new(repaint) {
            Ok(renderer) => Some(renderer),
            Err(e) => {
                tracing::error!("DomRenderer initialization failed: {}", e);
                web_sys::console::error_1(
                    &format!("[Entity Browser] DomRenderer init failed: {}", e).into(),
                );
                None
            }
        }
    }

    fn try_new(repaint: RepaintFn) -> Result<Self, String> {
        let dom_layer = util::get_element_by_id("dom-layer")
            .ok_or("could not find #dom-layer element")?;

        let shadow_init = ShadowRootInit::new(ShadowRootMode::Open);
        let shadow_root = dom_layer
            .attach_shadow(&shadow_init)
            .map_err(|e| format!("attach_shadow on #dom-layer failed: {:?}", e))?;

        // Inject styles.
        let style_el = util::document()
            .create_element("style")
            .map_err(|e| format!("create <style> element failed: {:?}", e))?;
        style_el.set_text_content(Some(style::DOM_STYLES));
        shadow_root
            .append_child(&style_el)
            .map_err(|e| format!("append <style> to shadow root failed: {:?}", e))?;

        // Root container.
        let root = util::document()
            .create_element("div")
            .map_err(|e| format!("create root <div> failed: {:?}", e))?;
        root.set_class_name("window-manager");
        shadow_root
            .append_child(&root)
            .map_err(|e| format!("append root to shadow root failed: {:?}", e))?;

        // Command palette.
        let palette = util::document()
            .create_element("nav")
            .map_err(|e| format!("create <nav> palette failed: {:?}", e))?;
        palette.set_class_name("command-palette");
        root.append_child(&palette)
            .map_err(|e| format!("append palette to root failed: {:?}", e))?;

        // Window area for dynamic window sections.
        let window_area = util::document()
            .create_element("div")
            .map_err(|e| format!("create window-area <div> failed: {:?}", e))?;
        window_area.set_class_name("window-area");
        root.append_child(&window_area)
            .map_err(|e| format!("append window-area to root failed: {:?}", e))?;

        let pending_actions = Rc::new(RefCell::new(Vec::new()));
        let group_open: HashMap<&'static str, bool> = crate::window::WindowCategory::all()
            .iter()
            .map(|c| (c.key(), c.open_by_default()))
            .collect();
        let group_open = Rc::new(RefCell::new(group_open));
        let shell_open = Rc::new(RefCell::new(false));
        let windows_open = Rc::new(RefCell::new(false));
        let selected_peer = Rc::new(RefCell::new(String::new()));

        Ok(Self {
            shadow_root,
            palette,
            window_area,
            pending_actions,
            group_open,
            shell_open,
            windows_open,
            selected_peer,
            repaint,
            last_palette_signature: String::new(),
            palette_closures: crate::window::new_closure_vec(),
            window_sections: HashMap::new(),
            empty_state: None,
            rebuild_count: 0,
            last_rebuild_log: 0.0,
            rebuilds_since_log: 0,
        })
    }

    /// The shared pending-action sink, drained at the top of each
    /// [`render`](Self::render). Light-DOM controls outside the shadow
    /// root (the status-bar Site Mode toggle) push here so their actions
    /// flow through the normal frame-loop dispatch.
    pub fn action_sink(&self) -> Rc<RefCell<Vec<Action>>> {
        self.pending_actions.clone()
    }

    /// A clone of the repaint signal, for the same light-DOM controls.
    pub fn repaint_handle(&self) -> RepaintFn {
        self.repaint.clone()
    }

    /// Scroll an existing window's section into view. Used by
    /// single-instance ("singleton") window mode to surface the already-open
    /// window instead of spawning a duplicate (mirrors the command palette's
    /// "Active" list click behavior).
    pub fn focus_window(&self, id: WindowId) {
        if let Ok(Some(el)) = self
            .shadow_root
            .query_selector(&format!("[data-instance='{}']", id))
        {
            el.scroll_into_view();
        }
    }

    /// Render DOM for all windows. Always drains pending actions.
    ///
    /// Two-tier rendering:
    /// 1. The command **palette** is rebuilt only when its signature
    ///    changes (window list, peer list, etc.). Palette closures
    ///    live in their own vec.
    /// 2. Each window's **section** is rebuilt only when its
    ///    `content_hash` changes. Each section has its own closures
    ///    vec, so rebuilding one section doesn't affect others.
    ///
    /// This is what preserves DOM-side state (input/textarea contents,
    /// scroll position, focus) across rebuilds caused by unrelated
    /// activity.
    pub fn render(
        &mut self,
        peers: &Peers,
        window_manager: &WindowManager,
        actions: &mut Vec<Action>,
        maximized: Option<WindowId>,
    ) {
        // Always drain pending actions.
        let mut pending = self.pending_actions.borrow_mut();
        actions.append(&mut *pending);
        drop(pending);

        let frame_start = js_sys::Date::now();

        // Update the palette if its signature changed.
        let palette_signature = Self::compute_palette_signature(peers, window_manager);
        let palette_changed = palette_signature != self.last_palette_signature;
        if palette_changed {
            self.last_palette_signature = palette_signature;
            self.rebuild_palette(peers, window_manager);
        }

        // Update window sections (per-section, only changed ones —
        // each window's `WindowWatch` decides whether to rebuild).
        // `section_timings` is populated regardless; the cost is one
        // js_sys::Date::now() per rebuilt window — negligible next to
        // the DOM ops themselves — and the breakdown is only emitted
        // when the slow-rebuild warning fires.
        let mut section_timings: Vec<(String, WindowId, f64)> = Vec::new();
        let any_section_changed =
            self.update_window_sections(peers, window_manager, &mut section_timings, maximized);

        // Show the first-run hint whenever the window area is empty (boot with
        // nothing open, or the user closed every window); remove it as soon as
        // a window opens. Cheap: a bool check each frame, a DOM op only on the
        // 0↔non-0 transition.
        self.sync_empty_state(window_manager);

        // Reconcile the maximized surface every frame, independent of the
        // dirty-gated section rebuild: a maximize/restore toggle changes no
        // tree state, so the owning window's watch may be clean and its
        // section won't rebuild — but the `.maximized` class still must track
        // `maximized`. Cheap idempotent classList writes. (The app also marks
        // the affected windows dirty so the button label rebuilds; this keeps
        // the full-screen promotion correct regardless.)
        for (id, state) in self.window_sections.iter() {
            let is_max = maximized == Some(*id);
            let want = if is_max { "window maximized" } else { "window" };
            if state.section_el.class_name() != want {
                state.section_el.set_class_name(want);
            }
            // The header glyph tracks maximize state the same way — reconciled
            // here, NOT via a dirty-mark + section rebuild, so toggling maximize
            // never tears down live DOM-side state (the Games/Apps iframe).
            let glyph = if is_max { "❐" } else { "▢" };
            if state.max_btn.text_content().as_deref() != Some(glyph) {
                util::set_text(&state.max_btn, glyph);
                util::set_attr(
                    &state.max_btn,
                    "title",
                    if is_max { "Restore window" } else { "Maximize window" },
                );
            }
        }

        // Diagnostics: only count this as a rebuild if we actually
        // touched the DOM. Pure no-op frames (every window's hash
        // unchanged AND palette unchanged) are silently skipped.
        if !palette_changed && !any_section_changed {
            return;
        }

        self.rebuild_count += 1;
        self.rebuilds_since_log += 1;

        if frame_start - self.last_rebuild_log > 1000.0 {
            if self.rebuilds_since_log > 10 {
                tracing::warn!(
                    rebuilds_per_sec = self.rebuilds_since_log,
                    total = self.rebuild_count,
                    "DOM: HIGH REBUILD RATE"
                );
            }
            self.rebuilds_since_log = 0;
            self.last_rebuild_log = frame_start;
        }

        let elapsed = js_sys::Date::now() - frame_start;
        // Threshold raised from 16ms → 33ms (60fps → 30fps). Bulk
        // operations like peer-deletion fire many subscriptions and
        // routinely land in the 17–24ms range — not user-visible
        // but enough to spam the console. Sustained sub-30fps IS
        // visible, so 33ms keeps the alarm useful.
        if elapsed > 33.0 {
            // Per-window breakdown of which sections rebuilt + their
            // wall times. Peer-delete is the canonical case that fires
            // 3+ subscribed windows in one frame (Peer Mgmt, Peer
            // Connections, Key Manager) — the breakdown identifies the
            // expensive ones.
            let breakdown: String = section_timings
                .iter()
                .map(|(name, id, ms)| format!("{}#{}={:.1}ms", name, id, ms))
                .collect::<Vec<_>>()
                .join(", ");
            tracing::warn!(
                elapsed_ms = elapsed,
                palette_rebuilt = palette_changed,
                sections = breakdown,
                "DOM: SLOW REBUILD — exceeded 30fps budget"
            );
        }
    }

    /// Add or remove the empty-window-area hint to match whether any window is
    /// open. Idempotent: only touches the DOM on the 0↔non-0 transition.
    fn sync_empty_state(&mut self, window_manager: &WindowManager) {
        let any_open = window_manager.windows.iter().any(|w| w.open);
        if any_open {
            if let Some(el) = self.empty_state.take() {
                el.remove();
            }
        } else if self.empty_state.is_none() {
            let el = build_empty_state();
            util::append(&self.window_area, &el);
            self.empty_state = Some(el);
        }
    }

    /// Compute a signature string that captures everything affecting
    /// the **command palette** (NOT individual window content). When
    /// this changes the palette is rebuilt; per-window sections are
    /// rebuilt independently based on their own content hash.
    fn compute_palette_signature(
        peers: &Peers,
        window_manager: &WindowManager,
    ) -> String {
        let mut s = String::new();
        for win in &window_manager.windows {
            if win.open {
                s.push_str(&format!(
                    "{}:{}:{};",
                    win.id,
                    win.view.type_name(),
                    win.view.title()
                ));
            }
        }
        // Peer list affects the peer selector dropdown.
        for pid in peers.peer_ids() {
            let display = crate::peer_display::PeerDisplay::classify(peers, &pid);
            let label = peers
                .peer_metadata(&pid)
                .and_then(|m| m.label)
                .unwrap_or_default();
            s.push_str(&format!("{}:{}:{};", pid, display, label));
        }
        s
    }

    /// Tear down and rebuild the command palette. Drops all palette
    /// closures (their DOM elements are about to be removed). Builds
    /// a fresh palette and stores its closures in `palette_closures`.
    fn rebuild_palette(&mut self, peers: &Peers, window_manager: &WindowManager) {
        util::clear_children(&self.palette);
        self.palette_closures.borrow_mut().clear();

        let system_pid = peers.system_peer_id().to_string();

        // Mobile: the palette collapses behind a split toggle bar so it doesn't
        // dominate the screen — `☰ Menu` (≈75%) reveals the spawn menu (`body`),
        // `Open Windows` (≈25%) reveals just the active-windows panel, each
        // independently. This lets a phone user pop their open windows in one tap
        // instead of opening the menu, expanding a group, then closing it again.
        // Desktop CSS hides the bar and always shows both (the sidebar is
        // unchanged). The shell's classes (`menu-open` / `windows-open`) gate the
        // two panels; toggle clicks recompute the class via `shell_class`.
        let open_count = window_manager.windows.iter().filter(|w| w.open).count();
        // No windows → nothing to show in the Open Windows panel. Keep it closed
        // so its toggle can't flash an empty panel (the 0-count jitter); the
        // toggle is also disabled below.
        if open_count == 0 {
            *self.windows_open.borrow_mut() = false;
        }
        let shell = util::create_element_with_class("div", "palette-shell");
        shell.set_class_name(&shell_class(
            *self.shell_open.borrow(),
            *self.windows_open.borrow(),
        ));

        let bar = util::create_element_with_class("div", "palette-bar");
        let menu_toggle = util::create_element_with_class("button", "palette-toggle");
        util::set_text(&menu_toggle, "☰ Menu");
        {
            let so = self.shell_open.clone();
            let wo = self.windows_open.clone();
            let shell_ref = shell.clone();
            let rp = self.repaint.clone();
            util::listen(
                &menu_toggle,
                "click",
                move |_| {
                    let now = !*so.borrow();
                    *so.borrow_mut() = now;
                    shell_ref.set_class_name(&shell_class(now, *wo.borrow()));
                    rp();
                },
                &self.palette_closures,
            );
        }
        util::append(&bar, &menu_toggle);

        let windows_toggle =
            util::create_element_with_class("button", "palette-windows-toggle");
        // Symbol + count instead of the word "Windows" — the word truncated to
        // "Window…" in the narrow (~25%) slot. ⧉ (two joined squares) reads as
        // "windows" without saying it; the count rides alongside.
        util::set_text(&windows_toggle, &format!("\u{29c9} {open_count}"));
        util::set_attr(&windows_toggle, "title", "Open windows");
        // Nothing to open at zero — disable so a tap is a no-op (no empty-panel
        // jitter). Re-enabled on the next rebuild once a window exists.
        if open_count == 0 {
            util::set_attr(&windows_toggle, "disabled", "");
        }
        {
            let so = self.shell_open.clone();
            let wo = self.windows_open.clone();
            let shell_ref = shell.clone();
            let rp = self.repaint.clone();
            util::listen(
                &windows_toggle,
                "click",
                move |_| {
                    let now = !*wo.borrow();
                    *wo.borrow_mut() = now;
                    shell_ref.set_class_name(&shell_class(*so.borrow(), now));
                    rp();
                },
                &self.palette_closures,
            );
        }
        util::append(&bar, &windows_toggle);
        util::append(&shell, &bar);

        let body = util::create_element_with_class("div", "palette-body");
        // A little header bar (mobile-only via CSS) so when both panels are open
        // it's obvious where the menu ends and the windows list begins.
        util::append(&body, &panel_head("Menu"));
        util::append(&shell, &body);
        util::append(&self.palette, &shell);

        // Ensure the selected peer is still valid; default to the system peer.
        {
            let current = self.selected_peer.borrow().clone();
            let all_ids = peers.peer_ids();
            if current.is_empty() || !all_ids.contains(&current) {
                *self.selected_peer.borrow_mut() = system_pid.clone();
            }
        }

        // Peers we can actually bind a window to. The selector only earns its
        // space when there's a real choice — a single-peer user never sees it,
        // and their peer-scoped windows bind the system peer silently.
        let selectable: Vec<String> = peers
            .peer_ids()
            .into_iter()
            .filter(|pid| peers.has_peer_context(pid))
            .collect();
        if selectable.len() > 1 {
            self.append_peer_selector(&body, peers, &selectable);
        }

        // Grouped, collapsible spawn sections. The roster→group mapping lives in
        // `window_registry::window_groups`; only the everyday group starts open.
        for (cat, names) in crate::window_registry::window_groups() {
            let open = self
                .group_open
                .borrow()
                .get(cat.key())
                .copied()
                .unwrap_or_else(|| cat.open_by_default());

            let details = util::create_element_with_class("details", "palette-group");
            if open {
                util::set_attr(&details, "open", "");
            }
            let summary = util::create_element("summary");
            util::set_text(&summary, cat.label());
            util::append(&details, &summary);

            // Persist this group's open/closed state across rebuilds.
            {
                let go = self.group_open.clone();
                let key = cat.key();
                util::listen(
                    &details,
                    "toggle",
                    move |event: web_sys::Event| {
                        if let Some(el) = event.target() {
                            if let Some(d) = el.dyn_ref::<web_sys::HtmlDetailsElement>() {
                                go.borrow_mut().insert(key, d.open());
                            }
                        }
                    },
                    &self.palette_closures,
                );
            }

            for name in names {
                if let Some(wtype) = window_manager.types.iter().find(|t| t.name == name) {
                    self.append_spawn_button(&details, wtype, &system_pid);
                }
            }
            util::append(&body, &details);
        }

        // Active windows live in their own panel (own mobile toggle); on desktop
        // it sits below the spawn menu in the sidebar, as before.
        let windows = util::create_element_with_class("div", "palette-windows");
        util::append(&windows, &panel_head("Open Windows"));
        self.append_active_windows(&windows, window_manager);
        util::append(&shell, &windows);
    }

    /// Append a spawn button for `wtype`. System-scoped windows bind the system
    /// peer; peer-scoped windows bind whatever the selector last chose (the
    /// system peer when there's no selector).
    fn append_spawn_button(&self, parent: &Element, wtype: &WindowType, system_pid: &str) {
        let btn = util::create_element_with_class("button", "spawn-btn");
        util::set_text(&btn, &format!("+ {}", wtype.name));
        let actions_rc = self.pending_actions.clone();
        let rp = self.repaint.clone();
        let name: &'static str = wtype.name;
        match wtype.scope {
            WindowScope::System => {
                let pid = system_pid.to_string();
                let so = self.shell_open.clone();
                util::listen(
                    &btn,
                    "click",
                    move |_| {
                        actions_rc.borrow_mut().push(Action::SpawnWindow {
                            type_name: name,
                            peer_id: Some(pid.clone()),
                        });
                        *so.borrow_mut() = false; // auto-close the mobile menu
                        rp();
                    },
                    &self.palette_closures,
                );
            }
            WindowScope::Peer => {
                let sp = self.selected_peer.clone();
                let so = self.shell_open.clone();
                util::listen(
                    &btn,
                    "click",
                    move |_| {
                        let pid = sp.borrow().clone();
                        actions_rc.borrow_mut().push(Action::SpawnWindow {
                            type_name: name,
                            peer_id: if pid.is_empty() { None } else { Some(pid) },
                        });
                        *so.borrow_mut() = false; // auto-close the mobile menu
                        rp();
                    },
                    &self.palette_closures,
                );
            }
        }
        util::append(parent, &btn);
    }

    /// The peer selector that targets peer-scoped spawns. Only rendered when
    /// more than one peer is bindable (see caller).
    fn append_peer_selector(&self, parent: &Element, peers: &Peers, selectable: &[String]) {
        let label = util::create_element("h3");
        util::set_text(&label, "Peer");
        util::append(parent, &label);

        let select = util::create_element("select");
        util::set_attr(
            &select,
            "style",
            "width: 100%; margin-bottom: 8px; padding: 6px; background: var(--bg, #1a1a2e); \
             color: var(--text, #e0e0e0); border: 1px solid var(--border-strong, #444); border-radius: 3px; font-size: 0.85em;",
        );

        // Authoritative mode map, read once (not per peer).
        let modes = crate::persistence::peer_modes();
        for pid in selectable {
            let option = util::create_element("option");
            util::set_attr(&option, "value", pid);
            // Glyph + alias-or-pid + role, e.g. "◆⛁ notes-store (backend (opfs))".
            let (_, glyph, role) = crate::peer_display::resolve_role(peers, pid, &modes);
            let name = crate::views::display_name(peers, pid);
            util::set_text(&option, &format!("{} {} ({})", glyph, name, role));
            if *self.selected_peer.borrow() == *pid {
                util::set_attr(&option, "selected", "");
            }
            util::append(&select, &option);
        }

        {
            let sp = self.selected_peer.clone();
            let rp = self.repaint.clone();
            util::listen(
                &select,
                "change",
                move |event: web_sys::Event| {
                    if let Some(target) = event.target() {
                        if let Some(sel) = target.dyn_ref::<web_sys::HtmlSelectElement>() {
                            *sp.borrow_mut() = sel.value();
                            rp();
                        }
                    }
                },
                &self.palette_closures,
            );
        }
        util::append(parent, &select);
    }

    /// The "Open Windows" group — focus/close controls for live instances.
    fn append_active_windows(&self, parent: &Element, window_manager: &WindowManager) {
        let open_count = window_manager.windows.iter().filter(|w| w.open).count();
        let details = util::create_element_with_class("details", "palette-group");
        util::set_attr(&details, "open", "");
        let summary = util::create_element("summary");
        util::set_text(&summary, &format!("Open Windows ({})", open_count));
        util::append(&details, &summary);

        for win in &window_manager.windows {
            if !win.open {
                continue;
            }
            let entry = util::create_element_with_class("div", "active-entry");
            let title = win.view.title();
            // Char-safe truncation: titles are static ASCII today, but a future
            // dynamic title (site/page name, app label with emoji) could put a
            // multibyte boundary at byte 27 — byte-slicing there would panic and
            // a panic in the render loop kills the app (D13).
            let short = if title.chars().count() > 30 {
                let head: String = title.chars().take(27).collect();
                format!("{head}...")
            } else {
                title
            };

            let span = util::create_element_with_class("span", "active-title");
            util::set_text(&span, &short);
            {
                let wid = win.id;
                let shadow = self.shadow_root.clone();
                let wo = self.windows_open.clone();
                let so = self.shell_open.clone();
                util::listen(
                    &span,
                    "click",
                    move |_| {
                        if let Ok(Some(el)) =
                            shadow.query_selector(&format!("[data-instance='{}']", wid))
                        {
                            el.scroll_into_view();
                        }
                        // Mobile: collapse the Open Windows panel after focusing
                        // so the chosen window isn't hidden behind it. Update the
                        // shell class directly — focusing changes no palette
                        // signature, so there's no rebuild to apply it.
                        *wo.borrow_mut() = false;
                        if let Ok(Some(shell)) = shadow.query_selector(".palette-shell") {
                            shell.set_class_name(&shell_class(*so.borrow(), false));
                        }
                    },
                    &self.palette_closures,
                );
            }
            util::append(&entry, &span);

            let close_btn = util::create_element_with_class("button", "close-small");
            util::set_text(&close_btn, "\u{00d7}");
            util::set_attr(&close_btn, "title", "Close window");
            {
                let actions_rc = self.pending_actions.clone();
                let rp = self.repaint.clone();
                let wid = win.id;
                util::listen(
                    &close_btn,
                    "click",
                    move |_| {
                        actions_rc.borrow_mut().push(Action::CloseWindow(wid));
                        rp();
                    },
                    &self.palette_closures,
                );
            }
            util::append(&entry, &close_btn);
            util::append(&details, &entry);
        }

        util::append(parent, &details);
    }

    /// Update the per-window sections. For each open window, compares
    /// its current `content_hash` against the previous frame's hash;
    /// only rebuilds the section if the hash changed. Windows whose
    /// `content_hash` returns `None` (legacy windows) get the
    /// `legacy_hash` parameter as their hash, which is computed once
    /// per frame from the same signals the old snapshot string
    /// included. Returns `true` if any section was added, removed,
    /// or rebuilt.
    fn update_window_sections(
        &mut self,
        peers: &Peers,
        window_manager: &WindowManager,
        timings_out: &mut Vec<(String, WindowId, f64)>,
        maximized: Option<WindowId>,
    ) -> bool {
        use std::collections::HashSet;

        let mut any_changed = false;

        // Determine which windows are currently open.
        let open_ids: HashSet<WindowId> = window_manager
            .windows
            .iter()
            .filter(|w| w.open)
            .map(|w| w.id)
            .collect();

        // Remove sections for windows that are no longer open.
        // Dropping the WindowSectionState drops its closures, which
        // dereferences the JS-side functions.
        let to_remove: Vec<WindowId> = self
            .window_sections
            .keys()
            .filter(|id| !open_ids.contains(id))
            .copied()
            .collect();
        for id in to_remove {
            if let Some(state) = self.window_sections.remove(&id) {
                state.section_el.remove();
                any_changed = true;
            }
        }

        // For each open window, ensure its section exists and is fresh.
        // Each window owns a `WindowWatch` whose dirty flag gates
        // rebuilds. The flag is flipped by L0 subscriptions on the
        // tree paths the window's render reads.
        for win in &window_manager.windows {
            if !win.open {
                continue;
            }

            let watch = win.view.watch();
            // First-render seed: a brand-new section must build even if
            // the watch happened to be clean.
            let first = !self.window_sections.contains_key(&win.id);
            if !watch.take_dirty() && !first {
                continue;
            }
            any_changed = true;
            let section_start = js_sys::Date::now();

            // Get or create the section state. New sections get a
            // fresh section element appended to window_area.
            if first {
                let section_el =
                    util::create_element_with_class("section", "window");
                util::set_attr(&section_el, "data-instance", &win.id.to_string());
                // Bound peer-id — lets e2e helpers / DOM consumers
                // disambiguate multiple windows of the same type bound
                // to different peers (e.g., one Shell per backend
                // Worker in the cross-Worker e2e flow).
                let pid = win.view.peer_id();
                if !pid.is_empty() {
                    util::set_attr(&section_el, "data-peer-id", pid);
                }
                // Prepend (not append): newest window shows at the TOP of the
                // stack, so a fresh spawn is immediately visible instead of
                // pushed to the bottom of a scrolled window-area (and the
                // autofocus/scroll-into-view lands at the top, not the bottom).
                util::prepend(&self.window_area, &section_el);
                self.window_sections.insert(
                    win.id,
                    WindowSectionState {
                        section_el,
                        // Transient placeholder; the real maximize button is
                        // assigned during the header build below (which runs on
                        // every rebuild, first included) before any reconcile.
                        max_btn: util::create_element("button"),
                        closures: crate::window::new_closure_vec(),
                        drafts: std::rc::Rc::new(std::cell::RefCell::new(
                            std::collections::HashMap::new(),
                        )),
                    },
                );
            }

            // Borrow split: we need to access self.pending_actions and
            // self.repaint while also mutating the section state.
            // Pull what we need into locals first.
            let pending_actions = self.pending_actions.clone();
            let repaint = self.repaint.clone();

            let state = self.window_sections.get_mut(&win.id).unwrap();

            // Preserve scroll positions of any opted-in scroll containers
            // (`data-scroll-key`) across the teardown below, so e.g. expanding a
            // deep Entity Tree node doesn't snap the user back to the top.
            let saved_scroll = util::capture_scroll_positions(&state.section_el);

            // Clear contents and closures of the existing section.
            util::clear_children(&state.section_el);
            state.closures.borrow_mut().clear();

            // Build header (title + peer badge + close button).
            let header = util::create_element("header");
            let h3 = util::create_element("h3");
            util::set_text(&h3, &win.view.title());
            util::append(&header, &h3);

            let pid = win.view.peer_id();
            if !pid.is_empty() {
                let badge = util::create_element("span");
                let modes = crate::persistence::peer_modes();
                let (kind, glyph, role) =
                    crate::peer_display::resolve_role(peers, pid, &modes);
                let color = match kind {
                    crate::peer_display::PeerDisplay::Primary => "#6b8",
                    crate::peer_display::PeerDisplay::Local => "#8ab",
                    crate::peer_display::PeerDisplay::Remote => "#b8a",
                };
                // Glyph (role/type) + alias-or-pid, e.g. "◆⛁ notes-store".
                let name = crate::views::display_name(peers, pid);
                util::set_text(&badge, &format!("{} {}", glyph, name));
                util::set_attr(
                    &badge,
                    "title",
                    &format!(
                        "{} · {} · {}",
                        name,
                        role,
                        crate::views::short_pid(pid),
                    ),
                );
                util::set_attr(
                    &badge,
                    "style",
                    &format!(
                        "font-size: 0.65em; color: {}; border: 1px solid {}; \
                         border-radius: 3px; padding: 1px 5px; margin-left: 8px; \
                         font-family: monospace; vertical-align: middle;",
                        color, color,
                    ),
                );
                util::append(&header, &badge);
            }

            // Right-side control cluster: maximize/restore + close.
            let is_max = maximized == Some(win.id);
            let ctls = util::create_element_with_class("div", "winctls");

            // Maximize ↔ restore (the surface toggle, reframe §4-B).
            let max_btn = util::create_element_with_class("button", "winctl");
            util::set_text(&max_btn, if is_max { "❐" } else { "▢" });
            util::set_attr(
                &max_btn,
                "title",
                if is_max { "Restore window" } else { "Maximize window" },
            );
            {
                let actions_rc = pending_actions.clone();
                let rp = repaint.clone();
                let wid = win.id;
                util::listen(
                    &max_btn,
                    "click",
                    move |_| {
                        actions_rc.borrow_mut().push(Action::ToggleMaximizeWindow(wid));
                        rp();
                    },
                    &state.closures,
                );
            }
            util::append(&ctls, &max_btn);
            // Hold the button so the per-frame reconcile can flip its glyph
            // without rebuilding the section (the toggle no longer marks dirty).
            state.max_btn = max_btn;

            let close_btn = util::create_element_with_class("button", "close");
            util::set_text(&close_btn, "\u{00d7}");
            util::set_attr(&close_btn, "title", "Close window");
            {
                let actions_rc = pending_actions.clone();
                let rp = repaint.clone();
                let wid = win.id;
                util::listen(
                    &close_btn,
                    "click",
                    move |_| {
                        actions_rc.borrow_mut().push(Action::CloseWindow(wid));
                        rp();
                    },
                    &state.closures,
                );
            }
            util::append(&ctls, &close_btn);
            util::append(&header, &ctls);
            util::append(&state.section_el, &header);

            // Reflect maximized state on the section (the `.maximized` CSS
            // rule promotes it to the full-screen surface). Set during the
            // rebuild; also reconciled every frame below as a safety net.
            state
                .section_el
                .set_class_name(if is_max { "window maximized" } else { "window" });

            // Window content — call the view's render_dom with a
            // DomCtx that captures THIS section's closures.
            let content = util::create_element_with_class("div", "window-content");
            let ctx = util::DomCtx {
                window_id: win.id,
                actions: pending_actions,
                repaint,
                closures: state.closures.clone(),
                drafts: state.drafts.clone(),
            };

            #[cfg(feature = "measurement")]
            let render_start = js_sys::Date::now();
            #[cfg(feature = "measurement")]
            let counters_before = crate::frame_counters::snapshot();

            win.view.render_dom(&content, peers, &ctx);

            #[cfg(feature = "measurement")]
            {
                let render_elapsed = js_sys::Date::now() - render_start;
                let (d_get, d_list) =
                    crate::frame_counters::diff_since(counters_before);
                tracing::info!(
                    window = %win.view.title(),
                    window_id = win.id,
                    render_ms = format!("{:.2}", render_elapsed),
                    get_entity = d_get,
                    tree_listing = d_list,
                    "window render"
                );
            }

            util::append(&state.section_el, &content);

            // Re-anchor any preserved scrollers now that the rebuilt content is
            // attached (matched by `data-scroll-key`; clamps if content shrank).
            util::restore_scroll_positions(&state.section_el, &saved_scroll);

            timings_out.push((
                win.view.type_name().to_string(),
                win.id,
                js_sys::Date::now() - section_start,
            ));
        }

        any_changed
    }
}

/// The `.palette-shell` class string for the current mobile toggle state. Two
/// independent panels: `menu-open` gates the spawn menu (`.palette-body`),
/// `windows-open` gates the active-windows panel (`.palette-windows`). Desktop
/// CSS ignores both (always shows the sidebar).
fn shell_class(menu_open: bool, windows_open: bool) -> String {
    let mut c = String::from("palette-shell");
    if menu_open {
        c.push_str(" menu-open");
    }
    if windows_open {
        c.push_str(" windows-open");
    }
    c
}

/// A small section header for a collapsible palette panel (`.palette-body` /
/// `.palette-windows`). Hidden on desktop (the sidebar shows both panels with
/// their own structure); on mobile it labels each expanded panel so a user can
/// tell the menu from the open-windows list when both are open.
fn panel_head(label: &str) -> Element {
    let head = util::create_element_with_class("div", "palette-panel-head");
    util::set_text(&head, label);
    head
}

/// Build the empty-window-area hint: a centered, theme-aware welcome that
/// orients a first-time user (or anyone who just closed every window) toward
/// the menu, with a short breakdown of what each menu section offers. Plain
/// text — no fake link/arrow (the menu is a left sidebar on desktop but the
/// `☰ Menu` toggle on mobile, and there's nothing here to click).
fn build_empty_state() -> Element {
    let wrap = util::create_element_with_class("div", "window-empty-state");
    util::set_attr(
        &wrap,
        "style",
        "flex:1;display:flex;flex-direction:column;align-items:center;\
         justify-content:center;text-align:center;gap:14px;padding:32px;\
         color:var(--text-dim, #888);font-family:var(--font-ui, system-ui, sans-serif);",
    );

    let title = util::create_element("div");
    util::set_attr(
        &title,
        "style",
        "font-size:18px;font-weight:600;color:var(--text, #e0e0e0);",
    );
    util::set_text(&title, "No windows open");
    util::append(&wrap, &title);

    let body = util::create_element("div");
    util::set_attr(&body, "style", "font-size:13px;max-width:380px;line-height:1.5;");
    util::set_text(
        &body,
        "Entity Browser is a workspace over your entity tree. \
         Pick a window from the menu to get started.",
    );
    util::append(&wrap, &body);

    // A small legend of the menu sections so a first-timer knows what's inside.
    let legend = util::create_element("div");
    util::set_attr(
        &legend,
        "style",
        "display:flex;flex-direction:column;gap:6px;text-align:left;\
         font-size:12.5px;max-width:380px;",
    );
    for cat in crate::window::WindowCategory::all() {
        let row = util::create_element("div");
        let name = util::create_element_with_class("span", "empty-state-cat");
        util::set_attr(
            &name,
            "style",
            "color:var(--accent, #90d0ff);font-weight:600;",
        );
        util::set_text(&name, cat.label());
        util::append(&row, &name);
        let desc = util::create_element("span");
        util::set_text(&desc, &format!(" — {}", cat.description()));
        util::append(&row, &desc);
        util::append(&legend, &row);
    }
    // The always-present "Open Windows" section (rounds out the menu picture).
    {
        let row = util::create_element("div");
        let name = util::create_element("span");
        util::set_attr(
            &name,
            "style",
            "color:var(--accent, #90d0ff);font-weight:600;",
        );
        util::set_text(&name, "Open Windows");
        util::append(&row, &name);
        let desc = util::create_element("span");
        util::set_text(&desc, " — jump to or close your active windows");
        util::append(&row, &desc);
        util::append(&legend, &row);
    }
    util::append(&wrap, &legend);

    // A light closing note on the two "full surface" modes, then an invitation
    // to explore — nothing prescriptive.
    let modes = util::create_element("div");
    util::set_attr(
        &modes,
        "style",
        "font-size:12.5px;max-width:380px;line-height:1.5;margin-top:4px;",
    );
    util::set_text(
        &modes,
        "Sites can open full-screen in Site Mode, and any window — games and \
         apps included — can be maximized to fill the screen. Explore and enjoy.",
    );
    util::append(&wrap, &modes);

    // Outbound link to the foundation — opens in a new tab (rel=noopener so the
    // new tab can't reach back into this one).
    let learn = util::create_element("div");
    util::set_attr(
        &learn,
        "style",
        "font-size:12.5px;max-width:380px;line-height:1.5;margin-top:8px;",
    );
    let learn_text = util::create_element("span");
    util::set_text(&learn_text, "Learn more and get involved at ");
    util::append(&learn, &learn_text);
    let link = util::create_element("a");
    util::set_attr(&link, "href", "https://entitychurchfoundation.org");
    util::set_attr(&link, "target", "_blank");
    util::set_attr(&link, "rel", "noopener noreferrer");
    util::set_attr(&link, "style", "color:var(--accent, #90d0ff);font-weight:600;");
    util::set_text(&link, "entitychurchfoundation.org");
    util::append(&learn, &link);
    util::append(&wrap, &learn);

    wrap
}
