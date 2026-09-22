//! Per-domain deployment config (boot-closure cut 2b).
//!
//! The architecture for *"one tool, published to N CDN domains, each with a
//! different home site / posture / origins"* is **one generic WASM bundle plus
//! a small per-domain config file fetched at boot** — NOT a 3.6M WASM rebuild
//! per domain. At boot the SPA `GET`s the well-known [`DEPLOYMENT_CONFIG_PATH`] from
//! its own origin; if served, it shapes the cold-boot config.
//!
//! ## Precedence (highest wins)
//!
//! 1. URL overrides (`?site=`, `?boot_window=` — dev/showcase, never persisted).
//! 2. **Durable persisted session config** (a returning user's own settings).
//! 3. **This fetched config** (the per-domain deployment posture).
//! 4. **Build-time defaults** (`ENTITY_STARTUP_SURFACE` + `ENTITY_HOME_*` —
//!    cut 2a, the testing path).
//! 5. Hard default (chrome, local demo).
//!
//! So a fetched config only shapes a **cold** boot (no durable config yet);
//! a returning user's persisted config always wins ([`crate::app::EntityApp::boot_load`]
//! only fetches when the durable config is absent). The build-time env vars are
//! the *fallback under* the fetched config, not a competitor — a generic bundle
//! with empty `ENTITY_HOME_*` defers entirely to `/entity-deployment.json`.
//!
//! ## Honesty (D16)
//!
//! The fetch is read-only HTTP. Any failure — not served (404), unreachable,
//! unparseable — is a **silent fall-through** to the build-time defaults
//! ([`fetch`] returns `None`); it never blocks or fails boot. A default build
//! served without the file boots byte-identically to before this cut.
//!
//! The pure parse / apply / resolve logic is native-testable; only [`fetch`]
//! is wasm-only.

use std::collections::BTreeMap;

use crate::session_config::{
    boot_default, boot_surface_from, home_origin_default, home_site_default, BootSurface,
    SessionConfig, SiteRef,
};

/// Well-known origin path the SPA fetches at boot. Emitted by
/// `make site --deployment-config` next to the published content.
pub const DEPLOYMENT_CONFIG_PATH: &str = "/entity-deployment.json";

/// A partial site-mode posture override — only the fields the deployment
/// config actually specified. Merged onto the base config's posture in
/// [`DeploymentConfig::apply_to`] (absent field = inherit the base).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SiteModeOverride {
    pub enabled: Option<bool>,
    pub show_toggle: Option<bool>,
    pub locked: Option<bool>,
}

impl SiteModeOverride {
    fn is_empty(&self) -> bool {
        self.enabled.is_none() && self.show_toggle.is_none() && self.locked.is_none()
    }
}

