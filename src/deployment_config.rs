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
//! 4. **Build-time defaults** (`ENTITY_PROFILE` + `ENTITY_HOME_*` — cut 2a,
//!    the testing path).
//! 5. Hard default (`Full`, local demo).
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
    boot_default, home_origin_default, home_site_default, Profile, SessionConfig, SiteRef,
};

/// Well-known origin path the SPA fetches at boot. Emitted by
/// `make publish --deployment-config` next to the published content.
pub const DEPLOYMENT_CONFIG_PATH: &str = "/entity-deployment.json";

/// A partial site-mode posture override — only the fields the deployment
/// config actually specified. Merged onto the profile preset's posture in
/// [`DeploymentConfig::apply_to`] (absent field = inherit the preset).
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
    /// Deployment posture preset (`full` / `tutorial` / `strict-site`).
    pub profile: Option<Profile>,
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
    /// on/off independent of the profile, so a `full` (chrome) deployment can
    /// still disable creation without becoming a locked site. Merges onto the
    /// profile preset like `site_mode`.
    pub peer_creation_enabled: Option<bool>,
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

        if let Some(p) = obj.get("profile").and_then(|v| v.as_str()) {
            cfg.profile = Profile::from_str(p.trim());
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

        Some(cfg)
    }

    /// Whether this config carries anything actionable. An object that parsed
    /// but named nothing we understand is treated as "no config."
    pub fn is_empty(&self) -> bool {
        self.profile.is_none()
            && self.home_site.is_none()
            && self.origins.is_empty()
            && self.site_mode.is_empty()
            && self.fast_paint.is_none()
            && self.peer_creation_enabled.is_none()
    }

    /// Apply this deployment config over a `base` session config (the build
    /// default), producing the cold-boot config. A named `profile` swaps in its
    /// **posture** preset (boot surface + overlay posture); a named `home_site`
    /// overrides the startup site; `site_mode` fields merge onto the posture;
    /// `fast_paint` overrides the kill switch. Anything the config doesn't name
    /// is inherited from `base`. The caller re-derives the runtime `active`
    /// flag from the resulting `boot_surface`, so we leave it.
    pub fn apply_to(&self, base: SessionConfig) -> SessionConfig {
        // A named profile re-seeds posture (and its env-default home); an
        // unnamed one keeps the build default's posture/home.
        let mut cfg = match self.profile {
            Some(p) => p.preset(),
            None => base,
        };
        if let Some(home) = &self.home_site {
            cfg.home_site = home.clone();
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
/// deployment config's profile posture, else the build-time profile's
/// (`boot_default`). This is what lets a **generic `Full` bundle** fast-paint
/// the site when the per-domain config says `strict-site`/`tutorial`, without a
/// rebuild.
pub fn resolve_boots_into_site(deployment: Option<&DeploymentConfig>) -> bool {
    match deployment.and_then(|d| d.profile) {
        Some(p) => p.preset().active_from_boot_surface(),
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
/// case (`make publish --deployment-config` emits `""` when no cross-origin
/// `--live` is given), expanded here to `window.location.origin` at runtime. A
/// **root-relative** origin (`/{prefix}`) is the same-origin case **with a
/// hosting prefix**: a peer published under `{PREFIX}` on this very domain — so
/// it expands to `{own-origin}/{prefix}` (`make publish --prefix=… ` without
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
#[cfg(target_arch = "wasm32")]
pub async fn fetch() -> Option<DeploymentConfig> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let window = web_sys::window()?;
    let resp_val = JsFuture::from(window.fetch_with_str(DEPLOYMENT_CONFIG_PATH))
        .await
        .ok()?;
    let resp: web_sys::Response = resp_val.dyn_into().ok()?;
    if !resp.ok() {
        tracing::debug!(
            status = resp.status(),
            "deployment-config: {DEPLOYMENT_CONFIG_PATH} not served — using build-time defaults"
        );
        return None;
    }
    let text_promise = resp.text().ok()?;
    let text = JsFuture::from(text_promise).await.ok()?.as_string()?;
    match DeploymentConfig::parse(&text) {
        Some(cfg) if !cfg.is_empty() => {
            tracing::info!(
                profile = ?cfg.profile.map(|p| p.as_str()),
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
        let json = r#"{
            "profile": "strict-site",
            "home_site": { "peer": "labs-peer", "site": "labs", "loc": "intro" },
            "origins": { "labs-peer": "https://labs.example" },
            "site_mode": { "enabled": true, "show_toggle": false, "locked": true },
            "fast_paint": false
        }"#;
        let cfg = DeploymentConfig::parse(json).unwrap();
        assert_eq!(cfg.profile, Some(Profile::StrictSite));
        assert_eq!(
            cfg.home_site,
            Some(SiteRef { peer_id: "labs-peer".into(), id: "labs".into(), loc: "intro".into() })
        );
        assert_eq!(cfg.origins.get("labs-peer").map(String::as_str), Some("https://labs.example"));
        assert_eq!(cfg.site_mode.show_toggle, Some(false));
        assert_eq!(cfg.site_mode.locked, Some(true));
        assert_eq!(cfg.fast_paint, Some(false));
    }

    #[test]
    fn parse_is_tolerant_of_partial_and_unknown() {
        // Only a profile; unknown keys ignored; empty home_site site dropped.
        let json = r#"{ "profile": "tutorial", "mystery": 7, "home_site": { "peer": "p" } }"#;
        let cfg = DeploymentConfig::parse(json).unwrap();
        assert_eq!(cfg.profile, Some(Profile::Tutorial));
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

    #[test]
    fn apply_profile_swaps_posture() {
        // A strict-site deployment config over a Full build default = locked,
        // boots-into-site posture (the generic-bundle-on-a-strict-domain case).
        let cfg = DeploymentConfig {
            profile: Some(Profile::StrictSite),
            ..Default::default()
        };
        let out = cfg.apply_to(SessionConfig::default());
        assert_eq!(out.boot_surface, BootSurface::Site);
        assert!(!out.site_mode.show_toggle);
        assert!(out.site_mode.locked);
    }

    #[test]
    fn apply_overrides_home_and_site_mode_merge() {
        // Profile sets the posture; site_mode names ONE field (show_toggle) and
        // must merge, not replace — `locked` stays the strict-site preset's.
        let cfg = DeploymentConfig {
            profile: Some(Profile::StrictSite),
            home_site: Some(SiteRef { peer_id: "h".into(), id: "labs".into(), loc: String::new() }),
            site_mode: SiteModeOverride { show_toggle: Some(true), ..Default::default() },
            ..Default::default()
        };
        let out = cfg.apply_to(SessionConfig::default());
        assert_eq!(out.home_site.id, "labs");
        assert_eq!(out.home_site.peer_id, "h");
        assert!(out.site_mode.show_toggle, "merged override");
        assert!(out.site_mode.locked, "untouched preset field preserved");
    }

    #[test]
    fn peer_creation_override_merges_independent_of_profile() {
        // A `full` (chrome) deployment that still disables peer creation —
        // capability is orthogonal to surface (MAP §5).
        let json = r#"{ "profile": "full", "peer_creation_enabled": false }"#;
        let cfg = DeploymentConfig::parse(json).unwrap();
        assert_eq!(cfg.peer_creation_enabled, Some(false));
        let out = cfg.apply_to(SessionConfig::default());
        assert_eq!(out.boot_surface, BootSurface::Chrome, "still chrome-first");
        assert!(!out.peer_creation_enabled, "creation disabled by override");

        // Absent override on a strict-site profile inherits the preset's `false`.
        let strict = DeploymentConfig { profile: Some(Profile::StrictSite), ..Default::default() };
        assert!(!strict.apply_to(SessionConfig::default()).peer_creation_enabled);

        // And an explicit `true` override can RE-ENABLE creation on a strict
        // profile (operator opt-in).
        let relaxed = DeploymentConfig {
            profile: Some(Profile::StrictSite),
            peer_creation_enabled: Some(true),
            ..Default::default()
        };
        assert!(relaxed.apply_to(SessionConfig::default()).peer_creation_enabled);
    }

    #[test]
    fn apply_without_profile_keeps_base_posture() {
        // No profile → base posture untouched; only home_site overridden.
        let cfg = DeploymentConfig {
            home_site: Some(SiteRef { peer_id: "h".into(), id: "labs".into(), loc: String::new() }),
            ..Default::default()
        };
        let base = SessionConfig::default(); // Full → Chrome
        let out = cfg.apply_to(base.clone());
        assert_eq!(out.boot_surface, base.boot_surface, "posture inherited from base");
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
            "profile": "strict-site",
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
    fn resolve_boots_into_site_from_profile() {
        let strict = DeploymentConfig { profile: Some(Profile::StrictSite), ..Default::default() };
        assert!(resolve_boots_into_site(Some(&strict)), "strict-site boots into the site");
        let full = DeploymentConfig { profile: Some(Profile::Full), ..Default::default() };
        assert!(!resolve_boots_into_site(Some(&full)), "full is chrome-first");
        // No config → the build default's posture (Full on a default build).
        assert_eq!(resolve_boots_into_site(None), boot_default().active_from_boot_surface());
    }
}
