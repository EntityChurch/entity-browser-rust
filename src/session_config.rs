//! Session Configuration — the spine that ties boot, surface, and content
//! together (boot-config-surfaces reframe §4-A).
//!
//! Before this module, three distinct concepts were fused into one
//! hand-wired POC: *which configuration the app boots into*, *which surface
//! is showing*, and *which content fills it*. [`SiteModeState`] mashed mode +
//! posture; "which site" was a hard-coded `DEMO_SITE_ID` constant. This
//! module is the single tree-backed config entity that splits them cleanly:
//!
//!   * **`boot_surface`** — the primary axis: which surface boot lands in — the
//!     entity-browser `Chrome`, a content `Site`, or a maximized `Window` (the
//!     §4-B Surfaces seam). This replaced the old opaque `full`/`site`/`strict-site`
//!     *profile* presets: a startup posture **is** a surface + a granular
//!     posture (`site_mode` / `peer_creation_enabled`), set directly, not
//!     bundled behind a preset name.
//!   * **`home_site`** — the site the overlay shows by default (where a `Site`
//!     boot lands, what the chrome toggle opens). The *current* location the
//!     user browsed to persists separately on the overlay
//!     ([`ContentSiteState`](crate::views::content_site::model)); this is the
//!     **default** the force-default loader resets to (§4-C).
//!   * **`site_mode`** — the overlay posture: is it available, is the toggle
//!     shown, is it locked (lockdown is a **held seam** — the field is stored
//!     and read but no behavior gates on it yet, §4-C "hold the seam, defer
//!     the feature").
//!   * **`active`** — the **runtime** surface flag ("is the overlay showing
//!     now"). DERIVED at boot from `boot_surface` (boot lands per config, not
//!     wherever a previous session's toggle last left it — reframe §4); the
//!     status-bar toggle flips it live during a session.
//!
//! Config (`boot_surface` / `home_site` / `site_mode`) is durable
//! and **preserved** across a warm boot — re-seeding a default over persisted
//! config was the original clobber bug. The owned boot-load step
//! ([`EntityApp::boot_load`](crate::app::EntityApp::boot_load)) reads the
//! durable entity, preserves the config, derives `active`, and writes it back
//! awaited + cache-reflected.
//!
//! Non-DOM and unit-testable: tree read/modify/write through [`Peers`],
//! arm-safe (`get_entity` + `seed_write` route through the router, so this
//! works in both the Direct and Worker arms — D15).

use entity_entity::Entity;

use crate::peers::Peers;

/// Settings stem (under `app/{app-id}/settings/`) for the session entity.
const SETTINGS_STEM: &str = "session";
/// Workspace stem (under `app/{app-id}/workspace/`) for the overlay's
/// persisted current location (the overlay surface's nav state, distinct
/// from the config: config is the *default*, this is *where you are now*).
const OVERLAY_LOCATION_STEM: &str = "site-overlay/location";
/// Entity type for the persisted session config.
const STATE_TYPE: &str = "app/state/session_config";

/// The bundled demo site id — the default `home_site` content. Seeded by
/// [`ensure_demo_site`](crate::views::content_site::ensure_demo_site). This
/// is the *content* id, not a boot-path pointer: the app reaches it through
/// `home_site`, never a hard-coded constant.
pub const DEMO_SITE_ID: &str = "demo";

/// The window type the "show my site" posture boots maximized (the Site
/// Browser) — the publish default `--surface=window --window-type` and the
/// settings surface point at it. Must match a registered window type name
/// (`window_registry::standard_window_types`); the `site_browser_window_is_registered`
/// test pins it so a rename can't silently make the surface boot into the void.
/// This is the "maximized Site Browser" surface — chosen over the site overlay
/// for the default show-sites posture: it lives in the normal chrome (the user
/// can un-maximize, open other windows, browse the directory rail), so it's more
/// forgiving than the kiosk-like overlay. The overlay (`BootSurface::Site`) is
/// reserved for the locked-kiosk posture (surface=site + `site_mode.locked`).
// Native publish (`--window-type` default) + tests reference it; the wasm
// runtime doesn't call it directly anymore (the old `Profile::Site` preset did),
// so it reads as dead on wasm32 — keep it as the canonical name, don't inline it.
#[allow(dead_code)]
pub const SITE_BROWSER_WINDOW: &str = "Site Browser";

/// Which surface boot lands in (reframe §4-B).
///
/// The boot target is a **(peer, target)** pair — the peer dimension that the
/// first cut dropped (handoff §3). `Window` carries the target peer explicitly;
/// an **empty `peer_id` means "the system peer, resolved at boot"** (presets
/// are `const` and can't bake a runtime peer-id). `Chrome` has no target;
/// `Site`'s peer rides on [`SessionConfig::home_site`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootSurface {
    /// The entity-browser window chrome (windows + status bar).
    Chrome,
    /// A content site overlay, pointed at [`SessionConfig::home_site`].
    Site,
    /// A single maximized window of `window_type`, spawned on `peer_id`
    /// (empty = system peer). The §4-B Surfaces seam — a window generalizes
    /// to a base surface (C1a). The `(peer, type)` is the durable identifier;
    /// the ephemeral window id is re-spawned at boot.
    Window { peer_id: String, window_type: String },
}

