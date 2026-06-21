//! Session Configuration — the spine that ties boot, surface, and content
//! together (boot-config-surfaces reframe §4-A).
//!
//! Before this module, three distinct concepts were fused into one
//! hand-wired POC: *which configuration the app boots into*, *which surface
//! is showing*, and *which content fills it*. [`SiteModeState`] mashed mode +
//! posture; "which site" was a hard-coded `DEMO_SITE_ID` constant. This
//! module is the single tree-backed config entity that splits them cleanly:
//!
//!   * **`profile`** — a deployment preset (`full` / `tutorial` / `strict-site`)
//!     that seeds the rest. Presets are config, **not** an architecture fork
//!     (`project_deployment_profiles_mode_model`) — the system peer always
//!     exists; a profile only changes defaults.
//!   * **`boot_surface`** — which surface boot lands in: the entity-browser
//!     `Chrome`, a content `Site`, or a maximized `Window` (the §4-B Surfaces
//!     seam, activated in step 5).
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
//! Config (`profile` / `boot_surface` / `home_site` / `site_mode`) is durable
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

/// Deployment profile preset. A profile seeds the rest of the config; it is
/// **not** an architecture fork. Maps onto the vision's three postures
/// (strict-site / tutorial / full).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Today's default: chrome-first, toggle available, fully explorable.
    Full,
    /// Mid posture: boot into the site, toggle still available.
    Tutorial,
    /// Locked content-site deployment: boot into the site, no toggle, locked.
    StrictSite,
}

impl Profile {
    pub fn as_str(&self) -> &'static str {
        match self {
            Profile::Full => "full",
            Profile::Tutorial => "tutorial",
            Profile::StrictSite => "strict-site",
        }
    }

    /// Parse a profile name (`full` / `tutorial` / `strict-site`). Used by the
    /// entity round-trip, the build-time `ENTITY_PROFILE`, and the per-domain
    /// deployment config ([`crate::deployment_config`]). Unknown → `None`.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "full" => Some(Profile::Full),
            "tutorial" => Some(Profile::Tutorial),
            "strict-site" => Some(Profile::StrictSite),
            _ => None,
        }
    }

    /// The deployment profile baked at build time via `ENTITY_PROFILE`
    /// (reframe §5; see `build.rs`). This is the **cold-boot default
    /// posture** — consumed only when no durable session config exists
    /// ([`boot_default`]); a persisted config always wins on a warm boot.
    /// Defaults to `Full` when unset or (defensively) unrecognized —
    /// `build.rs` validates the value, so an unknown one shouldn't reach here.
    pub fn build_default() -> Self {
        option_env!("ENTITY_PROFILE")
            .and_then(Self::from_str)
            .unwrap_or(Profile::Full)
    }

    /// The full config this profile seeds. Cold boot writes the active
    /// profile's preset; warm boot reads the persisted value and never
    /// clobbers it. The preset logic is wired now (held seam, exercised by
    /// unit tests); selecting a non-`Full` profile at boot is step 4 (the
    /// system-settings surface) / step 5 (build-time defaults).
    pub fn preset(self) -> SessionConfig {
        // The default home site is the build-time `ENTITY_HOME_*` (cut 2a) or,
        // unset, the bundled local demo. An empty `peer_id` = "the system peer,
        // resolved at boot" (presets can't bake a runtime peer-id, handoff §3).
        // Note: `set_profile` PRESERVES the live `home_site` and only takes a
        // preset's posture, so this default only shapes `default()` /
        // `boot_default()` — the absent-config cold-boot path.
        let home_site = home_site_default();
        match self {
            Profile::Full => SessionConfig {
                profile: self,
                boot_surface: BootSurface::Chrome,
                home_site,
                site_mode: SiteModePosture { enabled: true, show_toggle: true, locked: false },
                active: false,
                fast_paint: true,
                peer_creation_enabled: true,
            },
            Profile::Tutorial => SessionConfig {
                profile: self,
                boot_surface: BootSurface::Site,
                home_site,
                site_mode: SiteModePosture { enabled: true, show_toggle: true, locked: false },
                active: true,
                fast_paint: true,
                peer_creation_enabled: true,
            },
            // Kiosk: locked site AND no peer creation (the capability lock that
            // L-3 found missing — a strict-site deployment must not let the user
            // mint peers even if they reach a create surface).
            Profile::StrictSite => SessionConfig {
                profile: self,
                boot_surface: BootSurface::Site,
                home_site,
                site_mode: SiteModePosture { enabled: true, show_toggle: false, locked: true },
                active: true,
                fast_paint: true,
                peer_creation_enabled: false,
            },
        }
    }
}

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
    pub profile: Profile,
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
    /// explorable browser); a `strict-site`/kiosk preset seeds it **false** so
    /// the create affordance is hidden and `CreatePeerWithMode` is refused
    /// (closes L-3 — peer creation was reachable in *every* posture). Rides the
    /// same build/fetched/persisted precedence as the surface posture. Absent in
    /// a pre-1b persisted config → defaults true (from `default()`), so existing
    /// full deployments are unaffected.
    pub peer_creation_enabled: bool,
}

