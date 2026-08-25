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

impl WebRtcProvisioning {
    /// Append a parsed relay to [`Self::ice_servers`].
    ///
    /// Additive rather than a fourth argument to [`resolve_webrtc_provisioning`]
    /// because only the connector-registry path has a relay to supply: the URL
    /// query and the build knob carry reflectors only, and widening the shared
    /// signature would make every caller state a `None` it has no opinion about.
    ///
    /// **A second `IceServer`, never merged into the first.** An `RTCIceServer`
    /// carries one credential pair for all its URLs, so folding a credentialed
    /// relay in beside credential-free reflectors would either attach the
    /// username to the reflectors (which §9.3 forbids them to have) or drop it
    /// from the relay (which gathers no relay candidates). Two entries is what
    /// the browser API means.
    pub fn with_relay(mut self, relay: Option<IceServer>) -> Self {
        if let Some(r) = relay {
            self.ice_servers.push(r);
        }
        self
    }

    /// Is a **reflector** provisioned — as opposed to *any* ICE server?
    ///
    /// **The distinction only came into existence when the relay field landed,
    /// and reading `!ice_servers.is_empty()` for it is now wrong.** That list is
    /// mixed: a relay-only session has one entry and no reflector at all. The
    /// consumer is [`crate::reachability`], where this single bool is the whole
    /// difference between *"we asked a reflector and learned nothing"* and *"we
    /// never asked"* — so conflating the two tells a user with a rented relay
    /// and no reflector that **their reflector did not answer**, sending them to
    /// fix something they never configured. That is the cry-wolf failure the
    /// classifier exists to prevent, arriving through the back door.
    pub fn has_reflector(&self) -> bool {
        self.ice_servers.iter().any(|s| !s.is_relay())
    }

    /// Is a **relay** provisioned? Not yet an input to the classifier — see the
    /// `RelayUnreachable` gap in `BUILDOUT-SIGNALING-AND-NETWORK-EXTENSIONS.md`
    /// §8C item 27 — but it is the other half of [`Self::has_reflector`] and the
    /// two must be read off the same discriminator.
    ///
    /// Deliberately built and left unwired: shipping it with `has_reflector`
    /// keeps the pair symmetric and means the `RelayUnreachable` arm is a
    /// classifier change alone. Allowed rather than deleted per the standing
    /// rule on no-caller surfaces.
    #[allow(dead_code)]
    pub fn has_relay(&self) -> bool {
        self.ice_servers.iter().any(|s| s.is_relay())
    }
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

impl IceServer {
    /// **An entry is a relay iff it carries credentials.** The one discriminator
    /// — `parse_relay` enforces it on the way in (both halves or a refusal),
    /// `connectors::pack_mirror` partitions the localStorage mirror on it, and
    /// [`WebRtcProvisioning::has_reflector`] reads it back out. Three call sites
    /// that must agree, so they ask the same function rather than each spelling
    /// the predicate: the mirror already ate a relay once by disagreeing with
    /// the parser about what a relay looks like.
    ///
    /// Deliberately `||` rather than `&&`: a half-credentialed entry cannot be
    /// built through `parse_relay`, and if one ever appears it is a relay we
    /// should not mistake for a reflector.
    pub fn is_relay(&self) -> bool {
        self.username.is_some() || self.credential.is_some()
    }
}

/// Parse a user-supplied reflector list into [`IceServer`] entries.
///
/// Accepts comma- and/or whitespace-separated URLs and returns them as **one**
/// `IceServer` carrying every URL — which is what an `RTCIceServer` with no
/// credentials is, and matches `EXTENSION-SIGNALING` §9.3 forbidding reflector
/// authentication: no credential, nothing to expire, nothing to rotate.
///
/// **`turn:`/`turns:` is refused here and belongs in the relay field** — which
/// exists (see [`parse_relay`] directly below, and the three boxes on the
/// connector row). Accepting one *here* would build an `RTCIceServer` with no
/// credentials that silently gathers no relay candidates — a reflector that
/// looks configured and does nothing, the failure mode this whole area keeps
/// producing — so `validate_reflector_uri` refuses it **by pointing at the
/// right field**, not by saying the product cannot do it.
///
/// This paragraph used to end *"carrying credentials is a later, additive
/// shape"*. That shape landed. **A concession's expiry is worth writing down
/// where the concession is made, and then the note has to be spent** — a
/// comment describing a limitation the code no longer has reads as a product
/// statement, and this one had already been copied into the release notes as
/// one.
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
        validate_reflector_uri(u)?;
    }
    Ok(vec![IceServer { urls, username: None, credential: None }])
}

