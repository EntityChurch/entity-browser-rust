//! Settings model — typed accessor for the global settings entity.
//!
//! Architecture:
//!
//! - Settings state is **global** (single entity at
//!   `app/entity-browser/settings/ui`). Multiple Settings window
//!   instances share it.
//! - The model holds **no in-memory state**. Every action does
//!   read-modify-write directly against the tree; render reads from
//!   the tree. An in-memory mirror would go stale when another
//!   Settings window writes the shared path. Subscription would solve
//!   that but isn't worth the wiring for three trivial fields.
//! - Pull-based render: `render_output(&self, peers: &Peers)`
//!   reads the tree on demand. The renderer still receives a pure
//!   [`SettingsOutput`](super::output::SettingsOutput).
//!
//! No web-sys, no DOM imports.

use entity_entity::Entity;
use crate::peers::Peers;

use crate::window::WindowId;

use super::output::{
    PeerOption, ProfileOption, SessionSettings, SettingsOutput, SiteAppearanceOption, TargetOption,
    ThemeOption,
};
use crate::session_config::{self, BootSurface, Profile};

/// Tree path stem (under `app/{app-id}/settings/`) where the global
/// settings entity lives.
pub const SETTINGS_PATH: &str = "ui";

/// Persisted settings state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsState {
    pub theme: String,
    /// How the Content Site overlay is colored, independent of `theme` (the
    /// chrome theme). One of: `"site"` (the overlay's own palette — default),
    /// `"system"` (follow the chrome theme), or a registered theme name (strict
    /// override). See [`crate::theme_tokens::site_appearance_catalog`].
    pub site_appearance: String,
    pub auto_connect: bool,
    pub show_inspector: bool,
    /// Single-instance ("immutable") windows: when on, opening a window type
    /// that's already open for the same peer focuses the existing window
    /// instead of spawning a duplicate. Off by default (preserves the
    /// historical multi-instance behavior).
    pub singleton_windows: bool,
}

impl Default for SettingsState {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            site_appearance: "site".into(),
            auto_connect: false,
            show_inspector: true,
            singleton_windows: false,
        }
    }
}

impl SettingsState {
    pub fn from_entity(entity: &Entity) -> Self {
        let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        let map = match value.as_map() {
            Some(m) => m,
            None => return Self::default(),
        };

        let mut state = Self::default();
        for (k, v) in map {
            match k.as_text() {
                Some("theme") => {
                    if let Some(s) = v.as_text() {
                        state.theme = s.to_string();
                    }
                }
                Some("site_appearance") => {
                    if let Some(s) = v.as_text() {
                        state.site_appearance = s.to_string();
                    }
                }
                Some("auto_connect") => {
                    if let Some(b) = v.as_bool() {
                        state.auto_connect = b;
                    }
                }
                Some("show_inspector") => {
                    if let Some(b) = v.as_bool() {
                        state.show_inspector = b;
                    }
                }
                Some("singleton_windows") => {
                    if let Some(b) = v.as_bool() {
                        state.singleton_windows = b;
                    }
                }
                _ => {}
            }
        }
        state
    }

    pub fn to_entity(&self) -> Entity {
        let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
            "theme" => entity_ecf::text(&self.theme),
            "site_appearance" => entity_ecf::text(&self.site_appearance),
            "auto_connect" => entity_ecf::bool_val(self.auto_connect),
            "show_inspector" => entity_ecf::bool_val(self.show_inspector),
            "singleton_windows" => entity_ecf::bool_val(self.singleton_windows)
        });
        Entity::new("app/state/setting", data).unwrap()
    }
}

/// Settings model — stateless typed accessor.
#[derive(Debug)]
pub struct SettingsModel {
    window_id: WindowId,
    peer_id: String,
}