/// The parsed per-domain deployment config. Every field is optional so a
/// partial config is valid (it overrides only what it names; the rest comes
/// from the build-time defaults below it in the precedence chain).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeploymentConfig {
    /// The startup **surface** (`chrome` / `site` / `window`) — the primary
    /// axis that replaced the old `profile` preset. Absent = inherit the base
    /// (build-time) surface. Paired with `window_type` when `surface == "window"`.
    pub surface: Option<String>,
    /// The window type to boot maximized when `surface == "window"` (e.g.
    /// `"Site Browser"`). Ignored for other surfaces.
    pub window_type: Option<String>,
    /// The startup site — where a `Site` boot lands / the home toggle opens.
    pub home_site: Option<SiteRef>,
    /// `target-peer-id → HTTP origin` — where each hosting peer's published
    /// artifacts live. Seeds the site-origin registry so the resolver
    /// HTTP-polls them. An empty origin string = same-origin (relative fetch),
    /// the common CDN case where the SPA and the static tree share a domain.
    pub origins: BTreeMap<String, String>,
    /// Partial overlay-posture override (merged onto the profile preset).
    pub site_mode: SiteModeOverride,
    /// Phase-1 fast-paint kill switch override.
    pub fast_paint: Option<bool>,
    /// Capability-posture override (MAP §10 item 1b): force peer creation
    /// on/off independent of the surface, so a `chrome` deployment can still
    /// disable creation without becoming a locked site. Merges onto the base
    /// config like `site_mode`.
    pub peer_creation_enabled: Option<bool>,
    /// **This resolver's own ceiling on a name binding's lifetime, in ms** —
    /// `EXTENSION-REGISTRY` §6a's resolver-side TTL bound (1.11).
    ///
    /// A deployment knob rather than a constant because arch writes no number
    /// and neither do we: there is no defensible one, and baking one in makes
    /// every unconfigured deployment *look* configured. Absent = no ceiling
    /// declared, which is conformant (§6a makes it a MAY, and a MUST only once
    /// declared) and is what we ship.
    ///
    /// It is the half that protects **us**: a ceiling the registry enforces
    /// cannot defend a consumer against that registry. Setting it shortens the
    /// window in which a withheld revocation still resolves across a cold start
    /// — the one bound our per-session `seq` floor cannot supply.
    pub name_resolver_max_ttl_ms: Option<u64>,
    /// **The registry this deployment seeds as a pin** — `EXTENSION-REGISTRY`
    /// §7.4's *"preloaded Entity System Registry"*, which is the whole distance
    /// between "we built a naming system" and "a user who types nothing can use
    /// it".
    ///
    /// `{"peer_id": "...", "origin": "..."}`. The peer-id is the trust decision
    /// and the only required half (canonical form embeds the public key); an
    /// empty/absent origin means same-origin, like the `origins` map. A pin with
    /// no peer-id is **dropped**, not completed from the origin: pinning an
    /// origin would trust the origin, which is the one thing this chain never
    /// does.
    pub name_registry_pin: Option<crate::session_config::RegistryPin>,
}

impl DeploymentConfig {
    /// Parse a deployment config from JSON. Tolerant: unknown keys are
    /// ignored, missing/empty fields stay `None`, and a `home_site` without a
    /// non-empty `site` id is dropped (the overlay always needs a site to point
    /// at). Returns `None` only when the document isn't a JSON object at all —
    /// the caller treats that as "no config" (D16 silent fall-through).
    pub fn parse(json: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(json).ok()?;
        let obj = value.as_object()?;
        let mut cfg = DeploymentConfig::default();

        // The surface axis (`chrome` / `site` / `window`) + its window type.
        // Kept as strings and validated at apply time (`boot_surface_from` is
        // garbage-tolerant → unknown falls back to Chrome), mirroring how every
        // other field here is tolerant of partial/unknown input.
        if let Some(s) = obj.get("surface").and_then(|v| v.as_str()) {
            let s = s.trim();
            if !s.is_empty() {
                cfg.surface = Some(s.to_string());
            }
        }
        if let Some(w) = obj.get("window_type").and_then(|v| v.as_str()) {
            let w = w.trim();
            if !w.is_empty() {
                cfg.window_type = Some(w.to_string());
            }
        }

        if let Some(hs) = obj.get("home_site").and_then(|v| v.as_object()) {
            let field = |k: &str| hs.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let site = field("site");
            if !site.is_empty() {
                cfg.home_site = Some(SiteRef {
                    peer_id: field("peer"),
                    id: site,
                    loc: field("loc"),
                });
            }
        }

        if let Some(origins) = obj.get("origins").and_then(|v| v.as_object()) {
            for (peer, origin) in origins {
                if let Some(o) = origin.as_str() {
                    cfg.origins.insert(peer.clone(), o.trim().to_string());
                }
            }
        }

        if let Some(sm) = obj.get("site_mode").and_then(|v| v.as_object()) {
            cfg.site_mode = SiteModeOverride {
                enabled: sm.get("enabled").and_then(|v| v.as_bool()),
                show_toggle: sm.get("show_toggle").and_then(|v| v.as_bool()),
                locked: sm.get("locked").and_then(|v| v.as_bool()),
            };
        }

        cfg.fast_paint = obj.get("fast_paint").and_then(|v| v.as_bool());
        cfg.peer_creation_enabled = obj.get("peer_creation_enabled").and_then(|v| v.as_bool());
        // A zero or negative ceiling is dropped rather than honored: it would
        // expire every binding instantly and read as "the registry is broken",
        // which is the same failure shape as emitting `ttl` in seconds.
        cfg.name_resolver_max_ttl_ms =
            obj.get("name_resolver_max_ttl_ms").and_then(|v| v.as_u64()).filter(|ms| *ms > 0);

        // The §7.4 preload. Tolerant like everything else here, and dropped
        // whole when the peer-id is missing — an origin alone is not a pin.
        if let Some(p) = obj.get("name_registry_pin").and_then(|v| v.as_object()) {
            let field = |k: &str| p.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let peer_id = field("peer_id");
            if !peer_id.is_empty() {
                cfg.name_registry_pin =
                    Some(crate::session_config::RegistryPin { peer_id, origin: field("origin") });
            }
        }

        Some(cfg)
    }