/// Parse a user-supplied **relay** (TURN) list plus its credentials into an
/// [`IceServer`], or `Ok(None)` when no relay is configured.
///
/// # Why this is a separate field from the reflectors, and not one list
///
/// A reflector and a relay are different kinds of thing and the split is not
/// cosmetic. A reflector is a commodity — credential-free by spec
/// (`EXTENSION-SIGNALING` §9.3 forbids reflector authentication: no credential,
/// nothing to expire, nothing to rotate), and **node-advertisable**, which is
/// why `Connector::ice_advertised` exists and why its dedup is defined over
/// published bytes exactly. A relay is rented, carries credentials, forwards
/// every packet, and `EXTENSION-REGISTRY` §3b deliberately has **no credential
/// channel** — a node cannot advertise one. Merging them into one field would
/// put a credentialed entry into the list §4.5.1 merges byte-for-byte.
///
/// # The refusal that is the whole point
///
/// A relay URL **without** both a username and a credential is refused. It is
/// not a harmless partial config: `RTCPeerConnection` accepts it, the entry
/// looks configured in every surface, and it gathers **no relay candidates at
/// all**. That is the exact failure mode this area keeps producing — something
/// that looks set up and silently does nothing — and it is worse than an empty
/// field, because an empty field is at least legible. Refuse it where the user
/// typed it, and say which half is missing.
///
/// A `stun:` URI here is refused too, pointing back at the reflector field: the
/// two lists have different credential semantics, and a reflector smuggled in
/// here would be handed a username it must not have.
///
/// # Where the credential lives
///
/// In the connector row, in this peer's own tree, in plaintext — the same place
/// and the same protection as the rest of the app's configuration. Worth stating
/// rather than implying: a TURN credential is usually a shared, rotatable
/// secret, and this is not a secret store.
pub fn parse_relay(
    raw_urls: &str,
    username: &str,
    credential: &str,
) -> Result<Option<IceServer>, String> {
    let urls: Vec<String> = raw_urls
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let (user, cred) = (username.trim(), credential.trim());

    if urls.is_empty() {
        // No relay is the norm and is not an error — but credentials with no
        // URL to attach them to is a half-filled form, and saying so beats
        // discarding what was typed.
        if !user.is_empty() || !cred.is_empty() {
            return Err(
                "a relay username/credential needs a relay URL — \
                 add one like turn:relay.example.org:3478" // i18n-ignore
                    .to_string(),
            );
        }
        return Ok(None);
    }

    for u in &urls {
        let scheme = u.split_once(':').map(|(s, _)| s.to_ascii_lowercase());
        match scheme.as_deref() {
            Some("turn") | Some("turns") => {
                if u.contains("//") {
                    return Err(format!(
                        "'{u}' is not a TURN URI — RFC 7065 is turn:host[:port], with no '//'" // i18n-ignore
                    ));
                }
                if u.split_once(':').map(|(_, r)| r.trim().is_empty()).unwrap_or(true) {
                    return Err(format!("'{u}' names no host")); // i18n-ignore
                }
            }
            Some("stun") | Some("stuns") => {
                return Err(format!(
                    "'{u}' is a reflector (STUN) — put it in the Reflectors field. \
                     A reflector takes no credentials" // i18n-ignore
                ));
            }
            _ => {
                return Err(format!(
                    "'{u}' is not a relay URI — expected turn:host[:port]" // i18n-ignore
                ))
            }
        }
    }

    // Name the missing half specifically. "Invalid relay" would send the user
    // looking at the URL they got right.
    match (user.is_empty(), cred.is_empty()) {
        (true, true) => Err(
            "a relay needs a username and a credential — without them it gathers \
             no relay candidates while looking configured" // i18n-ignore
                .to_string(),
        ),
        (true, false) => Err("a relay needs a username as well as a credential".to_string()), // i18n-ignore
        (false, true) => Err("a relay needs a credential as well as a username".to_string()), // i18n-ignore
        (false, false) => Ok(Some(IceServer {
            urls,
            username: Some(user.to_string()),
            credential: Some(cred.to_string()),
        })),
    }
}

