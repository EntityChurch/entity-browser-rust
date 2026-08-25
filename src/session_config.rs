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
// identity key: a WindowType name, displayed via `window.<slug>`
pub const SITE_BROWSER_WINDOW: &str = "Site Browser"; // i18n-ignore

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

/// v11 WebRTC provisioning — the app-side **native shadow** of the worker
/// protocol's `WireWebRtcConfig`, which is `#![cfg(target_arch = "wasm32")]`
/// and so cannot be named in this natively-compiled module. `app.rs` converts
/// this to the wire type at the `InitParams` boundary (the same "decoupled
/// serializable shadow, convert at the boundary" split the wire type itself
/// documents). This is the *capability* — which signaling node the §6.5
/// establisher rendezvouses through — and is deployment-wide. **Which peers
/// install an establisher is still narrower than "everyone":** only the primary
/// does ([`webrtc_install_primary`]), and an additional peer must ask
/// explicitly. That is the part of the v6 Subscribe lesson that survives — a
/// worker-wide "config present" must not silently enable *every* peer. Whether
/// the primary installs, on the other hand, follows directly from this being
/// present; see [`webrtc_install_primary`] for why that axis collapsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebRtcProvisioning {
    /// The signaling node's peer-id. Required alongside `node_addr` — the
    /// carrier authenticates the node, so an address without its id is an
    /// unauthenticated rendezvous (rust's provisioning-payload ruling).
    pub node_peer_id: String,
    /// The node's browser-reachable address (`ws://` / `wss://`). A browser
    /// cannot open a raw TCP socket, so a TCP-only node is unreachable here.
    pub node_addr: String,
    /// ICE servers for the browser's own ICE agent. **Empty is legal and means
    /// host-candidates-only** — a LAN-only deployment (rung-1) — never "use a
    /// public default", which would enrol a third party invisibly. Carried so
    /// the harness / a real-NAT (rung-2) deployment can populate it; the
    /// build-knob default leaves it empty.
    pub ice_servers: Vec<IceServer>,
    /// §6.5 negotiation tunables. `None` = the worker impl's defaults.
    pub poll_interval_ms: Option<u64>,
    pub max_deadline_ms: Option<u64>,
}

/// One ICE server for the browser's ICE agent (native shadow of the protocol's
/// `WireIceServer`). A `stun:` entry carries no credentials; a `turn:`/`turns:`
/// entry carries both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IceServer {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

/// Parse a user-supplied reflector list into [`IceServer`] entries.
///
/// Accepts comma- and/or whitespace-separated URLs and returns them as **one**
/// `IceServer` carrying every URL — which is what an `RTCIceServer` with no
/// credentials is, and matches `EXTENSION-SIGNALING` §9.3 forbidding reflector
/// authentication: no credential, nothing to expire, nothing to rotate.
///
/// **`turn:`/`turns:` is refused, deliberately and sayably.** A TURN server
/// needs a username and credential, and there is nowhere to put them here yet.
/// Accepting one would build an `RTCIceServer` that silently gathers no relay
/// candidates — a reflector that looks configured and does nothing, which is the
/// failure mode this whole area keeps producing. Refusing it says so where the
/// user typed it. Carrying credentials is a later, additive shape.
///
/// An empty/blank input is `Ok(vec![])`, not an error: **host-candidates-only is
/// a legal deployment** (a LAN, our own green gates), never "use a public
/// default", which would enrol a third party invisibly.
///
/// The refusal strings are `i18n-ignore`d, matching the rest of this validation
/// family (`connectors::validate_node_peer_id`, `connectors::add_connector`),
/// which is English throughout. Extract the family together or not at all — a
/// window where two of five refusals are translated reads as a bug.
pub fn parse_ice_urls(raw: &str) -> Result<Vec<IceServer>, String> {
    let urls: Vec<String> = raw
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if urls.is_empty() {
        return Ok(Vec::new());
    }
    for u in &urls {
        let scheme = u.split_once(':').map(|(s, _)| s.to_ascii_lowercase());
        match scheme.as_deref() {
            // RFC 7064: `stun:host[:port]` — non-hierarchical, so there is no
            // `//` and a `stun://…` is a typo worth catching here.
            Some("stun") | Some("stuns") => {
                if u.contains("//") {
                    return Err(format!(
                        "'{u}' is not a STUN URI — RFC 7064 is stun:host[:port], with no '//'" // i18n-ignore
                    ));
                }
                if u.split_once(':').map(|(_, rest)| rest.trim().is_empty()).unwrap_or(true) {
                    return Err(format!("'{u}' names no host")); // i18n-ignore
                }
            }
            Some("turn") | Some("turns") => {
                return Err(format!(
                    "'{u}' is a TURN server, which needs a username and credential — \
                     this field carries reflectors (stun:) only" // i18n-ignore
                ))
            }
            _ => {
                return Err(format!(
                    "'{u}' is not a reflector URI — expected stun:host[:port]" // i18n-ignore
                ))
            }
        }
    }
    Ok(vec![IceServer { urls, username: None, credential: None }])
}