impl BootSurface {
    /// The `boot_surface_kind` discriminant (`chrome` / `site` / `window`) —
    /// the persisted form and the settings radio-group value.
    pub fn kind_str(&self) -> &'static str {
        match self {
            BootSurface::Chrome => "chrome",
            BootSurface::Site => "site",
            BootSurface::Window { .. } => "window",
        }
    }

    /// Human-readable form for logs (NOT persistence — the entity stores
    /// structured fields). `window:{peer}:{type}`.
    pub fn describe(&self) -> String {
        match self {
            BootSurface::Chrome => "chrome".to_string(),
            BootSurface::Site => "site".to_string(),
            BootSurface::Window { peer_id, window_type } => {
                let p = if peer_id.is_empty() { "system" } else { peer_id.as_str() };
                format!("window:{p}:{window_type}")
            }
        }
    }
}

/// Build a [`BootSurface`] from the `(kind, peer_id, window_type)` strings —
/// the shared **surface vocabulary** (`chrome` / `site` / `window`) spoken by
/// the build-time default ([`boot_default`]), the per-domain deployment config
/// ([`crate::deployment_config`]), and the settings surface. An empty
/// `peer_id` on a `window` = "the system peer, resolved at boot". An unknown /
/// absent kind falls back to `Chrome` (garbage-tolerant, like `from_entity`).
pub fn boot_surface_from(kind: &str, peer_id: &str, window_type: &str) -> BootSurface {
    match kind {
        "site" => BootSurface::Site,
        "window" => BootSurface::Window {
            peer_id: peer_id.to_string(),
            window_type: window_type.to_string(),
        },
        _ => BootSurface::Chrome,
    }
}

/// A reference to a site + page within it, on a specific peer. `peer_id` empty
/// = the system peer (resolved at boot — sites are cross-peer,
/// `entity://{peer}/sites/{id}/...`). `loc` empty = the manifest root
/// page. (§4-A `site:{id}@{loc}`, now peer-qualified.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteRef {
    pub peer_id: String,
    pub id: String,
    pub loc: String,
}

/// The content-site overlay's posture (availability / chrome toggle /
/// lockdown). `locked` is a **held seam** — stored and readable, but no
/// behavior gates on it yet (§4-C).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteModePosture {
    /// Whether the site overlay is available at all.
    pub enabled: bool,
    /// Whether the chrome ↔ site toggle is shown.
    pub show_toggle: bool,
    /// Lockdown: no exit / restricted nav. Enforced (`ToggleSiteMode` no-ops
    /// when set; the overlay renders no Exit control) since the BUG-1 fix
    /// — previously a held seam.
    pub locked: bool,
}

impl SiteModePosture {
    /// Whether the chrome↔site toggle is exposed to the user — the single
    /// predicate for the status-bar toggle (`apply_site_mode`) AND the
    /// overlay-side "Exit Site" control (`SiteOverlay::render` → `can_exit`).
    /// A locked/strict-site deployment (`show_toggle=false`) returns `false`,
    /// so neither affordance is rendered and the user can't strand themselves
    /// in chrome (BUG-1).
    pub fn exposes_toggle(&self) -> bool {
        self.show_toggle && self.enabled
    }
}

/// The session configuration entity — the spine (§4-A).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    pub boot_surface: BootSurface,
    pub home_site: SiteRef,
    pub site_mode: SiteModePosture,
    /// Runtime surface flag — overlay showing now. Derived at boot from
    /// `boot_surface`; toggled live. Not part of the durable *config* proper,
    /// but persisted so the per-frame `apply_site_mode` read and the toggle's
    /// reactivity seam work.
    pub active: bool,
    /// Phase-1 fast paint (cut 2c) — paint the configured remote home over
    /// HTTP into `#site-layer` while the peer boots. Default on; user-flippable
    /// kill switch (mirrored to localStorage so the pre-peer boot path, which
    /// can't read this durable tree config, can honor it — see
    /// [`crate::boot_fast_paint`]). Only has effect for a remote-home,
    /// boots-into-site deployment; inert otherwise.
    pub fast_paint: bool,
    /// **Capability** posture (MAP §5 dimension B / §10 item 1b): may the user
    /// create new peers in this deployment? Default **true** (the full
    /// explorable browser); a locked-kiosk deployment sets it **false**
    /// (via `/entity-deployment.json`'s `peer_creation_enabled`) so the create
    /// affordance is hidden and `CreatePeerWithMode` is refused (closes L-3 —
    /// peer creation was reachable in *every* posture). An independent axis from
    /// the surface (MAP §5): a chrome deployment can still disable creation.
    /// Absent in a pre-1b persisted config → defaults true (from `default()`),
    /// so existing full deployments are unaffected.
    pub peer_creation_enabled: bool,
}