/// Validate ONE reflector URI against `EXTENSION-SIGNALING` §4.5.1 /
/// `EXTENSION-REGISTRY` §3b.0 — the RFC 7064 form, which both fields pin
/// identically and which MUST NOT differ in shape between them.
///
/// **Split out from [`parse_ice_urls`] because the two callers need different
/// failure granularity, and that difference is deliberate.** A *typed* list is
/// all-or-nothing: it is refused whole at `connectors::add_connector` so the
/// user fixes their typo. A *node-advertised* list is merged per entry — one
/// malformed entry from a remote node must not discard the reflectors the user
/// configured themselves, because the node is not the user's to correct.
///
/// Never repairs. §4.5.1 pins the published form precisely so that no consumer
/// runs a transform: given `1.2.3.4:3478` one consumer prepends `stun:` and
/// another does not, and a prepending consumer handed `stun:1.2.3.4:3478`
/// produces `stun:stun:…`. A browser hands these to `RTCIceServer.urls`
/// verbatim, where a malformed entry **throws at `RTCPeerConnection`
/// construction** rather than degrading to host-only.
pub fn validate_reflector_uri(u: &str) -> Result<(), String> {
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
            Ok(())
        }
        Some("turn") | Some("turns") => Err(format!(
            "'{u}' is a relay (TURN), which needs a username and credential — \
             put it in the Relay field, not here" // i18n-ignore
        )),
        _ => Err(format!(
            "'{u}' is not a reflector URI — expected stun:host[:port]" // i18n-ignore
        )),
    }
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
    /// **This resolver's own ceiling on a name binding's lifetime, in ms**
    /// (`EXTENSION-REGISTRY` §6a, 1.11) — set by `/entity-deployment.json`'s
    /// `name_resolver_max_ttl_ms`. `None` = no ceiling declared, which is
    /// conformant (§6a makes it a MAY) and is what we ship, because there is no
    /// defensible constant and baking one in makes every unconfigured
    /// deployment look configured.
    ///
    /// It rides the durable config rather than being read from the deployment
    /// doc at use time, because a **returning** profile never fetches that doc
    /// (persisted > fetched, D16) — a ceiling read only at fetch time would
    /// apply on a fresh boot and silently not on a warm one, which is the
    /// security-half-nobody-can-reach shape. Known limit, inherited from the
    /// same model and not new here: a profile that already persisted a config
    /// keeps its old ceiling until that config is refreshed.
    pub name_resolver_max_ttl_ms: Option<u64>,
    /// **The registry this deployment seeds as a pin** (`EXTENSION-REGISTRY`
    /// §7.4's *"preloaded Entity System Registry"*), set by
    /// `/entity-deployment.json`'s `name_registry_pin`. `None` = no default,
    /// which is what a generic build ships: fail-closed, and `name pin` is the
    /// only way to get one.
    ///
    /// **It seeds; it never overwrites.** A pin the user typed outranks it for
    /// as long as that pin exists (the shell holds one per tab), because a pin
    /// is a trust decision and a deployment does not get to revise the user's.
    ///
    /// Same D16 reason as the ceiling above for riding the durable config rather
    /// than the fetched document: a returning profile never re-fetches
    /// `/entity-deployment.json`, so a pin read only at fetch time would apply
    /// on a cold boot and silently not on a warm one [AP22].
    pub name_registry_pin: Option<RegistryPin>,
}

/// A registry pin: **the one string a consumer holds a priori**, plus where that
/// registry is served from.
///
/// The peer-id is the whole of the trust decision — for Ed25519 canonical form
/// it *embeds* the 32-byte public key, so nothing is fetched to learn who the
/// registry is. The origin is trusted for nothing; it is only where bytes come
/// from, and every byte is checked against a hash chaining to a signature by the
/// pinned key. An empty origin means same-origin (the SPA expands it), matching
/// the `origins` map's convention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryPin {
    pub peer_id: String,
    pub origin: String,
}

/// The resolver ceiling this session will honor, mirrored out of the durable
/// [`SessionConfig`] for the paths that cannot reach one.
///
/// Same shape and same reason as [`crate::boot_fast_paint`]'s localStorage
/// mirror: the shell's `name` verb resolves inside a `spawn_task` and holds no
/// config handle, and threading one through every future call site is how a
/// security half ends up applied on some paths and not others. Set once at boot
/// from the resolved config; read wherever a resolution happens.
///
/// A `Cell<Option<u64>>` rather than anything shared: the browser arm is
/// single-threaded, and on native this is only read by tests.
mod resolver_ceiling {
    use std::cell::Cell;

    thread_local! {
        static ACTIVE: Cell<Option<u64>> = const { Cell::new(None) };
    }

    /// Install the ceiling the resolved config declares. Called at boot.
    pub fn set(ms: Option<u64>) {
        ACTIVE.with(|c| c.set(ms));
    }

    /// The ceiling in force, in ms. `None` = none declared (conformant).
    pub fn get() -> Option<u64> {
        ACTIVE.with(|c| c.get())
    }
}

pub use resolver_ceiling::{get as active_resolver_ceiling_ms, set as set_active_resolver_ceiling};

/// The deployment-seeded registry pin in force, mirrored out of the durable
/// [`SessionConfig`] for the same reason and by the same mechanism as
/// [`resolver_ceiling`]: the shell's `name` verb resolves inside a `spawn_task`
/// holding no config handle.
///
/// `RefCell<Option<..>>` rather than `Cell`, only because the value is not
/// `Copy`; the browser arm is single-threaded and on native this is read by
/// tests. Keyed by nothing — there is exactly one, so four sites shipping the
/// same pin converge by construction (design §4 constraint 3), and two sites
/// shipping *different* registries means whichever origin you booted from is
/// the one you resolve through, which is the honest per-deployment answer.
mod registry_pin {
    use super::RegistryPin;
    use std::cell::RefCell;

    thread_local! {
        static ACTIVE: RefCell<Option<RegistryPin>> = const { RefCell::new(None) };
    }