/// Resolve WebRTC provisioning from a `(node_peer_id, node_addr)` pair, plus the
/// node's optional reflectors. **Both halves of the node are required** — a lone
/// address is an unauthenticated rendezvous and a lone peer-id has nowhere to
/// dial — so any missing/blank half yields `None` (fails closed, D3).
///
/// **`ice` is optional and degrades LOUDLY, never silently.** A malformed list
/// is warned about and dropped rather than failing the whole node: rendezvous
/// (meet, discovery) rides the node and works fine on host candidates, so a typo
/// in a reflector must not cost the user their signaling. The place a bad value
/// is *refused* is where it is typed — `connectors::add_connector` — so this
/// path only ever sees a legacy row, a URL param, or a build knob.
///
/// Pure and native-testable; the three provisioning sources (build knob, URL
/// query, durable connector registry) all funnel through here, which is what
/// keeps them from disagreeing about what a provisioned session speaks.
pub fn resolve_webrtc_provisioning(
    node_peer_id: Option<&str>,
    node_addr: Option<&str>,
    ice: Option<&str>,
) -> Option<WebRtcProvisioning> {
    let clean = |o: Option<&str>| o.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let ice_servers = match ice.map(parse_ice_urls).transpose() {
        Ok(v) => v.unwrap_or_default(),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "webrtc provisioning: ignoring a malformed reflector list — this session \
                 will gather HOST CANDIDATES ONLY and will not traverse a NAT"
            );
            Vec::new()
        }
    };
    match (clean(node_peer_id), clean(node_addr)) {
        (Some(node_peer_id), Some(node_addr)) => Some(WebRtcProvisioning {
            node_peer_id,
            node_addr,
            ice_servers,
            poll_interval_ms: None,
            max_deadline_ms: None,
        }),
        _ => None,
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

    /// Whether the chrome-side status-bar site toggle should be exposed. It is
    /// the [`SiteModePosture::exposes_toggle`] predicate, further **suppressed
    /// when boot landed in a `Window` surface**: a maximized Site Browser window
    /// is a window in the WM, not an overlay host, so the "View Site" toggle has
    /// no coherent target there — and exposing it drops a fresh peer into the
    /// overlay pointed at `home_site` (the stray-toggle → missing-home footgun,
    /// fbdc0822). This is defense-in-depth at the render seam: it holds even if a
    /// deployment mis-emits `show_toggle=true` alongside `surface=window`, so a
    /// bad config can't strand a Window deployment with a broken toggle. (The
    /// operator `?chrome=1` escape forces the Chrome surface for the per-frame
    /// read, so it is unaffected.)
    pub fn status_toggle_visible(&self) -> bool {
        self.site_mode.exposes_toggle() && !matches!(self.boot_surface, BootSurface::Window { .. })
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

/// The build-time WebRTC provisioning *capability* — the §6.5 signaling node,
/// from `ENTITY_WEBRTC_NODE_PEER` + `ENTITY_WEBRTC_NODE_ADDR` (both required,
/// [`resolve_webrtc_provisioning`]). `None` on a default build → no establisher
/// is provisioned on any worker (exact v10 behaviour). This is the **build-knob
/// layer** — available *before* the boot worker spawns (`InitParams.webrtc` is
/// Init-only upstream, so a post-boot deployment-config fetch cannot reach it),
/// and per this module's convention (see [`boot_default`]) the build knob is
/// the **testing / dev** path. The production per-domain mechanism is
/// `/entity-deployment.json`; carrying the node there is a later, additive
/// layer (it needs a pre-spawn fetch) and is deliberately not wired yet.
pub fn webrtc_provisioning_default() -> Option<WebRtcProvisioning> {
    resolve_webrtc_provisioning(
        option_env!("ENTITY_WEBRTC_NODE_PEER"),
        option_env!("ENTITY_WEBRTC_NODE_ADDR"),
        option_env!("ENTITY_WEBRTC_ICE"),
    )
}

/// Whether the **primary** peer installs the §6.5 establisher.
///
/// **A provisioned signaling node *is* the decision.** `provisioned` is whether
/// [`crate::connectors::resolve_provisioning`] yielded a node at all —
/// URL param, the user's durable connector selection, or the build knob.
///
/// This used to be a second, independent axis (`ENTITY_WEBRTC_ENABLE_PRIMARY`
/// / `?webrtc_enable=1`) that had to be set *as well*, on the v6 reasoning that
/// "a worker-wide config present must not silently enable every peer". That
/// reasoning was about a **deployment-wide build knob**, and it stopped
/// describing reality when the connector registry landed: choosing a connector
/// in the UI is a user *act*, and there was never a surface to perform the
/// second half. The result was that every build a user actually runs resolved a
/// node and then installed nothing — the registry and the naming modes both
/// reached a connect that could not happen.
///
/// So the axes collapse. What survives from v6 is the part that was really
/// load-bearing: **fail closed** — no node, no establisher, in every branch,
/// because the worker-host *rejects* an Init that enables with no config.
///
/// `url_override` is [`webrtc_enable_from_query`]: `Some(false)` is an explicit
/// kill switch (a debugging affordance, and the one way to boot a provisioned
/// deployment with the seam off); `Some(true)` cannot conjure a node it does not
/// have; `None` — the normal case — follows the provisioning.
pub fn webrtc_install_primary(provisioned: bool, url_override: Option<bool>) -> bool {
    provisioned && url_override.unwrap_or(true)
}

/// Extract a URL query-param value from a raw `location.search` string
/// (`"?a=1&b=2"` or `"a=1&b=2"`). Returns the first match's raw value. No
/// percent-decoding — callers use it for `ws://host:port` values, whose chars
/// (`:` `/`) are query-legal unencoded; a value containing `&`/`=`/`#` is not
/// supported here (none of ours do). Mirrors the `main.rs` `?worker=` idiom.
fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.trim_start_matches('?').split('&').find_map(|pair| {
        let mut parts = pair.splitn(2, '=');
        (parts.next() == Some(key)).then(|| parts.next().unwrap_or(""))
    })
}

/// Runtime WebRTC provisioning from a URL query — the dev/showcase / e2e
/// injection channel, **higher precedence than the build knob** and never
/// persisted (`deployment_config.rs` precedence). Reads `webrtc_node_peer` +
/// `webrtc_node`; both required (via [`resolve_webrtc_provisioning`]). Its
/// reason for existing: a per-test signaling node has a **dynamic** address the
/// compile-time knob cannot carry.
pub fn webrtc_provisioning_from_query(query: &str) -> Option<WebRtcProvisioning> {
    resolve_webrtc_provisioning(
        query_param(query, "webrtc_node_peer"),
        query_param(query, "webrtc_node"),
        // `?webrtc_ice=stun:host:3478` — the rung-2 harness channel, same reason
        // the node halves are here: a per-test reflector has an address the
        // compile-time knob cannot carry.
        query_param(query, "webrtc_ice"),
    )
}

/// The primary-install *override* from a URL query (`?webrtc_enable=1`|`true`,
/// or bare `?webrtc_enable`; `=0`/`=no`/`=false` to refuse). `None` = not named,
/// which is the normal case and means "follow the provisioning"
/// ([`webrtc_install_primary`]).
///
/// It is no longer a required second half — see [`webrtc_install_primary`] for
/// why that axis collapsed. It remains as an explicit **off** switch, which is
/// worth keeping: it is how you boot a provisioned deployment with the seam
/// disabled to isolate whether a failure is WebRTC's.
pub fn webrtc_enable_from_query(query: &str) -> Option<bool> {
    query_param(query, "webrtc_enable").map(|v| {
        let v = v.trim();
        v.is_empty() || v == "1" || v.eq_ignore_ascii_case("true")
    })
}

/// The install shortfall (v12) — peer-ids we asked to `webrtc_enabled` that the
/// worker's `WireCaps.webrtc_peers` report says did NOT get a §6.5 establisher.
/// Empty = every enabled peer installed one. The worker-host refuses Init rather
/// than installing nothing, so today this is always empty — computing it anyway
/// is what keeps "the establisher installed" a **verified** property rather than
/// a remembered one (D13; the report is per-peer precisely so it can be diffed
/// against the per-peer request — never collapsed to one worker-wide bool).
pub fn webrtc_install_shortfall(requested: &[String], installed: &[String]) -> Vec<String> {
    requested
        .iter()
        .filter(|p| !installed.iter().any(|i| i == *p))
        .cloned()
        .collect()
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
/// Returns an **i18n key**, not display text: the refusal is shown to the user
/// as a banner (`show_action_refused_banner`), so the caller resolves it with
/// `t()` at the point of display. Keeping the key `&'static str` also lets the
/// tracing log and the unit tests assert on a stable identifier rather than on
/// prose that a copy-edit would break.
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
        return Some("peercreate.disabled");
    }
    if !can_persist {
        return Some("peercreate.cannot_save");
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

    /// Config/deploy hardening: the status-bar site toggle is suppressed when
    /// boot landed in a `Window` surface, EVEN IF the posture would otherwise
    /// expose it — a bad deployment (`show_toggle=true` + `surface=window`)
    /// can't strand a Site Browser window with a stray "View Site" toggle
    /// (fbdc0822). The overlay-side `exposes_toggle` is unchanged (a Window boot
    /// isn't in the overlay, so its Exit control is moot).
    #[test]
    fn window_surface_suppresses_status_toggle_even_if_posture_exposes_it() {
        let exposing = SiteModePosture { enabled: true, show_toggle: true, locked: false };
        assert!(exposing.exposes_toggle(), "posture alone would expose the toggle");

        // Chrome / Site boots honor the posture predicate…
        let chrome = SessionConfig { boot_surface: BootSurface::Chrome, site_mode: exposing.clone(), ..SessionConfig::default() };
        assert!(chrome.status_toggle_visible(), "chrome boot exposes the toggle");
        let site = SessionConfig { boot_surface: BootSurface::Site, site_mode: exposing.clone(), ..SessionConfig::default() };
        assert!(site.status_toggle_visible(), "site boot exposes the toggle");

        // …a Window boot suppresses it regardless of the (mis-emitted) posture.
        let window = SessionConfig {
            boot_surface: BootSurface::Window { peer_id: String::new(), window_type: SITE_BROWSER_WINDOW.into() },
            site_mode: exposing,
            ..SessionConfig::default()
        };
        assert!(
            !window.status_toggle_visible(),
            "a Window deployment must NOT expose the status-bar site toggle"
        );
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
            Some("peercreate.disabled")
        );
        assert_eq!(
            peer_create_refusal_reason(false, false),
            Some("peercreate.disabled"),
            "capability is reported first (deployment intent)"
        );
        // Enabled but not durable → the durability message.
        assert_eq!(
            peer_create_refusal_reason(false, true),
            Some("peercreate.cannot_save")
        );
        // The keys must resolve — a refusal that renders as a raw key is the
        // silent-failure D13 forbids.
        for key in ["peercreate.disabled", "peercreate.cannot_save"] {
            assert_ne!(crate::i18n::t(key, &[]), key, "{key} missing from EN");
        }
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

    /// The reflector parser — the whole of what stands between a typed URL and
    /// an ICE agent that gathers server-reflexive candidates.
    #[test]
    fn reflectors_parse_or_say_why_not() {
        // One server carrying every URL: that is what an RTCIceServer with no
        // credentials is, and §9.3 forbids authenticating a reflector.
        let s = parse_ice_urls("stun:a.example:3478, stun:b.example:3478").expect("valid");
        assert_eq!(s.len(), 1, "credential-free reflectors are ONE server entry");
        assert_eq!(s[0].urls, vec!["stun:a.example:3478", "stun:b.example:3478"]);
        assert!(s[0].username.is_none() && s[0].credential.is_none());
        // Whitespace, commas, or both.
        assert_eq!(parse_ice_urls("stun:a:1\n stun:b:2").unwrap()[0].urls.len(), 2);
        // Empty is NOT an error: host-only is a legal deployment, and every
        // build before this field shipped that way.
        assert!(parse_ice_urls("").unwrap().is_empty());
        assert!(parse_ice_urls("   ").unwrap().is_empty());

        // TURN is refused with its reason, not silently accepted into an entry
        // that would gather no relay candidates.
        let e = parse_ice_urls("turn:relay.example:3478").expect_err("turn needs credentials");
        assert!(e.contains("username"), "the refusal must say what is missing: {e}");
        // RFC 7064 has no authority component; `stun://` is the typo to catch.
        assert!(parse_ice_urls("stun://a.example:3478").is_err(), "no '//' in a STUN URI");
        assert!(parse_ice_urls("stun:").is_err(), "names no host");
        assert!(parse_ice_urls("https://a.example").is_err(), "not a reflector");
        assert!(parse_ice_urls("a.example:3478").is_err(), "no scheme");
    }

    /// A malformed list must not cost the user their *rendezvous*. Meet and
    /// discovery ride the node and work fine on host candidates, so a typo in an
    /// optional field degrades to host-only (loudly, via `warn!`) rather than
    /// failing the node. The place it is *refused* is `add_connector`, where
    /// there is a user to tell.
    #[test]
    fn a_bad_reflector_list_costs_reflexivity_not_the_node() {
        let p = resolve_webrtc_provisioning(
            Some("node-1"),
            Some("ws://n:9000"),
            Some("turn:relay.example:3478"),
        )
        .expect("the NODE is still provisioned — rendezvous does not need ICE");
        assert!(p.ice_servers.is_empty(), "and the session is host-only");
    }

    #[test]
    fn provisioning_carries_the_reflectors_it_is_given() {
        let p = resolve_webrtc_provisioning(
            Some("node-1"),
            Some("ws://n:9000"),
            Some(" stun:stun.example.org:3478 "),
        )
        .expect("provisioned");
        assert_eq!(p.ice_servers.len(), 1);
        assert_eq!(p.ice_servers[0].urls, vec!["stun:stun.example.org:3478"]);
    }

    #[test]
    fn webrtc_provisioning_requires_both_node_fields() {
        // Both present → provisioned, trimmed.
        let p = resolve_webrtc_provisioning(Some(" node-1 "), Some(" ws://n:9000 "), None)
            .expect("both fields present");
        assert_eq!(p.node_peer_id, "node-1");
        assert_eq!(p.node_addr, "ws://n:9000");
        assert!(p.ice_servers.is_empty(), "no reflectors named → host-only");
        assert_eq!(p.poll_interval_ms, None);

        // A lone half is an unauthenticated rendezvous / a peer with nowhere to
        // dial — fails closed to None, never a half-config on the wire.
        assert!(resolve_webrtc_provisioning(Some("node-1"), None, None).is_none(), "addr missing");
        assert!(resolve_webrtc_provisioning(None, Some("ws://n"), None).is_none(), "peer-id missing");
        // Blank counts as absent (a build knob left as "").
        assert!(resolve_webrtc_provisioning(Some("node-1"), Some("   "), None).is_none(), "blank addr");
        assert!(resolve_webrtc_provisioning(Some(""), Some("ws://n"), None).is_none(), "blank peer-id");
        assert!(resolve_webrtc_provisioning(None, None, None).is_none(), "neither → v10 inert");
    }

    #[test]
    fn webrtc_provisioning_from_query_reads_both_node_fields() {
        // A dynamic ws addr (unencoded `:` `/` are query-legal) + peer-id.
        let q = "?worker=1&webrtc_node=ws://127.0.0.1:4041&webrtc_node_peer=node-x&log=debug";
        let p = webrtc_provisioning_from_query(q).expect("both present");
        assert_eq!(p.node_addr, "ws://127.0.0.1:4041");
        assert_eq!(p.node_peer_id, "node-x");
        // Leading '?' optional; only one half present → None (fail closed).
        assert!(webrtc_provisioning_from_query("webrtc_node=ws://x").is_none());
        assert!(webrtc_provisioning_from_query("a=1&b=2").is_none(), "absent → None");
    }

    #[test]
    fn webrtc_enable_from_query_is_a_tri_state() {
        assert_eq!(webrtc_enable_from_query("?webrtc_enable=1"), Some(true));
        assert_eq!(webrtc_enable_from_query("?webrtc_enable=true"), Some(true));
        assert_eq!(webrtc_enable_from_query("?webrtc_enable"), Some(true), "bare = on");
        assert_eq!(webrtc_enable_from_query("?webrtc_enable=0"), Some(false));
        assert_eq!(webrtc_enable_from_query("?webrtc_enable=no"), Some(false));
        assert_eq!(webrtc_enable_from_query("?worker=1"), None, "absent → defer to knob");
    }

    #[test]
    fn webrtc_install_shortfall_diffs_requested_against_installed() {
        let p = |s: &str| s.to_string();
        // Everything we enabled installed → no shortfall.
        assert!(webrtc_install_shortfall(&[p("a"), p("b")], &[p("a"), p("b")]).is_empty());
        // An enabled peer absent from the report is the shortfall (the D13 case).
        assert_eq!(
            webrtc_install_shortfall(&[p("a"), p("b")], &[p("a")]),
            vec![p("b")]
        );
        // Extra installed peers we didn't ask about are not a shortfall.
        assert!(webrtc_install_shortfall(&[p("a")], &[p("a"), p("b")]).is_empty());
        // Nothing requested → never a shortfall, regardless of the report.
        assert!(webrtc_install_shortfall(&[], &[p("a")]).is_empty());
    }

    #[test]
    fn webrtc_defaults_are_inert_on_a_plain_build() {
        // This suite builds with none of the ENTITY_WEBRTC_* knobs set, so the
        // default provisioning is absent — exactly v10. (A knobbed build is
        // exercised by resolve_* above; the env path is a thin `option_env!`
        // feed with no branching to test.)
        assert!(
            webrtc_provisioning_default().is_none(),
            "no ENTITY_WEBRTC_NODE_* baked → no capability (v10)"
        );
        // …and with no capability there is nothing to install, whatever the URL
        // says. This is the whole of what survived the enable axis: fail closed.
        assert!(!webrtc_install_primary(false, None), "no node → no establisher");
        assert!(
            !webrtc_install_primary(false, Some(true)),
            "?webrtc_enable=1 cannot conjure a node — the worker-host would reject the Init"
        );
    }

    #[test]
    fn a_provisioned_node_is_the_install_decision() {
        // The bug this encodes: for as long as the install needed a *second*,
        // build-time-only knob, every build a user actually runs resolved a node
        // (from their selected connector) and then installed nothing — so the
        // connector registry and the naming modes both led to a connect that
        // could not happen. Provisioned now means installed.
        assert!(
            webrtc_install_primary(true, None),
            "a resolved node, nothing said in the URL → install"
        );
        assert!(webrtc_install_primary(true, Some(true)), "explicitly on → install");
        // The one override that survives, and the reason it does: booting a
        // provisioned deployment with the seam off is how you isolate whether a
        // failure is WebRTC's.
        assert!(
            !webrtc_install_primary(true, Some(false)),
            "?webrtc_enable=0 refuses a node we do have"
        );
    }
}