    /// Whether this config carries anything actionable. An object that parsed
    /// but named nothing we understand is treated as "no config."
    pub fn is_empty(&self) -> bool {
        self.surface.is_none()
            && self.window_type.is_none()
            && self.home_site.is_none()
            && self.origins.is_empty()
            && self.site_mode.is_empty()
            && self.fast_paint.is_none()
            && self.name_resolver_max_ttl_ms.is_none()
            && self.name_registry_pin.is_none()
            && self.peer_creation_enabled.is_none()
    }

    /// Apply this deployment config over a `base` session config (the build
    /// default), producing the cold-boot config. A named `surface` sets the boot
    /// surface (with `window_type` for `window`); a named `home_site` overrides
    /// the startup site; `site_mode` fields merge onto the base posture;
    /// `fast_paint` / `peer_creation_enabled` override their fields. Anything the
    /// config doesn't name is inherited from `base` — there is no preset bundling
    /// anymore, so a locked kiosk spells out `surface`+`site_mode`+
    /// `peer_creation_enabled` explicitly (what publish emits). The caller
    /// re-derives the runtime `active` flag from the resulting `boot_surface`.
    pub fn apply_to(&self, base: SessionConfig) -> SessionConfig {
        let mut cfg = base;
        // A named surface sets the boot surface directly; an unnamed one keeps
        // the build default's. The target peer is "" (system, resolved at boot)
        // — a per-domain config can't bake a runtime peer-id.
        if let Some(kind) = &self.surface {
            let wt = self.window_type.clone().unwrap_or_default();
            cfg.boot_surface = boot_surface_from(kind, "", &wt);
        }
        if let Some(home) = &self.home_site {
            cfg.home_site = home.clone();
        }
        if self.name_resolver_max_ttl_ms.is_some() {
            cfg.name_resolver_max_ttl_ms = self.name_resolver_max_ttl_ms;
        }
        if self.name_registry_pin.is_some() {
            cfg.name_registry_pin = self.name_registry_pin.clone();
        }
        if let Some(b) = self.site_mode.enabled {
            cfg.site_mode.enabled = b;
        }
        if let Some(b) = self.site_mode.show_toggle {
            cfg.site_mode.show_toggle = b;
        }
        if let Some(b) = self.site_mode.locked {
            cfg.site_mode.locked = b;
        }
        if let Some(fp) = self.fast_paint {
            cfg.fast_paint = fp;
        }
        if let Some(pce) = self.peer_creation_enabled {
            cfg.peer_creation_enabled = pce;
        }
        cfg
    }
}