    /// Install the pin the resolved config declares. Called at boot.
    pub fn set(pin: Option<RegistryPin>) {
        ACTIVE.with(|c| *c.borrow_mut() = pin);
    }

    /// The deployment-seeded pin, if this deployment ships one.
    pub fn get() -> Option<RegistryPin> {
        ACTIVE.with(|c| c.borrow().clone())
    }
}

pub use registry_pin::{get as active_registry_pin, set as set_active_registry_pin};

/// The registry pin the **user** chose, above the deployment's seed.
///
/// # Why this is not a slot on the Shell
///
/// It was. `ShellModel::name_pin` was an in-memory `Mutex` on one *window*, so
/// `name pin` bound a registry the Registry Browser could not see — that window
/// read only the deployment seed and said "no registry pinned" with no affordance
/// anywhere in the GUI to supply one, its own comment recording that wiring the
/// two together was "deliberately not answered". The result is one of this app's
/// three disjoint stores in miniature: two surfaces, one concept, no shared
/// state, and the answer depending on which window you asked.
///
/// So the pin lives in **one** place that both read, and
/// [`pinned_registry`] is the only expression of the precedence.
///
/// # Why a localStorage mirror rather than the tree
///
/// The tree is where state belongs, and a pin nearly qualifies. What disqualifies
/// it is *when* it is read: the shell resolves inside a `spawn_task` holding no
/// config handle, and on the Worker arm a boot-time tree read returns the default
/// because the cache mirror is not seeded. That is the same constraint
/// `connectors::write_selection_mirror` and `boot_fast_paint` already answer the
/// same way, so this uses their idiom rather than inventing a fourth.
///
/// Clearing on `None` is load-bearing for their reason too: a stale mirror would
/// keep resolving names through a registry the user has unpinned, silently.
mod user_registry_pin {
    use super::RegistryPin;
    use std::cell::RefCell;

    /// `\x1f`-joined `peer_id` + `origin`, matching the packing used elsewhere
    /// for two-field values. An origin may legitimately be empty (same-origin).
    #[cfg(target_arch = "wasm32")]
    const MIRROR_KEY: &str = "entity-browser:registry-pin";

    thread_local! {
        static CHOSEN: RefCell<Option<RegistryPin>> = const { RefCell::new(None) };
    }

    /// Record the user's choice, in memory and in the mirror.
    pub fn set(pin: Option<RegistryPin>) {
        CHOSEN.with(|c| *c.borrow_mut() = pin.clone());
        #[cfg(target_arch = "wasm32")]
        write_mirror(pin.as_ref());
    }

    pub fn get() -> Option<RegistryPin> {
        CHOSEN.with(|c| c.borrow().clone())
    }

    /// Restore the user's pin at boot. Called before the deployment config is
    /// applied, because the deployment only ever *seeds*: a user who has pinned
    /// a registry must not silently be moved back onto the deployment's.
    #[cfg(target_arch = "wasm32")]
    pub fn restore_from_mirror() {
        let Some(raw) = web_sys::window()
            .and_then(|w| w.local_storage().ok().flatten())
            .and_then(|s| s.get_item(MIRROR_KEY).ok().flatten())
        else {
            return;
        };
        // A pin with no peer-id is dropped rather than completed from the
        // origin — pinning an origin would trust the origin, which is the one
        // thing this chain never does (`DeploymentConfig`'s parser, same rule).
        let (peer_id, origin) = raw.split_once('\u{1f}').unwrap_or((raw.as_str(), ""));
        if peer_id.trim().is_empty() {
            return;
        }
        CHOSEN.with(|c| {
            *c.borrow_mut() = Some(RegistryPin {
                peer_id: peer_id.trim().to_string(),
                origin: origin.trim().to_string(),
            })
        });
    }

    #[cfg(target_arch = "wasm32")]
    fn write_mirror(pin: Option<&RegistryPin>) {
        let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten())
        else {
            return;
        };
        match pin {
            Some(p) => {
                let _ = storage
                    .set_item(MIRROR_KEY, &format!("{}\u{1f}{}", p.peer_id, p.origin));
            }
            None => {
                let _ = storage.remove_item(MIRROR_KEY);
            }
        }
    }
}

pub use user_registry_pin::{get as user_registry_pin, set as set_user_registry_pin};
#[cfg(target_arch = "wasm32")]
pub use user_registry_pin::restore_from_mirror as restore_user_registry_pin;

/// Where a registry pin came from. Surfaces **must** say which [AP25]: a name
/// resolving through a registry the user never chose, silently, is
/// indistinguishable from one they did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinSource {
    /// The user pinned it here, in this app.
    User,
    /// `/entity-deployment.json` seeded it — `EXTENSION-REGISTRY` §7.4.
    Deployment,
}

