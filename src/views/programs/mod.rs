//! Programs window — the native compute-program **launcher**.
//!
//! Lists the embedded native compute programs (Life / Snake / Asteroids —
//! authored once by workbench-go, imported hash-verified via
//! [`EMBEDDED_PROGRAMS`]) and runs the selected one **behind the L5 /
//! entity-apps boundary**: a sandboxed iframe booting browser-rust in stripped
//! `?app-host={key}` mode, whose inner peer runs the generic compute host
//! ([`crate::app_host::run_program`]). Compute never runs on the system peer —
//! the reframe §6-step-4 correction / discipline **D21**. This window is a
//! *launcher*, not a host: the program's clock, input capture, and display all
//! live inside the payload, behind the boundary.
//!
//! It reuses the entity-apps player wholesale ([`crate::dom::games::render_grid`]
//! / [`crate::dom::games::render_player`] with [`AppDelivery::Src`]) — a slimmer
//! sibling of the Apps window ([`crate::views::games::AppWindow`]), sourced from
//! the built-in program registry rather than a published-app catalog. Which
//! program is open is per-window view-state (survives rebuild/reload); the
//! honest-refusal gate now lives in the host (`run_program` renders a visible
//! refusal for a program binding a shape the L5 host doesn't drive).
//!
//! Kept (not retired) as the "entity native programs" top-level surface: today
//! it launches the built-in POC programs; it is where user / entity-native
//! programs running in local peers will surface later.

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

/// Entity type for **this** window's view-state.
///
/// It shares [`crate::views::games::AppViewState`]'s shape with the Apps
/// window but is deliberately a **different type**, because both persist at
/// `window_state_path` and a window id is reused across a reload: with one
/// shared type, a Programs window inheriting an Apps window's id would decode
/// its state and read a `selected` naming an app from another set as a program
/// key. A type guard cannot separate two windows that agree on their type, so
/// the separation has to be here (AP42).
///
/// **One-time cost, taken deliberately:** Programs state persisted before this
/// split carries the Apps type and is now refused, so a returning user's
/// "which program was open" resets once. That is `selected` on a launcher —
/// the alternative was leaving two windows sharing a reused key.
pub const PROGRAMS_VIEW_TYPE: &str = "app/state/programs_view";

use crate::program_host::bundle::EMBEDDED_PROGRAMS;

/// The save-path / grid key for built-in programs — distinct from the `games`
/// and `apps` sets even though programs persist no save-state yet.
pub const PROGRAMS_SET: &str = "programs";
use crate::window_watch::WindowWatch;

pub struct ProgramsWindow {
    pub window_id: WindowId,
    pub peer_id: String,
    watch: WindowWatch,
    /// The L5 player's host `message` listener for the current frame, owned for
    /// its lifetime and removed on rebuild / drop so listeners don't stack
    /// (mirrors [`crate::views::games::AppWindow`]). `None` while the launcher
    /// grid is showing.
    #[cfg(target_arch = "wasm32")]
    listener: std::cell::RefCell<Option<crate::dom::games::HostListener>>,
    /// The program mounted in this window, by name (see `AppWindow::running`).
    running: std::cell::RefCell<Option<String>>,
}

impl ProgramsWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        Self {
            window_id,
            peer_id,
            watch: WindowWatch::new(),
            #[cfg(target_arch = "wasm32")]
            listener: std::cell::RefCell::new(None),
            running: std::cell::RefCell::new(None),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Programs", // i18n-ignore — identity key; display via window.programs
            description: "Launch transferable compute programs (EXTENSION-COMPUTE) behind the app boundary", // i18n-ignore — dead_code
            scope: crate::window::WindowScope::Peer,
            create: |id, peer_id, pm| {
                let mut window = ProgramsWindow::new(id, peer_id.to_string());
                // Subscribe this window's own view-state path — `selected`
                // (which program is open) lives there and drives the
                // grid-or-player render. Nothing on the system peer is watched
                // anymore: no program mounts there.
                pm.watch_prefix(
                    &mut window.watch,
                    peer_id,
                    crate::app_paths::window_state_path(crate::app_paths::APP_ID, peer_id, id),
                );
                Box::new(window)
            },
        }
    }

    /// This window's view-state path (which program is open).
    fn state_path(&self) -> String {
        crate::app_paths::window_state_path(
            crate::app_paths::APP_ID,
            &self.peer_id,
            self.window_id,
        )
    }
}