impl Default for SessionConfig {
    /// The `Full` profile preset — chrome-first, toggle available, demo site
    /// as home. Reproduces the legacy `SiteModeState::default()` behavior so
    /// the boot path is unchanged for the default deployment.
    fn default() -> Self {
        Profile::Full.preset()
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
                Some("profile") => {
                    if let Some(p) = v.as_text().and_then(Profile::from_str) {
                        cfg.profile = p;
                    }
                }
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
            "profile" => entity_ecf::text(self.profile.as_str()),
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

/// The cold-boot default session config — the build-time profile's preset
/// (reframe §5, `ENTITY_PROFILE`). [`EntityApp::boot_load`] uses this when no
/// durable config exists, so a fresh / wiped deployment lands in its baked
/// posture (a `strict-site` build cold-boots into the site). A persisted
/// config always wins, so this only shapes the *absent* case. Distinct from
/// [`SessionConfig::default`] (always `Full`), which stays the type-level
/// default for `from_entity` fallback + tests.
pub fn boot_default() -> SessionConfig {
    Profile::build_default().preset()
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

/// Apply a profile preset's **posture** (`boot_surface` + `site_mode`) while
/// PRESERVING the chosen `home_site` and the current runtime `active` surface.
/// A profile is a quick preset over the granular fields, not a reset — picking
/// "Strict Site" sets the boot surface + hides the toggle but does not yank you
/// out of wherever you're browsing now (that takes effect next boot).
pub fn set_profile(peers: &Peers, peer_id: &str, profile: Profile) {
    let preset = profile.preset();
    let mut cfg = read(peers, peer_id);
    cfg.profile = profile;
    cfg.boot_surface = preset.boot_surface;
    cfg.site_mode = preset.site_mode;
    // The capability posture is part of the preset too (kiosk = no creation).
    cfg.peer_creation_enabled = preset.peer_creation_enabled;
    write(peers, peer_id, &cfg);
}

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
    fn default_is_full_profile_and_reproduces_legacy_site_mode() {
        let cfg = SessionConfig::default();
        assert_eq!(cfg.profile, Profile::Full);
        assert_eq!(cfg.boot_surface, BootSurface::Chrome);
        assert!(cfg.site_mode.show_toggle, "legacy default: toggle shown");
        assert!(!cfg.active, "legacy default: boots in chrome");
        assert!(!cfg.site_mode.locked);
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
            profile: Profile::StrictSite,
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
    fn profile_presets_match_postures() {
        assert_eq!(Profile::Full.preset().boot_surface, BootSurface::Chrome);
        assert!(Profile::Full.preset().site_mode.show_toggle);

        let tut = Profile::Tutorial.preset();
        assert_eq!(tut.boot_surface, BootSurface::Site);
        assert!(tut.site_mode.show_toggle);
        assert!(!tut.site_mode.locked);

        let strict = Profile::StrictSite.preset();
        assert_eq!(strict.boot_surface, BootSurface::Site);
        assert!(!strict.site_mode.show_toggle, "strict-site hides the toggle");
        assert!(strict.site_mode.locked);
    }

    /// BUG-1 invariant: the chrome↔site toggle (status bar AND the overlay's
    /// "Exit Site" control) is exposed iff `exposes_toggle()` — and a
    /// locked/strict-site deployment must NOT expose it, so the user can't
    /// toggle out into chrome and get stranded with no way back.
    #[test]
    fn locked_deployment_exposes_no_exit_toggle() {
        let strict = Profile::StrictSite.preset();
        assert!(strict.site_mode.locked, "strict-site is locked");
        assert!(
            !strict.site_mode.exposes_toggle(),
            "a locked deployment must expose no exit toggle (BUG-1 strand)"
        );

        // The explorable profiles DO expose the toggle (you can leave the site).
        assert!(Profile::Full.preset().site_mode.exposes_toggle());
        assert!(Profile::Tutorial.preset().site_mode.exposes_toggle());

        // `enabled=false` also closes the toggle even if `show_toggle` is set —
        // no site available ⇒ no inert toggle into an empty surface.
        let disabled = SiteModePosture { enabled: false, show_toggle: true, locked: false };
        assert!(!disabled.exposes_toggle());
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
    fn set_profile_applies_posture_but_preserves_home_site_and_active() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        // Custom home site + a live active surface.
        set_home_site(&peers, &pid, "", "church");
        let _ = set_active(&peers, &pid, true); // active = true
        set_profile(&peers, &pid, Profile::StrictSite);
        let cfg = read(&peers, &pid);
        assert_eq!(cfg.profile, Profile::StrictSite);
        assert_eq!(cfg.boot_surface, BootSurface::Site, "preset posture applied");
        assert!(!cfg.site_mode.show_toggle, "strict-site hides the toggle");
        assert!(cfg.site_mode.locked);
        assert_eq!(cfg.home_site.id, "church", "home site preserved across profile change");
        assert!(cfg.active, "runtime surface untouched by a profile change");
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
    fn build_default_matches_the_built_profile() {
        // `build.rs` always emits `ENTITY_PROFILE` (defaulting to "full"), so
        // `build_default()` must reflect whatever THIS binary was built with:
        // Full for a default dev/CI build, the baked profile for a profiled
        // build (`ENTITY_PROFILE=strict-site cargo test` flips this and stays
        // green — proving the build.rs → env! → build_default wiring in both
        // directions). Guards against the fallback logic silently drifting.
        let built = option_env!("ENTITY_PROFILE").and_then(Profile::from_str);
        assert_eq!(Profile::build_default(), built.unwrap_or(Profile::Full));
    }

    #[test]
    fn default_dev_build_is_full() {
        // The regression guard: a normal (unset / `full`) build must keep the
        // legacy Full posture. Skips on a non-`full` profiled build, which is
        // intentionally not the default.
        if option_env!("ENTITY_PROFILE").is_some_and(|p| p != "full") {
            return;
        }
        assert_eq!(Profile::build_default(), Profile::Full);
    }

    #[test]
    fn boot_default_is_the_build_profile_preset() {
        assert_eq!(boot_default(), Profile::build_default().preset());
    }

    #[test]
    fn strict_site_preset_cold_boots_into_locked_site() {
        // The ergonomic outcome a `strict-site` build must produce on cold
        // boot: site surface, no chrome toggle, locked. Derive `active` the
        // way boot_load does to confirm the overlay shows.
        let mut cfg = Profile::StrictSite.preset();
        cfg.active = cfg.active_from_boot_surface();
        assert!(cfg.active, "strict-site cold boot must show the site overlay");
        assert!(!cfg.site_mode.show_toggle, "no chrome toggle in strict-site");
        assert!(cfg.site_mode.locked, "strict-site is locked (behavior deferred)");
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
    fn full_and_tutorial_allow_creation_strict_site_disables_it() {
        assert!(Profile::Full.preset().peer_creation_enabled, "full = creatable");
        assert!(Profile::Tutorial.preset().peer_creation_enabled, "tutorial = creatable");
        assert!(
            !Profile::StrictSite.preset().peer_creation_enabled,
            "kiosk/strict-site disables peer creation (L-3)"
        );
        // The type-level default is the full, creatable posture.
        assert!(SessionConfig::default().peer_creation_enabled);
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
        // existing full deployment). Forge an entity with the old field set,
        // omitting the new one.
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
    fn set_profile_applies_capability_posture() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        // Default is creatable; switching to strict-site must disable creation...
        assert!(read(&peers, &pid).peer_creation_enabled);
        set_profile(&peers, &pid, Profile::StrictSite);
        assert!(!read(&peers, &pid).peer_creation_enabled, "strict-site preset disables creation");
        // ...and switching back to full re-enables it.
        set_profile(&peers, &pid, Profile::Full);
        assert!(read(&peers, &pid).peer_creation_enabled);
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