/// The registry pin in force and where it came from: **the user's choice above
/// the deployment's seed**.
///
/// The single expression of that precedence. Every surface calls this; none
/// keeps its own slot. There is still exactly ONE pin, not a resolver chain —
/// `name_dispatch::default_rules()` matches the ratified §4.1a table and is
/// deliberately installed nowhere, because a catch-all needs a default registry
/// to point at and shipping one *for everybody* is how two app tiers ship two.
pub fn pinned_registry() -> Option<(RegistryPin, PinSource)> {
    if let Some(p) = user_registry_pin() {
        return Some((p, PinSource::User));
    }
    active_registry_pin().map(|p| (p, PinSource::Deployment))
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
            // No ceiling declared by default — see the field docs. Conformant,
            // and honest: we do not ship a number nobody can defend.
            name_resolver_max_ttl_ms: None,
            // No default registry either — a generic build pins nothing, and
            // `name` fails closed until someone pins one. Only a deployment that
            // says so seeds it.
            name_registry_pin: None,
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
        let mut pin_peer = String::new();
        let mut pin_origin = String::new();
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
                // Absent in any config written before the resolver ceiling
                // existed → stays `None` (no ceiling declared). Zero is dropped
                // for the same reason the deployment parser drops it: it would
                // expire every binding instantly and read as a broken registry.
                Some("name_resolver_max_ttl_ms") => {
                    if let Some(ms) = entity_ecf::ValueExt::as_u64(v) {
                        cfg.name_resolver_max_ttl_ms = Some(ms).filter(|m| *m > 0);
                    }
                }
                // The pin's two halves round-trip as two text fields; an empty
                // peer-id encodes "no pin" (the origin alone is meaningless, so
                // it is the peer-id that decides). Absent in any config written
                // before the pin existed → stays `None`.
                Some("name_registry_pin_peer") => {
                    if let Some(s) = v.as_text() {
                        pin_peer = s.to_string();
                    }
                }
                Some("name_registry_pin_origin") => {
                    if let Some(s) = v.as_text() {
                        pin_origin = s.to_string();
                    }
                }
                _ => {}
            }
        }
        // Assemble boot_surface from the structured fields. An unknown / absent
        // kind keeps the default (`Full` → Chrome) — garbage-tolerant.
        // The peer-id decides: an origin with no peer-id is not a pin, it is a
        // URL, and pinning it would trust the origin — the one thing this chain
        // never does.
        cfg.name_registry_pin = (!pin_peer.is_empty())
            .then_some(RegistryPin { peer_id: pin_peer, origin: pin_origin });
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
            "peer_creation_enabled" => entity_ecf::bool_val(self.peer_creation_enabled),
            // `0` encodes "no ceiling declared" on the wire; `from_entity`
            // drops a zero back to `None`, so the round-trip is total and an
            // undeclared ceiling never comes back as an instant expiry.
            "name_resolver_max_ttl_ms" =>
                entity_ecf::uinteger(self.name_resolver_max_ttl_ms.unwrap_or(0)),
            // An empty peer-id encodes "no pin", so the round-trip is total the
            // same way the ceiling's zero is.
            "name_registry_pin_peer" => entity_ecf::text(
                self.name_registry_pin.as_ref().map(|p| p.peer_id.as_str()).unwrap_or("")
            ),
            "name_registry_pin_origin" => entity_ecf::text(
                self.name_registry_pin.as_ref().map(|p| p.origin.as_str()).unwrap_or("")
            )
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
/// One query parameter, **percent-decoded**.
///
/// The decode is not optional and its absence was a live bug: the desktop's SPA
/// server redirects a bare `/` to `?webrtc_node=ws%3A%2F%2F…` (a `ws://` URL
/// must be encoded or a parser truncates it at the first `:`), and reading it
/// raw handed the establisher the literal string `ws%3A%2F%2F192.168.68.55%3A4041`
/// as a node address. Nothing downstream validates the scheme at runtime — only
/// `build.rs` does — so it **installed cleanly**, and `net` then reported
/// `OK rendezvous … at ws%3A%2F%2F…`: a green row containing an address no
/// socket could ever open. Measured in a real browser against a real desktop.
///
/// `+` is deliberately **not** treated as a space: this is a URL query, not an
/// `application/x-www-form-urlencoded` form body, and a `+` in a peer-id or an
/// address is a literal.
fn query_param<'a>(query: &'a str, key: &str) -> Option<std::borrow::Cow<'a, str>> {
    let raw = query.trim_start_matches('?').split('&').find_map(|pair| {
        let mut parts = pair.splitn(2, '=');
        (parts.next() == Some(key)).then(|| parts.next().unwrap_or(""))
    })?;
    Some(percent_decode(raw))
}