impl Default for SessionConfig {
    /// The chrome-first default posture — the workspace (window manager),
    /// toggle available, demo site as home, fully creatable. Reproduces the
    /// legacy `SiteModeState::default()` behavior so the boot path is unchanged
    /// for the default deployment. A non-default startup posture is set
    /// directly (surface + `site_mode` + `peer_creation_enabled`), never via a
    /// preset name.
    fn default() -> Self {
        SessionConfig {
            boot_surface: BootSurface::Chrome,
            home_site: home_site_default(),
            site_mode: SiteModePosture { enabled: true, show_toggle: true, locked: false },
            active: false,
            fast_paint: true,
            peer_creation_enabled: true,
        }
    }
}

impl SessionConfig {
    /// Whether the overlay surface should be showing, derived purely from
    /// `boot_surface`. Boot lands per config, not per a stale runtime toggle.
    /// `Window` is a non-overlay surface (step 5), so it does not light the
    /// site overlay.
    pub fn active_from_boot_surface(&self) -> bool {
        matches!(self.boot_surface, BootSurface::Site)
    }

    pub fn from_entity(entity: &Entity) -> Self {
        let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
            Ok(v) => v,
            Err(_) => return Self::default(),
        };
        let map = match value.as_map() {
            Some(m) => m,
            None => return Self::default(),
        };
        // `boot_surface` is stored as three structured fields (kind / peer /
        // window) rather than one packed string — no delimiter ambiguity with
        // peer-id contents (handoff §3). Collect them, then assemble below.
        let mut boot_kind: Option<String> = None;
        let mut boot_peer = String::new();
        let mut boot_window = String::new();
        let mut cfg = Self::default();
        for (k, v) in map {
            match k.as_text() {
                // A legacy `profile` key (pre-surface-axis) is simply ignored —
                // the surface + posture fields below carry the whole config now.
                Some("boot_surface_kind") => {
                    if let Some(s) = v.as_text() {
                        boot_kind = Some(s.to_string());
                    }
                }
                Some("boot_surface_peer") => {
                    if let Some(s) = v.as_text() {
                        boot_peer = s.to_string();
                    }
                }
                Some("boot_surface_window") => {
                    if let Some(s) = v.as_text() {
                        boot_window = s.to_string();
                    }
                }
                Some("home_site_peer") => {
                    if let Some(s) = v.as_text() {
                        cfg.home_site.peer_id = s.to_string();
                    }
                }
                Some("home_site_id") => {
                    if let Some(s) = v.as_text() {
                        cfg.home_site.id = s.to_string();
                    }
                }
                Some("home_site_loc") => {
                    if let Some(s) = v.as_text() {
                        cfg.home_site.loc = s.to_string();
                    }
                }
                Some("site_enabled") => {
                    if let Some(b) = v.as_bool() {
                        cfg.site_mode.enabled = b;
                    }
                }
                Some("show_toggle") => {
                    if let Some(b) = v.as_bool() {
                        cfg.site_mode.show_toggle = b;
                    }
                }
                Some("locked") => {
                    if let Some(b) = v.as_bool() {
                        cfg.site_mode.locked = b;
                    }
                }
                Some("active") => {
                    if let Some(b) = v.as_bool() {
                        cfg.active = b;
                    }
                }
                Some("fast_paint") => {
                    if let Some(b) = v.as_bool() {
                        cfg.fast_paint = b;
                    }
                }
                // Absent in a pre-1b persisted config → keeps `default()`'s
                // `true` (existing full deployments stay creatable). Garbage-
                // tolerant like every other field.
                Some("peer_creation_enabled") => {
                    if let Some(b) = v.as_bool() {
                        cfg.peer_creation_enabled = b;
                    }
                }
                _ => {}
            }
        }
        // Assemble boot_surface from the structured fields. An unknown / absent
        // kind keeps the default (`Full` → Chrome) — garbage-tolerant.
        cfg.boot_surface = match boot_kind.as_deref() {
            Some("chrome") => BootSurface::Chrome,
            Some("site") => BootSurface::Site,
            Some("window") => BootSurface::Window {
                peer_id: boot_peer,
                window_type: boot_window,
            },
            _ => cfg.boot_surface,
        };
        cfg
    }

    pub fn to_entity(&self) -> Entity {
        // `boot_surface` → three structured fields; non-window surfaces store
        // empty peer/window (round-trips back to the same enum).
        let (boot_peer, boot_window) = match &self.boot_surface {
            BootSurface::Window { peer_id, window_type } => {
                (peer_id.as_str(), window_type.as_str())
            }
            _ => ("", ""),
        };
        let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
            "boot_surface_kind" => entity_ecf::text(self.boot_surface.kind_str()),
            "boot_surface_peer" => entity_ecf::text(boot_peer),
            "boot_surface_window" => entity_ecf::text(boot_window),
            "home_site_peer" => entity_ecf::text(&self.home_site.peer_id),
            "home_site_id" => entity_ecf::text(&self.home_site.id),
            "home_site_loc" => entity_ecf::text(&self.home_site.loc),
            "site_enabled" => entity_ecf::bool_val(self.site_mode.enabled),
            "show_toggle" => entity_ecf::bool_val(self.site_mode.show_toggle),
            "locked" => entity_ecf::bool_val(self.site_mode.locked),
            "active" => entity_ecf::bool_val(self.active),
            "fast_paint" => entity_ecf::bool_val(self.fast_paint),
            "peer_creation_enabled" => entity_ecf::bool_val(self.peer_creation_enabled)
        });
        Entity::new(STATE_TYPE, data).unwrap()
    }
}