impl SettingsModel {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        Self { window_id, peer_id }
    }

    /// Resolve the global settings path for this peer.
    fn state_path(&self, _peers: &Peers) -> String {
        crate::app_paths::settings_path(crate::app_paths::APP_ID, &self.peer_id, SETTINGS_PATH)
    }

    /// Ensure the global settings entity exists in the tree (writes
    /// defaults if absent). Uses `dispatch_write` (fire-and-forget L1)
    /// so the path works in both Direct and Worker modes. The default
    /// state arrives on the next subscription event after the put
    /// round-trips — first render falls back to `SettingsState::default()`.
    pub fn ensure_state(&self, peers: &Peers) {
        let path = self.state_path(peers);
        if peers.get_entity(&self.peer_id, &path).is_none() {
            peers.dispatch_write(&self.peer_id, path, SettingsState::default().to_entity());
        }
    }

    fn read_state(&self, peers: &Peers) -> SettingsState {
        let path = self.state_path(peers);
        peers
            .get_entity(&self.peer_id, &path)
            .map(|e| SettingsState::from_entity(&e))
            .unwrap_or_default()
    }

    fn write_state(&self, peers: &Peers, state: &SettingsState) {
        let path = self.state_path(peers);
        peers.dispatch_write(&self.peer_id, path, state.to_entity());
    }

    // -- Action methods (read-modify-write directly to tree) --

    pub fn set_theme(&self, value: &str, peers: &Peers) {
        let mut state = self.read_state(peers);
        state.theme = value.to_string();
        self.write_state(peers, &state);
        // Recolor the live page + mirror the choice for next boot. The tree
        // write above is the durable record; this is the appearance side.
        crate::theme_tokens::apply_and_persist(value);
    }

    /// Set how the Content Site overlay is themed (`"site"` / `"system"` /
    /// a registered theme name). Mirrors [`set_theme`](Self::set_theme): the
    /// tree write is the durable record, `apply_site_appearance` recolors the
    /// live overlay (rewrites `#site-theme-vars`) + mirrors the choice for boot.
    pub fn set_site_appearance(&self, value: &str, peers: &Peers) {
        let mut state = self.read_state(peers);
        state.site_appearance = value.to_string();
        self.write_state(peers, &state);
        crate::theme_tokens::apply_site_appearance(value);
    }

    pub fn toggle_inspector(&self, peers: &Peers) {
        let mut state = self.read_state(peers);
        state.show_inspector = !state.show_inspector;
        self.write_state(peers, &state);
    }

    pub fn toggle_autoconnect(&self, peers: &Peers) {
        let mut state = self.read_state(peers);
        state.auto_connect = !state.auto_connect;
        self.write_state(peers, &state);
    }

    pub fn toggle_singleton_windows(&self, peers: &Peers) {
        let mut state = self.read_state(peers);
        state.singleton_windows = !state.singleton_windows;
        self.write_state(peers, &state);
    }

    // -- Session config (Site & Surface) — the startup-surface (peer, kind,
    //    target) triple. Thin controller: this computes a *complete* surface
    //    (sensible defaults, scope-valid window types) and delegates the write
    //    to the cohesive `session_config` mutators (config logic + tests live
    //    there). The config is the system peer's (this window is System-scoped,
    //    so `self.peer_id` IS the system peer). --

    pub fn set_profile(&self, value: &str, peers: &Peers) {
        let profile = match value {
            "full" => Profile::Full,
            "tutorial" => Profile::Tutorial,
            "strict-site" => Profile::StrictSite,
            _ => return,
        };
        session_config::set_profile(peers, &self.peer_id, profile);
    }

    /// Is `pid` the system peer? Routed through the single accessor.
    fn is_system_peer(&self, peers: &Peers, pid: &str) -> bool {
        pid == peers.system_peer_id()
    }

    /// The peer the startup target currently lives on, for the UI's selection +
    /// the mutators' "keep current peer" logic. Window → its peer; Site →
    /// home_site's peer; Chrome → system. Empty (= "system, resolved at boot")
    /// resolves to the system peer.
    fn current_boot_peer(&self, peers: &Peers) -> String {
        let cfg = session_config::read(peers, &self.peer_id);
        let raw = match &cfg.boot_surface {
            BootSurface::Window { peer_id, .. } => peer_id.clone(),
            BootSurface::Site => cfg.home_site.peer_id.clone(),
            BootSurface::Chrome => String::new(),
        };
        if raw.is_empty() {
            peers.system_peer_id().to_string()
        } else {
            raw
        }
    }

    /// Window-type names valid as a boot target on `peer`. A non-system peer
    /// can only host **Peer-scoped** windows (System-scoped windows bind the
    /// system peer by definition — palette partition, handoff §4.4); the system
    /// peer can host any type (it also runs Peer-scoped windows, e.g. the
    /// default Entity Tree). Order preserved from the registry.
    fn scope_valid_window_types(&self, peers: &Peers, peer: &str) -> Vec<&'static str> {
        let system = self.is_system_peer(peers, peer);
        crate::window_registry::standard_window_type_meta()
            .into_iter()
            .filter(|(_, scope)| system || *scope == crate::window::WindowScope::Peer)
            .map(|(name, _)| name)
            .collect()
    }

    /// Change the boot-surface KIND (chrome/site/window), carrying the current
    /// peer over and picking a sensible default target for the new kind.
    pub fn set_boot_kind(&self, value: &str, peers: &Peers) {
        let peer = self.current_boot_peer(peers);
        match value {
            "chrome" => session_config::set_boot_surface(peers, &self.peer_id, BootSurface::Chrome),
            "site" => {
                // Site's peer rides on home_site; keep the current site id (or
                // default to the first site on the peer if none set).
                let cfg = session_config::read(peers, &self.peer_id);
                let id = if cfg.home_site.id.is_empty() {
                    crate::content_site::discovery::list_sites(peers, &peer)
                        .into_iter()
                        .next()
                        .unwrap_or_default()
                } else {
                    cfg.home_site.id
                };
                session_config::set_home_site(peers, &self.peer_id, &peer, &id);
                session_config::set_boot_surface(peers, &self.peer_id, BootSurface::Site);
                // Refresh the owned+cached site index so the target picker is
                // current when the user lands on Site kind.
                crate::content_site::discovery::refresh_site_index(peers, &self.peer_id);
            }
            "window" => {
                let window_type = self
                    .scope_valid_window_types(peers, &peer)
                    .first()
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                session_config::set_boot_surface(
                    peers,
                    &self.peer_id,
                    BootSurface::Window { peer_id: peer, window_type },
                );
            }
            _ => {}
        }
    }

    /// Change the target PEER, preserving the current kind. Re-validates the
    /// Window target type against the new peer's scope (drops an invalid one to
    /// the first valid type).
    pub fn set_boot_peer(&self, value: &str, peers: &Peers) {
        if value.is_empty() {
            return;
        }
        let cfg = session_config::read(peers, &self.peer_id);
        match &cfg.boot_surface {
            BootSurface::Window { window_type, .. } => {
                let valid = self.scope_valid_window_types(peers, value);
                let window_type = if valid.iter().any(|t| t == window_type) {
                    window_type.clone()
                } else {
                    valid.first().map(|s| s.to_string()).unwrap_or_default()
                };
                session_config::set_boot_surface(
                    peers,
                    &self.peer_id,
                    BootSurface::Window { peer_id: value.to_string(), window_type },
                );
            }
            BootSurface::Site => {
                session_config::set_home_site(peers, &self.peer_id, value, &cfg.home_site.id);
            }
            BootSurface::Chrome => {} // no peer relevance for chrome
        }
    }

    /// Change the TARGET (site id or window type) within the current kind.
    pub fn set_boot_target(&self, value: &str, peers: &Peers) {
        let cfg = session_config::read(peers, &self.peer_id);
        match &cfg.boot_surface {
            BootSurface::Window { peer_id, .. } => {
                session_config::set_boot_surface(
                    peers,
                    &self.peer_id,
                    BootSurface::Window {
                        peer_id: peer_id.clone(),
                        window_type: value.to_string(),
                    },
                );
            }
            BootSurface::Site => {
                // The Site target carries its OWN peer now (the picker lists
                // owned + cached sites across the universal tree): value is
                // `{peer}/{site}`. Fall back to the current home_site peer for a
                // bare site id (backward compat / owned-only callers).
                let (peer, id) = match value.split_once('/') {
                    Some((p, s)) => (p.to_string(), s.to_string()),
                    None => (cfg.home_site.peer_id.clone(), value.to_string()),
                };
                session_config::set_home_site(peers, &self.peer_id, &peer, &id);
            }
            BootSurface::Chrome => {}
        }
    }

    pub fn toggle_show_toggle(&self, peers: &Peers) {
        session_config::toggle_show_toggle(peers, &self.peer_id);
    }

    /// Toggle Phase-1 fast paint (cut 2c) and refresh the pre-peer localStorage
    /// mirror so the very next reload honors it (the durable config the toggle
    /// writes isn't readable before the peer exists).
    pub fn toggle_fast_paint(&self, peers: &Peers) {
        let value = session_config::toggle_fast_paint(peers, &self.peer_id);
        #[cfg(target_arch = "wasm32")]
        crate::boot_fast_paint::write_enabled_mirror(value);
        #[cfg(not(target_arch = "wasm32"))]
        let _ = value;
    }

    // -- Pure read API (called by renderer) --

    /// Materialize the renderer-neutral output. Pull-based: reads the
    /// tree on demand. Returns a struct with no peer references —
    /// renderer can build DOM without further I/O.
    #[allow(dead_code)] // called from WASM render path
    pub fn render_output(&self, peers: &Peers) -> SettingsOutput {
        let state = self.read_state(peers);
        let state_path = self.state_path(peers);

        // Registry-driven: one radio per registered theme
        // (`theme_tokens::THEMES`). Adding a theme is one entry there.
        let themes = crate::theme_tokens::THEMES
            .iter()
            .map(|t| ThemeOption {
                value: t.name,
                label: t.label,
                selected: state.theme == t.name,
            })
            .collect();

        // Site appearance: two fixed modes ("Site's theme" / "Match system
        // theme") + a strict override per registered theme. Registry-driven via
        // the catalog so adding a theme adds an "Always X" override.
        let site_appearance = crate::theme_tokens::site_appearance_catalog()
            .into_iter()
            .map(|(value, label)| SiteAppearanceOption {
                selected: state.site_appearance == value,
                value: value.to_string(),
                label,
            })
            .collect();

        let cfg = session_config::read(peers, &self.peer_id);
        let profiles = vec![
            ProfileOption { value: "full", label: "Full (explore everything)", selected: cfg.profile == Profile::Full },
            ProfileOption { value: "tutorial", label: "Tutorial (boot into site)", selected: cfg.profile == Profile::Tutorial },
            ProfileOption { value: "strict-site", label: "Strict Site (locked)", selected: cfg.profile == Profile::StrictSite },
        ];

        // The startup-surface (peer, kind, target) triple.
        let boot_kind = cfg.boot_surface.kind_str();
        let selected_peer = self.current_boot_peer(peers);

        // Peer dropdown — reachable peers, the boot target pre-selected, default
        // system. Mirrors the command palette's selector.
        let peers_list: Vec<PeerOption> = peers
            .peer_ids()
            .into_iter()
            .filter(|pid| peers.has_peer_context(pid))
            .map(|pid| PeerOption {
                selected: pid == selected_peer,
                label: crate::views::display_name(peers, &pid),
                id: pid,
            })
            .collect();

        // Contextual target dropdown.
        let (targets, target_disabled) = match &cfg.boot_surface {
            BootSurface::Chrome => (Vec::new(), true),
            BootSurface::Site => {
                // Boot-target sites span the WHOLE universal tree — owned
                // (`/{me}/sites/...`) AND cached foreign (`/{them}/sites/...`,
                // P1 write-through). Read the derived index (refreshed async on
                // window create / kind-switch; this surface re-renders via the
                // index-path subscription). Each target carries its own peer as
                // `{peer}/{site}` so a cached foreign site is selectable even
                // though its peer has no local context (the gap the single-peer,
                // context-gated `list_sites` couldn't surface).
                let cur_peer = &cfg.home_site.peer_id;
                let cur_id = &cfg.home_site.id;
                // Union the derived index with the direct store scan so CACHED
                // foreign sites are selectable as boot targets, not just owned
                // ones — the picker must never omit a site the directory rail
                // can show (BUG-3 divergence). See `list_targetable_sites`.
                let targets = crate::content_site::discovery::list_targetable_sites(peers, &self.peer_id)
                    .into_iter()
                    .map(|sr| TargetOption {
                        selected: &sr.peer == cur_peer && &sr.site == cur_id,
                        label: if sr.owned { sr.site.clone() } else { format!("{} (cached)", sr.site) },
                        value: format!("{}/{}", sr.peer, sr.site),
                    })
                    .collect();
                (targets, false)
            }
            BootSurface::Window { window_type, .. } => {
                let targets = self
                    .scope_valid_window_types(peers, &selected_peer)
                    .into_iter()
                    .map(|name| TargetOption {
                        selected: name == window_type,
                        label: name.to_string(),
                        value: name.to_string(),
                    })
                    .collect();
                (targets, false)
            }
        };

        let session = SessionSettings {
            profiles,
            boot_kind,
            peers: peers_list,
            targets,
            target_disabled,
            show_toggle: cfg.site_mode.show_toggle,
            fast_paint: cfg.fast_paint,
            locked: cfg.site_mode.locked,
        };

        SettingsOutput {
            window_id: self.window_id,
            state_path,
            themes,
            site_appearance,
            show_inspector: state.show_inspector,
            auto_connect: state.auto_connect,
            singleton_windows: state.singleton_windows,
            session,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pm() -> Peers {
        Peers::new_direct()
    }

    async fn flush_writes() {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    #[test]
    fn state_default_values() {
        let s = SettingsState::default();
        assert_eq!(s.theme, "dark");
        assert_eq!(s.site_appearance, "site", "overlay defaults to its own theme");
        assert!(!s.auto_connect);
        assert!(s.show_inspector);
        assert!(!s.singleton_windows, "multi-instance is the default");
    }

    #[tokio::test]
    async fn toggle_singleton_windows_flips_value() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let model = SettingsModel::new(1, pid.clone());
        model.ensure_state(&pm);

        model.toggle_singleton_windows(&pm);
        flush_writes().await;

        let entity = pm.get_entity(&pid, &model.state_path(&pm)).unwrap();
        assert!(SettingsState::from_entity(&entity).singleton_windows);
    }

    #[test]
    fn state_round_trip() {
        let s = SettingsState {
            theme: "light".into(),
            site_appearance: "light".into(),
            auto_connect: true,
            show_inspector: false,
            singleton_windows: true,
        };
        let e = s.to_entity();
        let s2 = SettingsState::from_entity(&e);
        assert_eq!(s, s2);
    }

    #[tokio::test]
    async fn ensure_state_writes_defaults_when_absent() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let model = SettingsModel::new(1, pid.clone());
        model.ensure_state(&pm);
        // ensure_state uses dispatch_write (fire-and-forget L1 put).
        // Wait for the put to propagate before reading.
        flush_writes().await;

        let path = model.state_path(&pm);
        let entity = pm.get_entity(&pid, &path).unwrap();
        let state = SettingsState::from_entity(&entity);
        assert_eq!(state, SettingsState::default());
    }

    #[test]
    fn ensure_state_does_not_overwrite_existing() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let custom = SettingsState {
            theme: "light".into(),
            site_appearance: "system".into(),
            auto_connect: true,
            show_inspector: false,
            singleton_windows: true,
        };
        let path = crate::app_paths::settings_path(crate::app_paths::APP_ID, &pid, SETTINGS_PATH);
        let ctx = pm.test_seed_ctx(&pid);
        ctx.store().put(&path, custom.clone().to_entity()).ok();

        let model = SettingsModel::new(1, pid.clone());
        model.ensure_state(&pm);

        let entity = pm.get_entity(&pid, &path).unwrap();
        assert_eq!(SettingsState::from_entity(&entity), custom);
    }

    #[tokio::test]
    async fn set_theme_persists() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let model = SettingsModel::new(1, pid.clone());
        model.ensure_state(&pm);

        model.set_theme("light", &pm);
        flush_writes().await;

        let entity = pm.get_entity(&pid, &model.state_path(&pm)).unwrap();
        assert_eq!(SettingsState::from_entity(&entity).theme, "light");
    }

    #[tokio::test]
    async fn set_site_appearance_persists() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let model = SettingsModel::new(1, pid.clone());
        model.ensure_state(&pm);

        model.set_site_appearance("system", &pm);
        flush_writes().await;

        let entity = pm.get_entity(&pid, &model.state_path(&pm)).unwrap();
        assert_eq!(SettingsState::from_entity(&entity).site_appearance, "system");

        // The catalog is what the dropdown renders: "site"/"system" + a strict
        // override per theme. The persisted value selects one of them.
        let out = model.render_output(&pm);
        let sys = out
            .site_appearance
            .iter()
            .find(|o| o.value == "system")
            .expect("system option present");
        assert!(sys.selected, "persisted system value is pre-selected");
        assert!(out.site_appearance.iter().any(|o| o.value == "site"));
    }

    #[tokio::test]
    async fn toggle_inspector_flips_value() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let model = SettingsModel::new(1, pid.clone());
        model.ensure_state(&pm);

        model.toggle_inspector(&pm);
        flush_writes().await;

        let entity = pm.get_entity(&pid, &model.state_path(&pm)).unwrap();
        assert!(!SettingsState::from_entity(&entity).show_inspector);
    }

    #[tokio::test]
    async fn toggle_autoconnect_flips_value() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let model = SettingsModel::new(1, pid.clone());
        model.ensure_state(&pm);

        model.toggle_autoconnect(&pm);
        flush_writes().await;

        let entity = pm.get_entity(&pid, &model.state_path(&pm)).unwrap();
        assert!(SettingsState::from_entity(&entity).auto_connect);
    }

    #[tokio::test]
    async fn second_model_sees_first_models_writes() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let m1 = SettingsModel::new(1, pid.clone());
        let m2 = SettingsModel::new(2, pid.clone());
        m1.ensure_state(&pm);

        m1.set_theme("light", &pm);
        flush_writes().await;

        // m2 reads the same global path → sees m1's write.
        let out = m2.render_output(&pm);
        assert_eq!(out.themes[0].value, "dark");
        assert!(!out.themes[0].selected);
        assert_eq!(out.themes[1].value, "light");
        assert!(out.themes[1].selected);
    }

    #[test]
    fn render_output_reflects_default_state() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let model = SettingsModel::new(1, pid);
        model.ensure_state(&pm);

        let out = model.render_output(&pm);
        assert_eq!(out.themes.len(), 2);
        assert!(out.themes[0].selected); // "dark"
        assert!(!out.themes[1].selected);
        assert!(out.show_inspector);
        assert!(!out.auto_connect);
        assert!(out.state_path.contains("app/entity-browser/settings/ui"));
    }

    #[test]
    fn startup_default_is_chrome_with_disabled_target_and_system_peer_selected() {
        let pm = pm();
        let pid = pm.system_peer_id().to_string();
        let model = SettingsModel::new(1, pid.clone());
        let s = model.render_output(&pm).session;
        assert_eq!(s.boot_kind, "chrome");
        assert!(s.target_disabled, "chrome has no target");
        assert!(s.targets.is_empty());
        // The lone (system) peer is present and pre-selected.
        let sys = s.peers.iter().find(|p| p.id == pid).expect("system peer listed");
        assert!(sys.selected, "system peer is the default boot target");
    }

    #[test]
    fn set_boot_kind_window_populates_scope_valid_targets() {
        let pm = pm();
        let pid = pm.system_peer_id().to_string();
        let model = SettingsModel::new(1, pid);
        model.set_boot_kind("window", &pm);
        let s = model.render_output(&pm).session;
        assert_eq!(s.boot_kind, "window");
        assert!(!s.target_disabled);
        // On the system peer ALL 19 window types are valid targets, and exactly
        // one is pre-selected (the default the mutator picked).
        assert_eq!(s.targets.len(), 19, "system peer hosts every window type");
        assert_eq!(s.targets.iter().filter(|t| t.selected).count(), 1);
    }

    #[test]
    fn set_boot_target_changes_the_window_type() {
        let pm = pm();
        let pid = pm.system_peer_id().to_string();
        let model = SettingsModel::new(1, pid.clone());
        model.set_boot_kind("window", &pm);
        model.set_boot_target("Shell", &pm);
        let cfg = crate::session_config::read(&pm, &pid);
        assert_eq!(
            cfg.boot_surface,
            BootSurface::Window { peer_id: pid.clone(), window_type: "Shell".into() }
        );
    }

    #[test]
    fn site_targets_include_cached_foreign_sites() {
        // The boot-target picker must offer CACHED foreign sites, not just owned
        // ones — the gap the user hit (booted into a cached site, couldn't set it
        // as a target). A cached manifest physically in my store + its origin,
        // with NO async index refresh (plain #[test] = no runtime → the refresh
        // task is dropped), must still surface via the direct scan union.
        use crate::content_site::format::SiteManifest;
        use crate::content_site::{origins, paths};
        let pm = pm();
        let pid = pm.system_peer_id().to_string();
        let foreign = Peers::new_direct().primary_peer_id().to_string();
        pm.seed_write(
            &pid,
            paths::manifest_path(&foreign, "labs"),
            SiteManifest::new("labs", "Labs", "index", vec![]).to_entity(),
        );
        origins::set_origin(&pm, &pid, &foreign, "http://labs.example");

        let model = SettingsModel::new(1, pid.clone());
        model.set_boot_kind("site", &pm);
        let s = model.render_output(&pm).session;
        assert_eq!(s.boot_kind, "site");
        let cached = s
            .targets
            .iter()
            .find(|t| t.value == format!("{}/labs", foreign))
            .expect("cached foreign site offered as a boot target");
        assert!(cached.label.contains("cached"), "cached label: {}", cached.label);
    }

    #[test]
    fn set_boot_target_site_parses_peer_and_site_for_cached_targets() {
        // The Site picker lists owned + cached sites across the universal tree,
        // each target carrying its own peer as `{peer}/{site}`. Booting into a
        // CACHED foreign site must set home_site to that foreign peer — not the
        // current/boot peer (the gap that made foreign sites unselectable).
        let pm = pm();
        let pid = pm.system_peer_id().to_string();
        let model = SettingsModel::new(1, pid.clone());
        model.set_boot_kind("site", &pm);
        model.set_boot_target("FOREIGNPEER/labs", &pm);
        let cfg = crate::session_config::read(&pm, &pid);
        assert!(matches!(cfg.boot_surface, BootSurface::Site));
        assert_eq!(cfg.home_site.peer_id, "FOREIGNPEER");
        assert_eq!(cfg.home_site.id, "labs");

        // A bare site id (owned-only / backward compat) keeps the current peer.
        model.set_boot_target("aboutme", &pm);
        let cfg = crate::session_config::read(&pm, &pid);
        assert_eq!(cfg.home_site.peer_id, "FOREIGNPEER", "bare id keeps current peer");
        assert_eq!(cfg.home_site.id, "aboutme");
    }
}