impl WindowView for ProgramsWindow {
    fn title(&self) -> String {
        crate::i18n::window_title("Programs") // i18n-ignore — lookup key, resolves via catalog
    }

    fn running_app(&self) -> Option<String> {
        self.running.borrow().clone()
    }

    fn type_name(&self) -> &'static str {
        "Programs" // i18n-ignore — stable type identifier, not UI text
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, peers: &Peers) {
        // The launcher tiles and the player's back button both emit the shared
        // `dom::games::SELECT_EVENT` (value = program key, or "" to return to
        // the grid) — window-scoped, so it never collides with an Apps window.
        if let Action::WindowEvent { event, value, .. } = action {
            if event == crate::views::games::SELECT_EVENT {
                let st = crate::views::games::AppViewState {
                    selected: value.clone(),
                    ..Default::default()
                };
                peers.seed_write(
                    &self.peer_id,
                    self.state_path(),
                    st.to_entity_as(PROGRAMS_VIEW_TYPE),
                );
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
        use crate::apps::format::AppEntry;
        use crate::apps::paths;

        self.running.replace(None);
        // Drop any stale player listener before (re)building the section.
        if let Some(old) = self.listener.borrow_mut().take() {
            crate::dom::games::remove_listener(&old);
        }

        let title = crate::i18n::window_title("Programs"); // i18n-ignore — lookup key
        let selected = peers
            .get_entity(&self.peer_id, &self.state_path())
            .map(|e| {
                crate::views::games::AppViewState::from_entity_as(&e, PROGRAMS_VIEW_TYPE).selected
            })
            .unwrap_or_default();

        // Nothing selected → the launcher grid over the built-in programs. Each
        // program is an L5 app (`APP_TYPE_L5`): the payload is browser-rust in
        // stripped `?app-host={key}` mode, delivered by `src`.
        // The shared grid emits `"{set}/{id}"` since the Apps window started
        // hosting both app-sets; here the set is always [`PROGRAMS_SET`], and a
        // bare key persisted before that change still resolves.
        let selected_key = crate::views::games::parse_selection(&selected)
            .map(|(_, id)| id)
            .unwrap_or("");
        let program = EMBEDDED_PROGRAMS.iter().find(|p| p.key == selected_key);
        let Some(program) = program else {
            let entries: Vec<AppEntry> = EMBEDDED_PROGRAMS
                .iter()
                .map(|p| AppEntry {
                    id: p.key.to_string(),
                    name: p.name.to_string(),
                    description: p.description.to_string(),
                    glyph: (!p.glyph.is_empty()).then(|| p.glyph.to_string()),
                    app_type: Some(paths::APP_TYPE_L5.to_string()),
                    ..Default::default()
                })
                .collect();
            let tagged: Vec<(&'static str, &AppEntry)> =
                entries.iter().map(|e| (PROGRAMS_SET, e)).collect();
            let empty = crate::i18n::t("programs.empty", &[]);
            // No chips: the built-in program registry is one flat list, and a
            // filter row over a single category is chrome pretending to be a
            // control (the same rule `apps::category::chips_for` enforces).
            crate::dom::games::render_grid(
                container,
                ctx,
                &crate::dom::games::GridView {
                    entries: &tagged,
                    // No chips: the built-in program registry is one flat list,
                    // and a filter row over a single category is chrome
                    // pretending to be a control (the same rule
                    // `apps::category::chips_for` enforces).
                    chips: &[],
                    filter: "",
                    // No Saves either: programs persist no save state, so the
                    // panel would always be empty.
                    saves_entry: false,
                    title: &title,
                    empty_msg: &empty,
                },
            );
            return;
        };

        // A program is selected → run it behind the boundary. The same L5
        // delivery the Apps window uses for an `APP_TYPE_L5` app: a sandboxed
        // iframe at `index.html?app-host={key}`. Compute runs in the payload's
        // inner peer, never here (D21). Programs are emit-only today, so there
        // is no save-state to seed (`init_state` empty).
        let cfg = crate::dom::games::GamesHostConfig {
            peer_id: self.peer_id.clone(),
            // Keys the save path; `programs` keeps it distinct from the games /
            // apps sets even though programs don't persist save-state yet.
            set: PROGRAMS_SET.to_string(),
            back_label: title,
            size: None,
            game_id: program.key.to_string(),
            game_name: program.name.to_string(),
            bundle_html: String::new(), // ignored for `Src` delivery
            delivery: crate::dom::games::AppDelivery::Src(format!(
                "index.html?app-host={}",
                program.key
            )),
            init_state: String::new(),
            init_save_hash: None,
            // A built-in program is our own L5 payload with no file surface.
            files: false,
            assets: None,
            workspace: None,
            // Programs report no running-app key, so the window's size key is
            // its type — the one the renderer computes for it.
            size_key: crate::window_size::size_key("Programs", None), // i18n-ignore — stable type identifier
        };
        self.running.replace(Some(cfg.game_name.clone()));
        let listener = crate::dom::games::render_player(container, peers, ctx, &cfg);
        *self.listener.borrow_mut() = listener;
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for ProgramsWindow {
    fn drop(&mut self) {
        if let Some(l) = self.listener.borrow_mut().take() {
            crate::dom::games::remove_listener(&l);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program_host::bundle::Bundle;
    use crate::program_host::descriptor::{
        ProgramDescriptor, SHAPE_DIRECTION, SHAPE_DISPLAY_LIST, SHAPE_KEY_SET, SHAPE_TEXT,
    };

    #[test]
    fn window_type_is_peer_scoped() {
        let t = ProgramsWindow::window_type();
        assert_eq!(t.name, "Programs");
        assert!(matches!(t.scope, crate::window::WindowScope::Peer));
    }

    /// Every embedded POC program admits against the shape set the L5 host
    /// drives (text + display-list display; direction + key-set input). This is
    /// the union `app_host` enforces at mount; keeping the fixtures inside it is
    /// what lets the launcher run all three behind the boundary. A program that
    /// grew an unsupported shape (or a capability import) would fail here — and,
    /// at runtime, surface a visible in-iframe refusal (`run_program`).
    #[test]
    fn embedded_programs_admit_on_l5_host() {
        // Mirror of `app_host::SUPPORTED_DISPLAY_SHAPES ∪ input::SUPPORTED_INPUT_SHAPES`
        // (that module is wasm-only; this native test pins the same set).
        let l5_shapes = [SHAPE_TEXT, SHAPE_DISPLAY_LIST, SHAPE_DIRECTION, SHAPE_KEY_SET];
        for p in EMBEDDED_PROGRAMS {
            let bundle = Bundle::parse(p.json).unwrap_or_else(|e| panic!("{}: parse: {e}", p.key));
            let be = bundle
                .entities
                .iter()
                .find(|e| e.path == bundle.descriptor_path)
                .unwrap_or_else(|| panic!("{}: no descriptor entity", p.key));
            let ent = entity_entity::Entity::new(&be.entity_type, be.data.clone())
                .unwrap_or_else(|e| panic!("{}: entity: {e}", p.key));
            let desc = ProgramDescriptor::decode(&ent)
                .unwrap_or_else(|e| panic!("{}: decode: {e}", p.key));
            desc.admit(&l5_shapes)
                .unwrap_or_else(|e| panic!("{} should admit on the L5 host: {e}", p.key));
        }
    }
}