/// Tree path of the session config entity for `peer_id`.
pub fn state_path(peer_id: &str) -> String {
    crate::app_paths::settings_path(crate::app_paths::APP_ID, peer_id, SETTINGS_STEM)
}

/// Tree path of the overlay's persisted current location for `peer_id`.
/// App-level (not per-window) — the overlay is its own surface. This is the
/// runtime *location*, distinct from the config's `home_site` *default*.
pub fn overlay_location_path(peer_id: &str) -> String {
    crate::app_paths::workspace_path(crate::app_paths::APP_ID, peer_id, OVERLAY_LOCATION_STEM)
}

/// Assemble the default [`SiteRef`] from the (peer, site, loc) triple, applying
/// the empty-→-fallback rules. Pure (no env read) so the fallback logic is
/// unit-testable; [`home_site_default`] feeds it the `ENTITY_HOME_*` values.
/// An empty/absent site id falls back to the bundled demo; an empty peer means
/// "local/system peer" (the common case).
fn home_site_from(peer: Option<&str>, site: Option<&str>, loc: Option<&str>) -> SiteRef {
    fn trim(o: Option<&str>) -> Option<&str> {
        o.map(str::trim).filter(|s| !s.is_empty())
    }
    SiteRef {
        peer_id: trim(peer).unwrap_or("").to_string(),
        id: trim(site).unwrap_or(DEMO_SITE_ID).to_string(),
        loc: trim(loc).unwrap_or("").to_string(),
    }
}

/// The build-time default home site (boot-closure cut 2a) — `ENTITY_HOME_*`
/// baked by `build.rs`, or the bundled local demo when unset. This is the
/// *test / build-default* layer of the deployment-config precedence; the
/// production knob is the per-domain config fetch (cut 2b). A persisted
/// `home_site` always wins on a warm boot.
pub fn home_site_default() -> SiteRef {
    home_site_from(
        option_env!("ENTITY_HOME_PEER"),
        option_env!("ENTITY_HOME_SITE"),
        option_env!("ENTITY_HOME_LOC"),
    )
}

/// The build-time HTTP origin for the default home peer (`ENTITY_HOME_ORIGIN`),
/// or `None` when unset. Where the home peer's published artifacts live —
/// [`EntityApp::boot_load`] seeds it into the site-origin registry so a remote
/// thin-lens home resolves over HTTP-poll on first browse. Trailing slash is
/// trimmed by the origin-registry encoder.
pub fn home_origin_default() -> Option<String> {
    option_env!("ENTITY_HOME_ORIGIN")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The cold-boot default session config — the build-time startup **surface**
/// (`ENTITY_STARTUP_SURFACE` + `ENTITY_STARTUP_WINDOW_TYPE`; see `build.rs`).
/// [`EntityApp::boot_load`] uses this when no durable config exists, so a fresh
/// / wiped deployment lands in its baked surface (a `site` build cold-boots
/// into the site overlay). A persisted config always wins, so this only shapes
/// the *absent* case. Only the **surface** is baked — the granular posture
/// (`site_mode` / `peer_creation_enabled`) keeps the escapable, creatable
/// default; a locked kiosk is expressed per-domain via `/entity-deployment.json`
/// (the real deployment mechanism), not the build knob. Distinct from
/// [`SessionConfig::default`] (always `Chrome`), the type-level fallback.
pub fn boot_default() -> SessionConfig {
    let kind = option_env!("ENTITY_STARTUP_SURFACE").unwrap_or("chrome");
    let window_type = option_env!("ENTITY_STARTUP_WINDOW_TYPE").unwrap_or("");
    let mut cfg = SessionConfig {
        boot_surface: boot_surface_from(kind, "", window_type),
        ..SessionConfig::default()
    };
    // `active` is re-derived by boot_load, but keep the type honest here too.
    cfg.active = cfg.active_from_boot_surface();
    cfg
}

/// Why peer creation is refused right now, or `None` when it's allowed (MAP
/// §10 items 1a + 1b). Pure decision shared by the hard action guard
/// (`CreatePeerWithMode`) and the UI gate, so both agree.
///
/// - `creation_enabled` — the **deployment capability** flag (1b). Checked
///   first because it's the deployment's *intent*: a kiosk reports "disabled
///   here" regardless of this tab's durability.
/// - `can_persist` — whether **this tab's** primary tree is durable (1a):
///   `false` on the three ephemeral `BootStorageStatus` states (ephemeral
///   Direct, Worker→Direct downgrade, multi-tab secondary). Refusing here
///   closes the S-1 vault multi-writer race + the L-2 silent-loss footgun — a
///   created peer would write the shared localStorage vault but its tree would
///   evaporate on reload.
pub fn peer_create_refusal_reason(can_persist: bool, creation_enabled: bool) -> Option<&'static str> {
    if !creation_enabled {
        return Some("peer creation is disabled in this deployment");
    }
    if !can_persist {
        return Some(
            "this tab can't save — another tab owns your storage, or storage is \
             unavailable. Close the other tab and reload to create peers here.",
        );
    }
    None
}