// -- Shared resolution helpers (deployment config OVER build-time env) --------
//
// Used by both `boot_load` (post-peer config spine) and `boot_fast_paint`
// (pre-peer paint), so the precedence is defined once. Each takes an
// `Option<&DeploymentConfig>` (the fetch result) and falls back to the cut-2a
// build-time env defaults when the config is absent or silent on that field.

/// The effective startup site: the deployment config's `home_site`, else the
/// build-time `ENTITY_HOME_*` default (else the bundled local demo).
pub fn resolve_home_site(deployment: Option<&DeploymentConfig>) -> SiteRef {
    deployment
        .and_then(|d| d.home_site.clone())
        .unwrap_or_else(home_site_default)
}

/// The effective HTTP origin for `peer_id`: the deployment config's `origins`
/// entry, else the build-time `ENTITY_HOME_ORIGIN`. `None` = unknown (the home
/// won't resolve over HTTP unless its origin is registered elsewhere).
pub fn resolve_home_origin(deployment: Option<&DeploymentConfig>, peer_id: &str) -> Option<String> {
    deployment
        .and_then(|d| d.origins.get(peer_id).cloned())
        .or_else(home_origin_default)
}

/// Whether the effective deployment boots into the site overlay — the
/// deployment config's `surface`, else the build-time surface (`boot_default`).
/// This is what lets a **generic chrome bundle** fast-paint the site when the
/// per-domain config says `surface: "site"`, without a rebuild.
pub fn resolve_boots_into_site(deployment: Option<&DeploymentConfig>) -> bool {
    match deployment.and_then(|d| d.surface.as_deref()) {
        Some(kind) => boot_surface_from(kind, "", "") == BootSurface::Site,
        None => boot_default().active_from_boot_surface(),
    }
}

/// The SPA's own origin (`window.location.origin`), or `None` if unavailable.
/// The concrete value an empty / `self` configured origin expands to — so a
/// published config can say "this domain" without baking the domain in.
#[cfg(target_arch = "wasm32")]
pub fn same_origin() -> Option<String> {
    web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .filter(|s| !s.is_empty())
}

/// Expand a configured origin to a concrete fetch base. An empty string or the
/// literal `self` means "this SPA's own origin" — the portable same-origin CDN
/// case (`make site --deployment-config` emits `""` when no cross-origin
/// `--live` is given), expanded here to `window.location.origin` at runtime. A
/// **root-relative** origin (`/{prefix}`) is the same-origin case **with a
/// hosting prefix**: a peer published under `{PREFIX}` on this very domain — so
/// it expands to `{own-origin}/{prefix}` (`make site --prefix=… ` without
/// `--live`). Any other value (a concrete `https://…`, e.g. a cross-origin host
/// or `{https://host}/{prefix}`) passes through. `None` only if same-origin is
/// wanted but unavailable.
#[cfg(target_arch = "wasm32")]
pub fn expand_origin(origin: &str) -> Option<String> {
    match origin.trim() {
        "" | "self" => same_origin(),
        rel if rel.starts_with('/') => {
            same_origin().map(|o| format!("{}{}", o.trim_end_matches('/'), rel))
        }
        concrete => Some(concrete.to_string()),
    }
}