/// Decode `%XX` escapes. Borrows when there is nothing to decode, which is the
/// common case (a Base58 peer-id needs no escaping).
fn percent_decode(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('%') {
        return std::borrow::Cow::Borrowed(s);
    }
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) =
                ((b[i + 1] as char).to_digit(16), (b[i + 2] as char).to_digit(16))
            {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    // A malformed escape yields the original rather than an error: this feeds a
    // fail-closed resolver, and handing it undecodable bytes is a better outcome
    // than a panic on a URL somebody typed.
    match String::from_utf8(out) {
        Ok(decoded) => std::borrow::Cow::Owned(decoded),
        Err(_) => std::borrow::Cow::Borrowed(s),
    }
}

/// Runtime WebRTC provisioning from a URL query — the dev/showcase / e2e
/// injection channel, **higher precedence than the build knob** and never
/// persisted (`deployment_config.rs` precedence). Reads `webrtc_node_peer` +
/// `webrtc_node`; both required (via [`resolve_webrtc_provisioning`]). Its
/// reason for existing: a per-test signaling node has a **dynamic** address the
/// compile-time knob cannot carry.
pub fn webrtc_provisioning_from_query(query: &str) -> Option<WebRtcProvisioning> {
    // Bound before the call: each `Cow` must outlive the `&str` handed on.
    let node_peer = query_param(query, "webrtc_node_peer");
    let node_addr = query_param(query, "webrtc_node");
    // `?webrtc_ice=stun:host:3478` — the rung-2 harness channel, same reason
    // the node halves are here: a per-test reflector has an address the
    // compile-time knob cannot carry.
    let ice = query_param(query, "webrtc_ice");
    resolve_webrtc_provisioning(
        node_peer.as_deref(),
        node_addr.as_deref(),
        ice.as_deref(),
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
            // A declared ceiling rides the round-trip too: it is `Option<u64>`
            // over a wire that has no null, so `0` encodes "undeclared" and
            // `from_entity` maps it back — a field that survived `to_entity` and
            // came back as an instant expiry would expire every name.
            name_resolver_max_ttl_ms: Some(3_600_000),
            // And the §7.4 pin. Both halves, because an origin dropped in the
            // round-trip is a pin that resolves WHO but not WHERE — the same
            // shape as the relay the localStorage mirror quietly ate.
            name_registry_pin: Some(RegistryPin {
                peer_id: "2KRegistryPeer".into(),
                origin: "https://registry.example".into(),
            }),
        };
        assert_eq!(SessionConfig::from_entity(&cfg.to_entity()), cfg);
    }

    /// **S6 — the pin survives the boot a default registry usually dies on.**
    ///
    /// A cold boot fetches `/entity-deployment.json` and applies it; every boot
    /// after that reads the *persisted* config and fetches nothing (D16). So the
    /// path a seeded pin has to survive is not the fetch — it is the round trip
    /// through the durable entity, and then into the mirror that the shell's
    /// `spawn_task` can actually read.
    ///
    /// Simulated here end to end, because each hop has already eaten a field
    /// once: the relay was lost in a mirror that round-tripped through the tree
    /// perfectly, with every native test green.
    #[test]
    fn a_seeded_registry_pin_survives_a_warm_boot_and_reaches_the_mirror() {
        use crate::deployment_config::DeploymentConfig;

        // Cold boot: the deployment doc, applied over the build default.
        let deployment = DeploymentConfig::parse(
            r#"{"name_registry_pin": {"peer_id": "2KSeeded", "origin": "https://reg.example"}}"#,
        )
        .expect("parses");
        let cold = deployment.apply_to(SessionConfig::default());
        assert_eq!(cold.name_registry_pin.as_ref().map(|p| p.peer_id.as_str()), Some("2KSeeded"));

        // …persisted, and read back on a WARM boot with no fetch at all.
        let warm = SessionConfig::from_entity(&cold.to_entity());
        assert_eq!(
            warm.name_registry_pin, cold.name_registry_pin,
            "a warm boot fetches nothing, so the pin has to be in the durable config"
        );

        // …and installed where a resolution can reach it. Reading the mirror is
        // the assertion that matters: a pin sitting in a `SessionConfig` nobody
        // consults is the security-half-nobody-can-reach shape [AP22].
        set_active_registry_pin(warm.name_registry_pin.clone());
        let live = active_registry_pin().expect("the mirror carries it");
        assert_eq!(live.peer_id, "2KSeeded");
        assert_eq!(live.origin, "https://reg.example");
        set_active_registry_pin(None);
    }

    /// **A config written before the pin existed comes back with no pin**, and a
    /// pin with an empty peer-id is not a pin. Both are the same rule as the
    /// ceiling's zero: the wire has no null, so the absent value has to encode
    /// as something that round-trips back to `None` — and here that has to be
    /// the *peer-id*, since an origin alone would otherwise read as a registry
    /// to be trusted.
    #[test]
    fn an_origin_without_a_peer_id_is_not_a_pin() {
        let mut cfg = SessionConfig::default();
        cfg.name_registry_pin =
            Some(RegistryPin { peer_id: String::new(), origin: "https://registry.example".into() });
        assert_eq!(
            SessionConfig::from_entity(&cfg.to_entity()).name_registry_pin,
            None,
            "an empty peer-id must come back as no pin, not as a pin on an origin"
        );
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

        // A relay in the REFLECTOR field is refused and pointed at the right
        // one. The two lists have different credential semantics — §9.3 forbids
        // a reflector to carry credentials — so a relay smuggled in here would
        // silently lose the username it cannot work without.
        let e = parse_ice_urls("turn:relay.example:3478").expect_err("turn needs credentials");
        assert!(e.contains("username"), "the refusal must say what is missing: {e}");
        assert!(e.contains("Relay field"), "and where it goes instead: {e}");
        // RFC 7064 has no authority component; `stun://` is the typo to catch.
        assert!(parse_ice_urls("stun://a.example:3478").is_err(), "no '//' in a STUN URI");
        assert!(parse_ice_urls("stun:").is_err(), "names no host");
        assert!(parse_ice_urls("https://a.example").is_err(), "not a reflector");
        assert!(parse_ice_urls("a.example:3478").is_err(), "no scheme");
    }

    /// The relay parser, and the refusal it exists for.
    ///
    /// **A relay URL with no credentials is the failure this whole area keeps
    /// producing**: `RTCPeerConnection` accepts it, every surface shows it as
    /// configured, and it gathers no relay candidates at all. Refusing it where
    /// the user typed it — and naming *which* half is missing — is the entire
    /// point of splitting these into three fields.
    #[test]
    fn a_relay_needs_both_credentials_and_says_which_is_missing() {
        // The happy path: one entry, credentials attached.
        let s = parse_relay("turn:relay.example:3478", "alice", "s3cret")
            .expect("valid")
            .expect("some");
        assert_eq!(s.urls, vec!["turn:relay.example:3478"]);
        assert_eq!(s.username.as_deref(), Some("alice"));
        assert_eq!(s.credential.as_deref(), Some("s3cret"));
        // Comma- and whitespace-separated, same as the reflector field.
        assert_eq!(
            parse_relay("turn:a:1, turns:b:2", "u", "p").unwrap().unwrap().urls.len(),
            2
        );

        // No relay at all is the norm, not an error.
        assert!(parse_relay("", "", "").unwrap().is_none());
        assert!(parse_relay("   ", "", "").unwrap().is_none());

        // Each missing half names ITSELF. "Invalid relay" would send someone to
        // re-check the URL they got right.
        let both = parse_relay("turn:r:1", "", "").expect_err("needs both");
        assert!(both.contains("username") && both.contains("credential"), "{both}");
        let no_user = parse_relay("turn:r:1", "", "p").expect_err("needs a username");
        assert!(no_user.contains("username"), "{no_user}");
        let no_cred = parse_relay("turn:r:1", "u", "").expect_err("needs a credential");
        assert!(no_cred.contains("credential"), "{no_cred}");

        // Credentials with nowhere to attach are a half-filled form, and saying
        // so beats silently discarding what was typed.
        assert!(parse_relay("", "alice", "s3cret").is_err());

        // A reflector in the RELAY field is refused and pointed back — it would
        // otherwise be handed a username §9.3 says it must not have.
        let wrong = parse_relay("stun:a.example:3478", "u", "p").expect_err("stun is not a relay");
        assert!(wrong.contains("Reflectors field"), "{wrong}");
        // RFC 7065, like 7064, has no authority component.
        assert!(parse_relay("turn://r:1", "u", "p").is_err(), "no '//' in a TURN URI");
        assert!(parse_relay("turn:", "u", "p").is_err(), "names no host");
        assert!(parse_relay("https://r.example", "u", "p").is_err(), "not a relay URI");
    }

    /// A relay is a **second** `IceServer`, never folded into the reflectors.
    ///
    /// An `RTCIceServer` carries one credential pair for all its URLs, so
    /// merging would either attach the username to credential-free reflectors
    /// or drop it from the relay — and a relay with no credential gathers
    /// nothing, which is the exact silent-nothing this feature exists to stop.
    #[test]
    fn a_relay_rides_beside_the_reflectors_not_inside_them() {
        let p = resolve_webrtc_provisioning(
            Some("2KNode"),
            Some("ws://n:9000"),
            Some("stun:a.example:3478 stun:b.example:3478"),
        )
        .expect("provisions")
        .with_relay(parse_relay("turn:r.example:3478", "alice", "s3cret").unwrap());

        assert_eq!(p.ice_servers.len(), 2, "reflectors and relay are separate entries");
        // The reflectors keep NO credentials.
        assert_eq!(p.ice_servers[0].urls.len(), 2);
        assert!(p.ice_servers[0].username.is_none());
        assert!(p.ice_servers[0].credential.is_none());
        // The relay keeps both.
        assert_eq!(p.ice_servers[1].urls, vec!["turn:r.example:3478"]);
        assert_eq!(p.ice_servers[1].username.as_deref(), Some("alice"));

        // No relay configured leaves the list exactly as it was — host-only and
        // reflector-only deployments must not gain an empty entry, which would
        // throw at `RTCPeerConnection` construction.
        let none = resolve_webrtc_provisioning(Some("2KNode"), Some("ws://n:9000"), None)
            .expect("provisions")
            .with_relay(None);
        assert!(none.ice_servers.is_empty());
    }

    /// **A relay is not a reflector, and the list they share cannot be asked
    /// with `is_empty()`.**
    ///
    /// `reachability::classify` takes one bool for "was a reflector
    /// configured", and it is the whole difference between *"we asked and
    /// learned nothing"* (`ReflectorUnreachable` — go fix the URL) and *"we
    /// never asked"* (`NoReflector` — go add one). The install site read
    /// `!ice_servers.is_empty()`, which was exactly right until the relay field
    /// landed one commit later and made that list mixed: a relay-only session
    /// has one entry, no reflector, and was being told its **reflector** did not
    /// answer.
    ///
    /// Neither feature's own tests could see it — the classifier's predate the
    /// relay, and the relay gate asserts on `ice_servers` counts, never on a
    /// verdict. So it is pinned here, at the discriminator both of them share.
    #[test]
    fn a_relay_only_session_has_no_reflector() {
        let node = || resolve_webrtc_provisioning(Some("2KNode"), Some("ws://n:9000"), None);
        let relay = || parse_relay("turn:r.example:3478", "alice", "s3cret").unwrap();

        let relay_only = node().expect("provisions").with_relay(relay());
        assert_eq!(relay_only.ice_servers.len(), 1, "precondition: the list is non-empty");
        assert!(
            !relay_only.has_reflector(),
            "a relay-only session must not report a configured reflector — \
             that tells the user to fix a reflector they never configured"
        );
        assert!(relay_only.has_relay());

        // Host-only: neither half.
        let bare = node().expect("provisions");
        assert!(!bare.has_reflector() && !bare.has_relay());

        // Reflector-only: the arm that was always right, still right.
        let refl = resolve_webrtc_provisioning(
            Some("2KNode"),
            Some("ws://n:9000"),
            Some("stun:a.example:3478"),
        )
        .expect("provisions");
        assert!(refl.has_reflector() && !refl.has_relay());

        // Both: each is seen, and neither masks the other.
        let both = refl.clone().with_relay(relay());
        assert!(both.has_reflector() && both.has_relay());

        // The discriminator itself, stated once so the mirror and the install
        // site cannot drift apart again.
        assert!(relay().expect("a relay").is_relay());
        assert!(!refl.ice_servers[0].is_relay());
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

    /// **A percent-encoded node address must decode, and it did not.**
    ///
    /// The test above passes the address *unencoded* — legal, and what the e2e
    /// harness has always sent, which is exactly why this went unnoticed. The
    /// desktop's SPA server encodes it (a `ws://` URL in a query value must be,
    /// or a parser truncates it at the first `:`), and the raw value went
    /// straight through: the establisher installed with the literal string
    /// `ws%3A%2F%2F…` as its node address, `net` printed
    /// `OK rendezvous … at ws%3A%2F%2F…`, and nothing failed until a meet did.
    /// Measured in two real browsers against a real desktop, 2026-08-21.
    ///
    /// Both forms must work — the encoded one is now the shipped path and the
    /// unencoded one is every existing harness.
    #[test]
    fn a_percent_encoded_node_address_decodes() {
        let q = "?webrtc_node_peer=2KaNODE&webrtc_node=ws%3A%2F%2F192.168.68.55%3A4041";
        let p = webrtc_provisioning_from_query(q).expect("both halves present");
        assert_eq!(
            p.node_addr, "ws://192.168.68.55:4041",
            "an encoded address must reach the establisher decoded",
        );
        assert_eq!(p.node_peer_id, "2KaNODE");

        // A Base58 peer-id needs no escaping, so the common case must not be
        // disturbed by the decoder.
        let q = "?webrtc_node_peer=2KaNODE&webrtc_node=ws://plain:4041";
        assert_eq!(
            webrtc_provisioning_from_query(q).unwrap().node_addr,
            "ws://plain:4041",
        );
    }

    /// `+` is a literal here, not a space: this is a URL query, not an
    /// `application/x-www-form-urlencoded` body. Getting that wrong would
    /// silently corrupt any value containing one.
    #[test]
    fn percent_decoding_leaves_plus_and_malformed_escapes_alone() {
        assert_eq!(percent_decode("a+b"), "a+b");
        assert_eq!(percent_decode("100%"), "100%", "a trailing % is not an escape");
        assert_eq!(percent_decode("%zz"), "%zz", "non-hex is not an escape");
        assert_eq!(percent_decode("nothing-to-do"), "nothing-to-do");
        assert_eq!(percent_decode("%2F%2f"), "//", "either hex case");
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