/// Read the session config from the tree (defaults if absent/garbage).
pub fn read(peers: &Peers, peer_id: &str) -> SessionConfig {
    peers
        .get_entity(peer_id, &state_path(peer_id))
        .map(|e| SessionConfig::from_entity(&e))
        .unwrap_or_default()
}

/// Persist `cfg` for `peer_id`. Arm-aware (D15) via the blessed
/// [`Peers::seed_write`] router method — Direct → sync L0 (readable same
/// frame), Worker → async `dispatch_write`. The write fires any subscription
/// on the settings prefix (reactivity seam for the settings surface, step 4).
pub fn write(peers: &Peers, peer_id: &str, cfg: &SessionConfig) {
    peers.seed_write(peer_id, state_path(peer_id), cfg.to_entity());
}

/// Set the runtime `active` overlay flag explicitly (NOT a blind toggle) and
/// persist. Returns the value. The app uses this for the chrome ↔ site toggle:
/// it computes the *intended* value from what's actually visible, because a
/// `?site=` deep-link override (or a just-changed boot surface) can desync the
/// persisted flag from the visible surface — a blind toggle then no-ops or
/// inverts wrong (the "Exit Site won't exit after a `?site=` boot" bug).
pub fn set_active(peers: &Peers, peer_id: &str, value: bool) -> bool {
    let mut cfg = read(peers, peer_id);
    cfg.active = value;
    write(peers, peer_id, &cfg);
    value
}

// -- Settings-surface mutators (read-modify-write; step 4) -------------------
//
// Each preserves everything it doesn't touch. They live here (not in the
// Settings window) so the config semantics stay cohesive and unit-testable;
// the window is a thin controller that delegates.

/// Set which site is home (the default the overlay / a `Site` boot points at),
/// on a specific peer. Empty `target_peer` = the system peer (resolved at boot).
pub fn set_home_site(peers: &Peers, peer_id: &str, target_peer: &str, id: &str) {
    let mut cfg = read(peers, peer_id);
    cfg.home_site.peer_id = target_peer.to_string();
    cfg.home_site.id = id.to_string();
    write(peers, peer_id, &cfg);
}

/// Set the whole boot surface (`Chrome` / `Site` / `Window{peer,type}`),
/// preserving everything else — INCLUDING the runtime `active` overlay flag.
/// This is the **"Startup surface"** picker: it changes where the *next boot*
/// lands and must NOT yank the current session into the overlay (you're editing
/// in windowed chrome). Entering the overlay *now* is a separate, explicit
/// action — the status-bar toggle (`ToggleSiteMode` → [`set_active`]). (An
/// earlier cut applied it live; that made enabling "boot into site" abruptly
/// jump into the overlay mid-edit — wrong for a *startup* setting.) The
/// settings model computes the complete surface (defaults, scope-valid window
/// types) and hands it here, so this stays dumb about window-type knowledge.
pub fn set_boot_surface(peers: &Peers, peer_id: &str, surface: BootSurface) {
    let mut cfg = read(peers, peer_id);
    cfg.boot_surface = surface;
    write(peers, peer_id, &cfg);
}

/// Reactive self-heal: a config can't reference a peer that no longer exists.
/// When `deleted` is removed, drop any boot reference to it — a `Window` on
/// that peer falls back to `Chrome`, a `home_site` on that peer falls back to
/// the system peer (empty `peer_id`). Returns whether anything changed (writes
/// only on change). Called from the peer-delete path so the boot surface stays
/// predictable; the boot-time validation in `boot_load` is the backstop.
pub fn repair_for_deleted_peer(peers: &Peers, system_peer_id: &str, deleted: &str) -> bool {
    let mut cfg = read(peers, system_peer_id);
    let mut changed = false;
    if let BootSurface::Window { peer_id, .. } = &cfg.boot_surface {
        if peer_id == deleted {
            cfg.boot_surface = BootSurface::Chrome;
            changed = true;
        }
    }
    if cfg.home_site.peer_id == deleted {
        cfg.home_site.peer_id = String::new();
        changed = true;
    }
    if changed {
        write(peers, system_peer_id, &cfg);
    }
    changed
}

/// Toggle whether the chrome ↔ site status-bar toggle is shown.
pub fn toggle_show_toggle(peers: &Peers, peer_id: &str) {
    let mut cfg = read(peers, peer_id);
    cfg.site_mode.show_toggle = !cfg.site_mode.show_toggle;
    write(peers, peer_id, &cfg);
}