/// Fetch and parse the per-domain deployment config from the SPA's own origin.
/// Read-only, best-effort: any failure (not served, unreachable, unparseable,
/// or an empty/unrecognized doc) returns `None` and the build-time defaults
/// stand (D16 — never blocks or fails boot).
///
/// **Bounded (D23, C1).** This is called from `boot_load`, which is awaited
/// before the rAF loop starts and before the frozen-frame watchdog installs, so
/// an unbounded await here is a blank page with no watchdog and no exit — not a
/// slow boot. It used to be a bare `window.fetch_with_str`, which *rejects*
/// promptly on a refused connection (why offline boot mostly worked) and
/// *never returns* on an origin that accepts and never answers. Both call sites
/// — the cold-boot `durable.is_none()` read and the warm-boot routing reconcile
/// — go through this one function, so this deadline covers the whole boot path;
/// the enumeration behind that claim is design §4A, and its falsifier is G1
/// (`boot_survives_a_blackholed_deployment_config`), which was observed red
/// before this line changed.
///
/// A timeout is deliberately indistinguishable from "not served": both mean the
/// build-time defaults stand for this boot, which is the behaviour D16 already
/// promises and every caller already handles.
#[cfg(target_arch = "wasm32")]
pub async fn fetch() -> Option<DeploymentConfig> {
    let resp = match crate::net::fetch_text_bounded(
        DEPLOYMENT_CONFIG_PATH,
        crate::net::BOOT_FETCH_DEADLINE_MS,
    )
    .await
    {
        Some(r) => r,
        None => {
            // Unreachable, aborted at the deadline, or unreadable. Logged at
            // WARN rather than DEBUG because on a healthy origin it does not
            // happen, and when it does it is the single most useful line in a
            // "the page was blank" report.
            tracing::warn!(
                "deployment-config: {DEPLOYMENT_CONFIG_PATH} unreachable or timed out after \
                 {}ms — booting on build-time defaults (D23: a deadline is a state the boot \
                 proceeds from)",
                crate::net::BOOT_FETCH_DEADLINE_MS
            );
            return None;
        }
    };
    if !resp.ok {
        tracing::debug!(
            status = resp.status,
            "deployment-config: {DEPLOYMENT_CONFIG_PATH} not served — using build-time defaults"
        );
        return None;
    }
    let text = resp.text;
    match DeploymentConfig::parse(&text) {
        Some(cfg) if !cfg.is_empty() => {
            tracing::info!(
                surface = ?cfg.surface.as_deref(),
                home_site = ?cfg.home_site.as_ref().map(|h| h.id.as_str()),
                origins = cfg.origins.len(),
                "deployment-config: applied {DEPLOYMENT_CONFIG_PATH}"
            );
            Some(cfg)
        }
        Some(_) => {
            tracing::debug!("deployment-config: served but empty — using build-time defaults");
            None
        }
        None => {
            tracing::warn!("deployment-config: served but unparseable — ignoring");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_config::{BootSurface, DEMO_SITE_ID};

    #[test]
    fn parse_full_config() {
        // A locked-kiosk config now spells out surface + site_mode +
        // peer_creation_enabled explicitly (no preset bundling).
        let json = r#"{
            "surface": "site",
            "home_site": { "peer": "labs-peer", "site": "labs", "loc": "intro" },
            "origins": { "labs-peer": "https://labs.example" },
            "site_mode": { "enabled": true, "show_toggle": false, "locked": true },
            "peer_creation_enabled": false,
            "fast_paint": false
        }"#;
        let cfg = DeploymentConfig::parse(json).unwrap();
        assert_eq!(cfg.surface.as_deref(), Some("site"));
        assert_eq!(
            cfg.home_site,
            Some(SiteRef { peer_id: "labs-peer".into(), id: "labs".into(), loc: "intro".into() })
        );
        assert_eq!(cfg.origins.get("labs-peer").map(String::as_str), Some("https://labs.example"));
        assert_eq!(cfg.site_mode.show_toggle, Some(false));
        assert_eq!(cfg.site_mode.locked, Some(true));
        assert_eq!(cfg.peer_creation_enabled, Some(false));
        assert_eq!(cfg.fast_paint, Some(false));
    }

    #[test]
    fn parse_window_surface_carries_window_type() {
        let json = r#"{ "surface": "window", "window_type": "Site Browser" }"#;
        let cfg = DeploymentConfig::parse(json).unwrap();
        assert_eq!(cfg.surface.as_deref(), Some("window"));
        assert_eq!(cfg.window_type.as_deref(), Some("Site Browser"));
    }

    #[test]
    fn parse_is_tolerant_of_partial_and_unknown() {
        // Only a surface; unknown keys ignored; empty home_site site dropped.
        let json = r#"{ "surface": "window", "mystery": 7, "home_site": { "peer": "p" } }"#;
        let cfg = DeploymentConfig::parse(json).unwrap();
        assert_eq!(cfg.surface.as_deref(), Some("window"));
        assert!(cfg.home_site.is_none(), "home_site with no site id is dropped");
        assert!(cfg.origins.is_empty());
        assert!(cfg.site_mode.is_empty());
    }

    #[test]
    fn parse_rejects_non_object() {
        assert!(DeploymentConfig::parse("not json").is_none());
        assert!(DeploymentConfig::parse("[1,2,3]").is_none());
        assert!(DeploymentConfig::parse("42").is_none());
        // A valid-but-empty object parses to an `is_empty` config.
        assert!(DeploymentConfig::parse("{}").unwrap().is_empty());
    }

    /// The §6a resolver ceiling is a **deployment** knob, and a nonsense value
    /// is dropped rather than honored.
    ///
    /// Zero is the trap worth a test: honored literally it expires every
    /// binding the instant it resolves, and the operator sees "no binding for
    /// this name" — indistinguishable from a bad signature, a revocation, or a
    /// broken registry. Exactly the failure shape as emitting `ttl` in seconds
    /// instead of ms, which is how we learned to look for it.
    #[test]
    fn the_resolver_ceiling_is_read_from_the_deployment_and_zero_is_dropped() {
        let cfg = DeploymentConfig::parse(r#"{"name_resolver_max_ttl_ms": 3600000}"#).unwrap();
        assert_eq!(cfg.name_resolver_max_ttl_ms, Some(3_600_000));
        assert!(!cfg.is_empty(), "a ceiling alone is actionable config");

        // Applied onto a base config, it reaches the durable spine — which is
        // what a warm boot reads, since a returning profile never re-fetches
        // the deployment doc.
        let applied = cfg.apply_to(SessionConfig::default());
        assert_eq!(applied.name_resolver_max_ttl_ms, Some(3_600_000));

        for bad in [r#"{"name_resolver_max_ttl_ms": 0}"#, r#"{"name_resolver_max_ttl_ms": -5}"#] {
            let cfg = DeploymentConfig::parse(bad).unwrap();
            assert_eq!(cfg.name_resolver_max_ttl_ms, None, "dropped: {bad}");
        }

        // Absent = no ceiling declared, which is conformant and is what we ship.
        let cfg = DeploymentConfig::parse(r#"{"surface": "site"}"#).unwrap();
        assert_eq!(cfg.name_resolver_max_ttl_ms, None);
        assert_eq!(cfg.apply_to(SessionConfig::default()).name_resolver_max_ttl_ms, None);
    }

    /// **The §7.4 preload reaches the durable spine, and an origin alone is not
    /// a pin.**
    ///
    /// The `apply_to` half is the one that matters and is easy to skip: a pin
    /// that lived only in the fetched document would seed a cold boot and vanish
    /// on every warm one, because a returning profile never re-fetches
    /// `/entity-deployment.json` (D16) — a default that works exactly once, in
    /// the direction nobody tests [AP22].
    #[test]
    fn the_registry_pin_is_read_from_the_deployment_and_reaches_the_durable_config() {
        let json = r#"{
            "surface": "site",
            "name_registry_pin": { "peer_id": "2KRegistryPeer", "origin": "https://reg.example" }
        }"#;
        let cfg = DeploymentConfig::parse(json).unwrap();
        let pin = cfg.name_registry_pin.clone().expect("the pin parses");
        assert_eq!(pin.peer_id, "2KRegistryPeer");
        assert_eq!(pin.origin, "https://reg.example");

        let applied = cfg.apply_to(SessionConfig::default());
        assert_eq!(applied.name_registry_pin, Some(pin), "the pin must ride the durable config");

        // An origin with no peer-id is dropped WHOLE rather than completed from
        // the origin: a pin is a key, and pinning an origin would trust the
        // origin — the one thing neither hop of this chain ever does.
        let orphan =
            DeploymentConfig::parse(r#"{"name_registry_pin": {"origin": "https://reg.example"}}"#)
                .unwrap();
        assert_eq!(orphan.name_registry_pin, None);
        assert!(orphan.is_empty(), "a pin with no peer-id is not actionable config");

        // A pin alone IS actionable — it is the whole §7.4 deliverable.
        let alone =
            DeploymentConfig::parse(r#"{"name_registry_pin": {"peer_id": "2KOnlyThis"}}"#).unwrap();
        assert!(!alone.is_empty());
        assert_eq!(
            alone.name_registry_pin.unwrap().origin,
            "",
            "an absent origin means same-origin, like the origins map"
        );

        // Absent = no pin, which is what a generic build ships: fail-closed.
        let none = DeploymentConfig::parse(r#"{"surface": "site"}"#).unwrap();
        assert_eq!(none.name_registry_pin, None);
        assert_eq!(none.apply_to(SessionConfig::default()).name_registry_pin, None);
    }

    #[test]
    fn apply_surface_sets_boot_surface_and_site_mode_merges() {
        // A locked-site config over a chrome build default = boots-into-site,
        // locked (the generic-bundle-on-a-locked-domain case). site_mode is now
        // explicit (no preset), and it merges onto the base — an unnamed field
        // (`enabled`) stays the base's `true`.
        let cfg = DeploymentConfig {
            surface: Some("site".into()),
            site_mode: SiteModeOverride { show_toggle: Some(false), locked: Some(true), ..Default::default() },
            ..Default::default()
        };
        let out = cfg.apply_to(SessionConfig::default());
        assert_eq!(out.boot_surface, BootSurface::Site);
        assert!(!out.site_mode.show_toggle, "explicit override");
        assert!(out.site_mode.locked, "explicit override");
        assert!(out.site_mode.enabled, "unnamed field inherited from base");
    }

    #[test]
    fn apply_window_surface_carries_type() {
        let cfg = DeploymentConfig {
            surface: Some("window".into()),
            window_type: Some("Site Browser".into()),
            ..Default::default()
        };
        let out = cfg.apply_to(SessionConfig::default());
        assert_eq!(
            out.boot_surface,
            BootSurface::Window { peer_id: String::new(), window_type: "Site Browser".into() }
        );
    }

    #[test]
    fn peer_creation_override_merges_independent_of_surface() {
        // A `chrome` deployment that still disables peer creation — capability is
        // orthogonal to surface (MAP §5).
        let json = r#"{ "surface": "chrome", "peer_creation_enabled": false }"#;
        let cfg = DeploymentConfig::parse(json).unwrap();
        assert_eq!(cfg.peer_creation_enabled, Some(false));
        let out = cfg.apply_to(SessionConfig::default());
        assert_eq!(out.boot_surface, BootSurface::Chrome, "still chrome-first");
        assert!(!out.peer_creation_enabled, "creation disabled by override");

        // Absent override → inherits the base's `true` (no preset to seed false).
        let site = DeploymentConfig { surface: Some("site".into()), ..Default::default() };
        assert!(site.apply_to(SessionConfig::default()).peer_creation_enabled);
    }

    #[test]
    fn apply_without_surface_keeps_base_posture() {
        // No surface → base surface untouched; only home_site overridden.
        let cfg = DeploymentConfig {
            home_site: Some(SiteRef { peer_id: "h".into(), id: "labs".into(), loc: String::new() }),
            ..Default::default()
        };
        let base = SessionConfig::default(); // chrome-first
        let out = cfg.apply_to(base.clone());
        assert_eq!(out.boot_surface, base.boot_surface, "surface inherited from base");
        assert_eq!(out.home_site.id, "labs");
    }

    #[test]
    fn resolve_home_site_prefers_deployment() {
        let dc = DeploymentConfig {
            home_site: Some(SiteRef { peer_id: "h".into(), id: "labs".into(), loc: String::new() }),
            ..Default::default()
        };
        assert_eq!(resolve_home_site(Some(&dc)).id, "labs");
        // No config → build-time default (demo on a default build).
        assert_eq!(resolve_home_site(None).id, DEMO_SITE_ID);
    }

    #[test]
    fn resolve_home_origin_prefers_deployment_then_env() {
        let mut dc = DeploymentConfig::default();
        dc.origins.insert("h".into(), "https://h.example".into());
        assert_eq!(resolve_home_origin(Some(&dc), "h").as_deref(), Some("https://h.example"));
        // A peer the config doesn't list falls to the env default (None on a
        // default build with ENTITY_HOME_ORIGIN unset).
        assert_eq!(resolve_home_origin(Some(&dc), "other"), home_origin_default());
        assert_eq!(resolve_home_origin(None, "h"), home_origin_default());
    }

    #[test]
    fn parse_and_resolve_multi_peer_origins() {
        // A deployment that federates content from MORE than one hosting peer:
        // the config carries several `origins`, and `resolve_home_origin` picks
        // the right one per target peer (boot_load registers them all). The home
        // is on one peer; another peer's site cross-links in from a third origin.
        let json = r#"{
            "surface": "site",
            "home_site": { "peer": "peer-a", "site": "labs", "loc": "" },
            "origins": {
                "peer-a": "https://a.example",
                "peer-b": "https://b.example",
                "peer-c": ""
            }
        }"#;
        let cfg = DeploymentConfig::parse(json).unwrap();
        assert_eq!(cfg.origins.len(), 3, "all three peer origins parsed");
        assert_eq!(resolve_home_origin(Some(&cfg), "peer-a").as_deref(), Some("https://a.example"));
        assert_eq!(resolve_home_origin(Some(&cfg), "peer-b").as_deref(), Some("https://b.example"));
        // A same-origin ("") entry resolves to "" here (the wasm `expand_origin`
        // turns it into window.location.origin at the call site); the key fact is
        // it's present and distinct per peer.
        assert_eq!(resolve_home_origin(Some(&cfg), "peer-c").as_deref(), Some(""));
        // A peer the config doesn't name falls back to the env default.
        assert_eq!(resolve_home_origin(Some(&cfg), "peer-z"), home_origin_default());
    }

    #[test]
    fn resolve_home_origin_passes_through_a_base_path_origin() {
        // The "change-prefix" / non-root deployment: when the SPA is served under
        // a sub-path (e.g. https://host/app/) the same-origin "" shortcut can't be
        // used (window.location.origin drops the path), so the config gives an
        // explicit base-path origin. It must pass through verbatim so the resolver
        // builds `{base}/{peer}/sites/...` under the sub-path.
        let mut dc = DeploymentConfig::default();
        dc.origins.insert("peer-a".into(), "https://host.example/app".into());
        assert_eq!(
            resolve_home_origin(Some(&dc), "peer-a").as_deref(),
            Some("https://host.example/app"),
            "base-path origin is not rewritten — subpath deployments resolve under it"
        );
    }

    #[test]
    fn resolve_boots_into_site_from_surface() {
        let site = DeploymentConfig { surface: Some("site".into()), ..Default::default() };
        assert!(resolve_boots_into_site(Some(&site)), "surface=site boots into the site");
        let chrome = DeploymentConfig { surface: Some("chrome".into()), ..Default::default() };
        assert!(!resolve_boots_into_site(Some(&chrome)), "chrome is not the overlay");
        let window = DeploymentConfig { surface: Some("window".into()), ..Default::default() };
        assert!(!resolve_boots_into_site(Some(&window)), "window is a non-overlay surface");
        // No config → the build default's posture (chrome on a default build).
        assert_eq!(resolve_boots_into_site(None), boot_default().active_from_boot_surface());
    }
}