/// Toggle Phase-1 fast paint (cut 2c). Writes the durable config; the caller
/// (the wasm settings surface) also refreshes the pre-peer localStorage mirror
/// via [`crate::boot_fast_paint::write_enabled_mirror`] so the next reload's
/// pre-peer boot honors the change immediately. Returns the new value.
pub fn toggle_fast_paint(peers: &Peers, peer_id: &str) -> bool {
    let mut cfg = read(peers, peer_id);
    cfg.fast_paint = !cfg.fast_paint;
    let value = cfg.fast_paint;
    write(peers, peer_id, &cfg);
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_chrome_and_reproduces_legacy_site_mode() {
        let cfg = SessionConfig::default();
        assert_eq!(cfg.boot_surface, BootSurface::Chrome);
        assert!(cfg.site_mode.show_toggle, "legacy default: toggle shown");
        assert!(!cfg.active, "legacy default: boots in chrome");
        assert!(!cfg.site_mode.locked);
        assert!(cfg.peer_creation_enabled, "legacy default: creatable");
        assert_eq!(cfg.home_site.id, DEMO_SITE_ID);
    }

    #[test]
    fn default_round_trips_through_entity() {
        let cfg = SessionConfig::default();
        assert_eq!(SessionConfig::from_entity(&cfg.to_entity()), cfg);
    }

    #[test]
    fn non_default_round_trips_through_entity() {
        // Exercises the peer dimension on BOTH the boot surface and home_site.
        let cfg = SessionConfig {
            boot_surface: BootSurface::Window {
                peer_id: "peer-ten".into(),
                window_type: "Shell".into(),
            },
            home_site: SiteRef {
                peer_id: "labs-peer".into(),
                id: "church".into(),
                loc: "about".into(),
            },
            site_mode: SiteModePosture { enabled: true, show_toggle: false, locked: true },
            active: true,
            fast_paint: false,
            peer_creation_enabled: false,
        };
        assert_eq!(SessionConfig::from_entity(&cfg.to_entity()), cfg);
    }

    #[test]
    fn window_surface_empty_peer_round_trips() {
        // Empty peer = "system, resolved at boot" — must survive the round trip
        // as empty, not collapse to a non-window surface.
        let cfg = SessionConfig {
            boot_surface: BootSurface::Window {
                peer_id: String::new(),
                window_type: "Settings".into(),
            },
            ..SessionConfig::default()
        };
        assert_eq!(SessionConfig::from_entity(&cfg.to_entity()), cfg);
    }

    #[test]
    fn boot_surface_from_builds_each_kind() {
        assert_eq!(boot_surface_from("chrome", "", ""), BootSurface::Chrome);
        assert_eq!(boot_surface_from("site", "", ""), BootSurface::Site);
        assert_eq!(
            boot_surface_from("window", "peer-x", "Shell"),
            BootSurface::Window { peer_id: "peer-x".into(), window_type: "Shell".into() }
        );
        // An empty peer on a window = "system, resolved at boot" (round-trips as empty).
        assert_eq!(
            boot_surface_from("window", "", SITE_BROWSER_WINDOW),
            BootSurface::Window { peer_id: String::new(), window_type: SITE_BROWSER_WINDOW.into() }
        );
        // Unknown / absent kind → Chrome (garbage-tolerant).
        assert_eq!(boot_surface_from("nonsense", "", ""), BootSurface::Chrome);
    }

    /// BUG-1 invariant: the chrome↔site toggle (status bar AND the overlay's
    /// "Exit Site" control) is exposed iff `exposes_toggle()` — and a locked
    /// deployment must NOT expose it, so the user can't toggle out into chrome
    /// and get stranded with no way back. Now expressed directly on the granular
    /// `SiteModePosture` (no preset in the middle).
    #[test]
    fn locked_posture_exposes_no_exit_toggle() {
        let locked = SiteModePosture { enabled: true, show_toggle: false, locked: true };
        assert!(locked.locked, "a locked kiosk posture");
        assert!(
            !locked.exposes_toggle(),
            "a locked deployment must expose no exit toggle (BUG-1 strand)"
        );

        // The escapable posture DOES expose the toggle (you can leave the site).
        let escapable = SiteModePosture { enabled: true, show_toggle: true, locked: false };
        assert!(escapable.exposes_toggle());

        // `enabled=false` also closes the toggle even if `show_toggle` is set —
        // no site available ⇒ no inert toggle into an empty surface.
        let disabled = SiteModePosture { enabled: false, show_toggle: true, locked: false };
        assert!(!disabled.exposes_toggle());
    }

    #[test]
    fn site_browser_window_is_registered() {
        // The publish default (`--surface=window --window-type="Site Browser"`)
        // and the settings surface boot a maximized window BY NAME; that name
        // must be a real registered window type, or boot_load silently falls
        // back to chrome (app.rs "names an unknown window type; staying in
        // chrome"). The const pins it so a registry rename can't break it.
        let registered: Vec<&str> = crate::window_registry::standard_window_type_meta()
            .into_iter()
            .map(|(name, _scope)| name)
            .collect();
        assert!(
            registered.contains(&SITE_BROWSER_WINDOW),
            "{SITE_BROWSER_WINDOW:?} must be a registered window type: {registered:?}"
        );
    }

    #[test]
    fn active_derives_from_boot_surface() {
        let mut cfg = SessionConfig::default();
        cfg.boot_surface = BootSurface::Chrome;
        assert!(!cfg.active_from_boot_surface());
        cfg.boot_surface = BootSurface::Site;
        assert!(cfg.active_from_boot_surface());
        cfg.boot_surface = BootSurface::Window { peer_id: "p".into(), window_type: "Shell".into() };
        assert!(!cfg.active_from_boot_surface(), "Window is a non-overlay surface");
    }

    #[test]
    fn boot_surface_kinds_round_trip_through_entity() {
        for surface in [
            BootSurface::Chrome,
            BootSurface::Site,
            BootSurface::Window { peer_id: "p10".into(), window_type: "Shell".into() },
        ] {
            let cfg = SessionConfig { boot_surface: surface.clone(), ..SessionConfig::default() };
            assert_eq!(SessionConfig::from_entity(&cfg.to_entity()).boot_surface, surface);
        }
    }

    #[test]
    fn from_entity_tolerates_garbage() {
        let e = Entity::new("x/y", vec![0xff, 0x00, 0x42]).unwrap();
        assert_eq!(SessionConfig::from_entity(&e), SessionConfig::default());
    }

    #[test]
    fn read_returns_default_when_absent_and_persisted_after_write() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        assert_eq!(read(&peers, &pid), SessionConfig::default());
        let mut mutated = SessionConfig::default();
        mutated.active = true;
        mutated.home_site.id = "church".into();
        write(&peers, &pid, &mutated);
        assert_eq!(read(&peers, &pid), mutated);
    }

    #[test]
    fn set_active_sets_explicit_value_not_a_flip() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        // Idempotent: setting the same value twice keeps it (a flip would not).
        assert!(set_active(&peers, &pid, true));
        assert!(set_active(&peers, &pid, true), "set_active is not a toggle");
        assert!(read(&peers, &pid).active);
        assert!(!set_active(&peers, &pid, false));
        assert!(!read(&peers, &pid).active);
    }

    #[test]
    fn set_boot_surface_is_startup_only_and_preserves_runtime_active() {
        // The "Startup surface" picker changes where the NEXT boot lands but
        // must NOT change the live `active` overlay flag — enabling "boot into
        // site" while editing in chrome must not abruptly jump into the overlay.
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        // Start in the overlay (active = true), as if toggled on now.
        set_active(&peers, &pid, true);
        set_boot_surface(&peers, &pid, BootSurface::Chrome);
        assert_eq!(read(&peers, &pid).boot_surface, BootSurface::Chrome, "boot surface persists");
        assert!(read(&peers, &pid).active, "runtime surface untouched by a startup-surface change");
        // And the reverse: configuring Site startup while in chrome stays chrome.
        set_active(&peers, &pid, false);
        set_boot_surface(&peers, &pid, BootSurface::Site);
        assert_eq!(read(&peers, &pid).boot_surface, BootSurface::Site);
        assert!(!read(&peers, &pid).active, "choosing Site as the STARTUP surface must not light the overlay now");
    }

    #[test]
    fn set_home_site_persists_peer_and_id() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        set_home_site(&peers, &pid, "labs-peer", "labs");
        let cfg = read(&peers, &pid);
        assert_eq!(cfg.home_site.id, "labs");
        assert_eq!(cfg.home_site.peer_id, "labs-peer");
    }

    #[test]
    fn set_boot_surface_persists_window_peer_and_type() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        assert_eq!(read(&peers, &pid).boot_surface, BootSurface::Chrome);
        let surface = BootSurface::Window { peer_id: "peer-10".into(), window_type: "Shell".into() };
        set_boot_surface(&peers, &pid, surface.clone());
        assert_eq!(read(&peers, &pid).boot_surface, surface);
        // ...and it preserves the rest (profile/home_site untouched).
        assert_eq!(read(&peers, &pid).home_site.id, DEMO_SITE_ID);
    }

    #[test]
    fn repair_for_deleted_peer_resets_window_and_home_site() {
        let peers = Peers::new_direct();
        let sys = peers.system_peer_id().to_string();
        // Window boots on a peer that's about to be deleted; home_site on another.
        set_boot_surface(&peers, &sys, BootSurface::Window {
            peer_id: "gone".into(),
            window_type: "Shell".into(),
        });
        set_home_site(&peers, &sys, "gone", "labs");
        assert!(repair_for_deleted_peer(&peers, &sys, "gone"), "should report a change");
        let cfg = read(&peers, &sys);
        assert_eq!(cfg.boot_surface, BootSurface::Chrome, "Window on a gone peer → Chrome");
        assert_eq!(cfg.home_site.peer_id, "", "home_site on a gone peer → system (empty)");
        assert_eq!(cfg.home_site.id, "labs", "the site id itself is preserved");
        // A second repair for an unrelated peer is a no-op.
        assert!(!repair_for_deleted_peer(&peers, &sys, "someone-else"));
    }

    #[test]
    fn boot_default_reflects_the_built_startup_surface() {
        // `build.rs` always emits `ENTITY_STARTUP_SURFACE` (defaulting to
        // "chrome"), so `boot_default()`'s surface must reflect whatever THIS
        // binary was built with: Chrome for a default dev/CI build, the baked
        // surface for a surfaced build (`ENTITY_STARTUP_SURFACE=site cargo test`
        // flips this and stays green — proving the build.rs → env! → boot_default
        // wiring in both directions). Guards against the fallback drifting.
        let kind = option_env!("ENTITY_STARTUP_SURFACE").unwrap_or("chrome");
        let window_type = option_env!("ENTITY_STARTUP_WINDOW_TYPE").unwrap_or("");
        assert_eq!(boot_default().boot_surface, boot_surface_from(kind, "", window_type));
    }

    #[test]
    fn default_dev_build_is_chrome() {
        // The regression guard: a normal (unset / `chrome`) build must keep the
        // legacy chrome-first posture. Skips on a non-`chrome` surfaced build,
        // which is intentionally not the default.
        if option_env!("ENTITY_STARTUP_SURFACE").is_some_and(|s| s != "chrome") {
            return;
        }
        assert_eq!(boot_default().boot_surface, BootSurface::Chrome);
    }

    #[test]
    fn boot_default_derives_active_from_surface() {
        // Whatever surface was baked, `active` is consistent with it (boot_load
        // re-derives it, but the type stays honest): a `site` build cold-boots
        // into the (escapable) overlay showing, a chrome/window build does not.
        let cfg = boot_default();
        assert_eq!(cfg.active, cfg.active_from_boot_surface());
    }

    #[test]
    fn home_site_from_falls_back_to_demo_when_unset() {
        // The default-build path: all env vars empty/absent → bundled local
        // demo, byte-identical to the pre-cut-2a hardcoded default.
        let home = home_site_from(None, None, None);
        assert_eq!(home.peer_id, "");
        assert_eq!(home.id, DEMO_SITE_ID);
        assert_eq!(home.loc, "");
        // Empty strings (build.rs always emits, empty when unset) are treated
        // as absent too.
        assert_eq!(home_site_from(Some(""), Some("  "), Some("")), home);
    }

    #[test]
    fn home_site_from_threads_a_remote_home() {
        let home = home_site_from(Some("labs-peer"), Some("labs"), Some("intro"));
        assert_eq!(home.peer_id, "labs-peer");
        assert_eq!(home.id, "labs");
        assert_eq!(home.loc, "intro");
        // A peer with no explicit site id still falls back to demo (site id is
        // never empty — the overlay always needs a site to point at).
        assert_eq!(home_site_from(Some("p"), None, None).id, DEMO_SITE_ID);
    }

    #[test]
    fn toggle_show_toggle_flips() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        assert!(read(&peers, &pid).site_mode.show_toggle, "default on");
        toggle_show_toggle(&peers, &pid);
        assert!(!read(&peers, &pid).site_mode.show_toggle);
    }

    // --- 1b: peer-creation capability posture (MAP §10) ----------------------

    #[test]
    fn default_is_creatable_and_capability_is_independent() {
        // The type-level default is the creatable posture; the capability is now
        // a plain independent field (a locked kiosk sets it false explicitly via
        // the deployment config — no preset bundles it, L-3).
        assert!(SessionConfig::default().peer_creation_enabled, "default = creatable");
        let kiosk = SessionConfig {
            peer_creation_enabled: false,
            site_mode: SiteModePosture { enabled: true, show_toggle: false, locked: true },
            ..SessionConfig::default()
        };
        assert!(!kiosk.peer_creation_enabled, "an explicit kiosk disables creation");
    }

    #[test]
    fn peer_creation_enabled_round_trips() {
        let mut cfg = SessionConfig::default();
        cfg.peer_creation_enabled = false;
        assert!(!SessionConfig::from_entity(&cfg.to_entity()).peer_creation_enabled);
        cfg.peer_creation_enabled = true;
        assert!(SessionConfig::from_entity(&cfg.to_entity()).peer_creation_enabled);
    }

    #[test]
    fn pre_1b_config_without_the_field_defaults_to_creatable() {
        // A persisted config written before 1b carries no `peer_creation_enabled`
        // key. Decoding must keep creation ENABLED (don't silently lock an
        // existing deployment). Forge an entity omitting the new field — and
        // carrying a legacy `profile` key to prove it's ignored, not choked on.
        let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
            "profile" => entity_ecf::text("full"),
            "boot_surface_kind" => entity_ecf::text("chrome")
        });
        let e = Entity::new(STATE_TYPE, data).unwrap();
        assert!(
            SessionConfig::from_entity(&e).peer_creation_enabled,
            "missing key → creatable (backward compatible)"
        );
    }

    #[test]
    fn refusal_reason_reports_capability_first_then_durability() {
        // Allowed only when BOTH durable AND enabled.
        assert_eq!(peer_create_refusal_reason(true, true), None);
        // Capability off → "disabled in this deployment", regardless of durability.
        assert_eq!(
            peer_create_refusal_reason(true, false),
            Some("peer creation is disabled in this deployment")
        );
        assert_eq!(
            peer_create_refusal_reason(false, false),
            Some("peer creation is disabled in this deployment"),
            "capability is reported first (deployment intent)"
        );
        // Enabled but not durable → the durability message.
        assert!(peer_create_refusal_reason(false, true)
            .unwrap()
            .contains("can't save"));
    }

    #[test]
    fn toggle_fast_paint_flips_and_persists() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        assert!(read(&peers, &pid).fast_paint, "default on");
        assert!(!toggle_fast_paint(&peers, &pid), "returns the new value (off)");
        assert!(!read(&peers, &pid).fast_paint, "persisted off");
        assert!(toggle_fast_paint(&peers, &pid), "back on");
    }
}
