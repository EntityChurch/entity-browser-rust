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
            // **Stamp the provenance here, not at the call sites.** This is the
            // deployment's own writer — every path by which a document reaches
            // a `SessionConfig` goes through it — so the mark cannot be
            // forgotten by a caller added later (AP44). Its twin is
            // `session_config::set_home_site`, which stamps `User`.
            cfg.home_site_source = crate::session_config::HomeSource::Deployment;
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

/// **What the origin said when we asked for its deployment document.**
///
/// The read used to collapse four different things into one `None`: a 404, a
/// document that parsed but declared nothing, bytes we could not read, and
/// hearing nothing at all inside D23's deadline. **Only the first two are facts
/// about the deployment**; the last is a fact about the network and the third is
/// a fact about some bytes. Flattening them is the same conflation that runs
/// through this whole audit — `put_if_absent` could not tell *the user set this*
/// from *we wrote it last boot*, and a presence check could not tell *I have a
/// current copy* from *I have a copy* (D24). Here it means the line an incident
/// gets debugged from cannot say which happened.
///
/// **Note what this deliberately does NOT do.** The dossier that asked for this
/// also proposed *"a 404, once recorded, does not re-probe every boot."* That is
/// rejected, and stating why is the point of writing it down: a durable record
/// that the origin has no document, written because the origin said so once and
/// never re-examined, is **AP30 exactly** — and it would re-create the wedge one
/// layer up, because a deployment that ADDS `/entity-deployment.json` later
/// would never reach a returning profile. It would also undo map-B2, which
/// landed one commit ago specifically to make every warm boot re-read. The
/// efficiency it was reaching for is already delivered by boot's
/// `!config_was_absent` guard, which stops the *second* read within one boot —
/// the only re-probe that was ever real.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentRead {
    /// The origin served a document that declares something we can use.
    Served(DeploymentConfig),
    /// **The origin answered, and it has no deployment document.** A **404 or
    /// 410**, or a document that parsed and declared nothing actionable. A
    /// *fact about the deployment*: it is generic-bundle-on-build-time-defaults
    /// by choice.
    ///
    /// **Only 404/410, and this is the line the first version of this enum got
    /// wrong** — it mapped every non-2xx here, so a 502 from a CDN reported as
    /// *"this deployment runs on build-time defaults by choice"*, which is a
    /// false claim about the deployer's intent and the wrong instruction to an
    /// operator. That is the exact conflation B3 exists to remove, one status
    /// family over. `PollError::NotFound` states the same rule for the content
    /// path and states it first; this is now consistent with it.
    NoDocument { status: u16 },
    /// **The origin answered with a failure** — a 403, a 5xx, a proxy error. It
    /// answered, so this is not [`Unheard`](Self::Unheard); but a server fault
    /// says *nothing* about whether a document exists, so it is not
    /// [`NoDocument`](Self::NoDocument) either. Retryable, like every origin
    /// fault that is not a deliberate 404.
    OriginError { status: u16 },
    /// The origin answered with bytes we could not read. A fact about the
    /// **bytes** — a truncated or half-written file, a proxy error page served
    /// with a 200 — never a fact about whether a document exists.
    Unreadable { status: u16 },
    /// **Nothing was heard**: unreachable, aborted, or D23's deadline expired.
    /// A fact about nothing at all, and the one outcome from which no durable
    /// conclusion may ever be drawn (AP30 corollary (a)).
    Unheard,
}

impl DocumentRead {
    /// The usable config, if there is one. The shape every existing caller
    /// wants; the distinction above is for the caller that needs to *report*.
    pub fn into_config(self) -> Option<DeploymentConfig> {
        match self {
            Self::Served(cfg) => Some(cfg),
            _ => None,
        }
    }

    /// The same thing without consuming the read — for a caller that needs both
    /// the config *and* the outcome it arrived by, which is every caller that
    /// reports rather than acts.
    pub fn config(&self) -> Option<&DeploymentConfig> {
        match self {
            Self::Served(cfg) => Some(cfg),
            _ => None,
        }
    }

    /// A short stable tag for a structured log field — `served` / `not-served` /
    /// `unreadable` / `unheard`.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Served(_) => "served",
            Self::NoDocument { .. } => "not-served",
            Self::OriginError { .. } => "origin-error",
            Self::Unreadable { .. } => "unreadable",
            Self::Unheard => "unheard",
        }
    }

    /// **Did the origin answer at all?** Literally: was there an HTTP response.
    /// True for everything but [`Unheard`](Self::Unheard).
    pub fn origin_answered(&self) -> bool {
        !matches!(self, Self::Unheard)
    }

    /// **Is this a statement that the deployment has no document?**
    ///
    /// The *only* fact a caller may ever act on or write down, and it is
    /// narrower than "the origin answered": a 502 answered, and said nothing
    /// about whether a document exists. True for [`NoDocument`](Self::NoDocument)
    /// alone.
    ///
    /// Even then — see the module note — this may be **reported**, never
    /// cached: a deployment that adds `/entity-deployment.json` later must
    /// reach a returning profile (AP30).
    pub fn declares_no_document(&self) -> bool {
        matches!(self, Self::NoDocument { .. })
    }

    /// The sentence a human debugging an incident reads. **Each outcome says
    /// something different**, and that is the deliverable: the previous single
    /// `None` produced one line for a domain that ships no config on purpose and
    /// for a domain that could not be reached, which are opposite problems.
    pub fn describe(&self) -> String {
        match self {
            Self::Served(_) => format!("{DEPLOYMENT_CONFIG_PATH} applied"),
            Self::NoDocument { status } => format!(
                "the origin answered {status}: it serves no {DEPLOYMENT_CONFIG_PATH} — this \
                 deployment runs on build-time defaults by choice"
            ),
            Self::OriginError { status } => format!(
                "the origin answered {status} for {DEPLOYMENT_CONFIG_PATH} — a server fault, \
                 which says NOTHING about whether this deployment has a document"
            ),
            Self::Unreadable { status } => format!(
                "the origin answered {status} with a {DEPLOYMENT_CONFIG_PATH} we could not \
                 read — the file is malformed, not absent"
            ),
            Self::Unheard => format!(
                "no answer for {DEPLOYMENT_CONFIG_PATH} within the boot deadline — \
                 unreachable or too slow, and NOT evidence that the origin has none"
            ),
        }
    }
}

/// Classify a bounded read of the deployment document. **Pure** — the whole
/// point of the split, so every outcome has a native test rather than only the
/// one a browser happened to produce.
///
/// `None` in means the bounded read gave us nothing: unreachable, aborted, or
/// the deadline. That is [`DocumentRead::Unheard`] and nothing else.
pub fn classify(resp: Option<crate::net::BoundedResponse>) -> DocumentRead {
    let Some(resp) = resp else {
        return DocumentRead::Unheard;
    };
    if !resp.ok {
        // **Only 404/410 mean "there is none".** Everything else the origin can
        // answer with — 403, 500, 502, a proxy page — is a fault that says
        // nothing about whether a document exists, and calling it a deliberate
        // absence turns a transient origin problem into a claim about the
        // deployer's intent. Same rule, same reason, and the same wording as
        // `PollError::NotFound` on the content path.
        return match resp.status {
            404 | 410 => DocumentRead::NoDocument { status: resp.status },
            status => DocumentRead::OriginError { status },
        };
    }
    match DeploymentConfig::parse(&resp.text) {
        // Parsed and declares something we act on.
        Some(cfg) if !cfg.is_empty() => DocumentRead::Served(cfg),
        // Parsed and declares nothing. The origin ANSWERED and has, in every
        // sense that matters to a boot, no deployment document — same outcome as
        // a 404 and the same fact about the deployment.
        Some(_) => DocumentRead::NoDocument { status: resp.status },
        // Not a JSON object at all.
        None => DocumentRead::Unreadable { status: resp.status },
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
    read_document().await.into_config()
}

/// The three-state read: ask the origin, classify what it said, and **say which
/// of the four things happened** before handing back the answer.
///
/// [`fetch`] is the thin `Option` wrapper over this for the callers that only
/// want the config. A caller that needs to *report* — the boot log, a future
/// operator surface — takes the [`DocumentRead`] and keeps the distinction.
#[cfg(target_arch = "wasm32")]
pub async fn read_document() -> DocumentRead {
    let out = classify(
        crate::net::fetch_text_bounded(
            DEPLOYMENT_CONFIG_PATH,
            crate::net::BOOT_FETCH_DEADLINE_MS,
        )
        .await,
    );
    // The levels differ because the audiences do. `Unheard` is WARN: on a
    // healthy origin it does not happen, and when it does it is the single most
    // useful line in a "the page was blank" report. `NoDocument` is DEBUG
    // because it is the normal state of a build served without the file — a
    // WARN there would be the same cried-wolf failure C1 just fixed one surface
    // over. `Unreadable` is WARN because somebody published a broken file.
    match &out {
        DocumentRead::Served(cfg) => tracing::info!(
            outcome = out.label(),
            surface = ?cfg.surface.as_deref(),
            home_site = ?cfg.home_site.as_ref().map(|h| h.id.as_str()),
            origins = cfg.origins.len(),
            "deployment-config: applied {DEPLOYMENT_CONFIG_PATH}"
        ),
        DocumentRead::NoDocument { .. } => tracing::debug!(
            outcome = out.label(),
            "deployment-config: {} — using build-time defaults",
            out.describe()
        ),
        DocumentRead::OriginError { .. } => tracing::warn!(
            outcome = out.label(),
            "deployment-config: {}",
            out.describe()
        ),
        DocumentRead::Unreadable { .. } => tracing::warn!(
            outcome = out.label(),
            "deployment-config: served but unparseable — {}",
            out.describe()
        ),
        DocumentRead::Unheard => tracing::warn!(
            outcome = out.label(),
            deadline_ms = crate::net::BOOT_FETCH_DEADLINE_MS,
            "deployment-config: {} (D23: a deadline is a state the boot proceeds from)",
            out.describe()
        ),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── The provenance census — AP49's enforcement point ─────────────────────
    //
    // **Every field a deployment can declare gets classified, and the count is
    // asserted**, so adding one to this document forces the question that was
    // never asked about `home_site`: *if the end user has also set this, whose
    // value wins on the next boot, and how does the code tell them apart?*
    //
    // This is a census rather than a structural guarantee because the answer is
    // genuinely per-field — and a census you have not falsified reports what you
    // hoped, so `Owned::User` is only claimed where a mechanism exists and can
    // be named.

    /// Who owns a field once the end user has also expressed a preference about
    /// it, and by what mechanism the code can tell.
    #[derive(Debug, PartialEq, Eq)]
    enum Owned {
        /// The deployer's, always. The end user has no way to set it, so there
        /// is nothing to conflate.
        DeployerOnly,
        /// **Both can set it, and a mechanism distinguishes them.** Name the
        /// mechanism in the row — a row claiming this with nothing behind it is
        /// the defect this census exists to make impossible.
        UserMayOverride(&'static str),
        /// Both can set it, and nothing distinguishes them — the `home_site`
        /// shape before this audit. **Only legal for a field that is never
        /// adopted over an established value.** Say which arm keeps it out.
        AdoptedOnlyOnFirstContact(&'static str),
    }

    /// The census. One row per `DeploymentConfig` field, in declaration order.
    #[test]
    fn every_deployment_declared_field_says_who_owns_it() {
        let rows: Vec<(&str, Owned)> = vec![
            (
                "surface",
                Owned::AdoptedOnlyOnFirstContact(
                    "HomeDecision::FirstContact — apply_to runs on a profile that never \
                     read a document; the warm-boot arms never touch posture",
                ),
            ),
            ("window_type", Owned::AdoptedOnlyOnFirstContact("with `surface`")),
            (
                "home_site",
                Owned::UserMayOverride(
                    "session_config::HomeSource — stamped `User` by set_home_site, \
                     `Deployment` by apply_to; read by decide_home",
                ),
            ),
            (
                "origins",
                Owned::UserMayOverride(
                    "content_site::origins `source: deployment | user`, with \
                     Adoption::KeptUserOverride",
                ),
            ),
            ("site_mode", Owned::AdoptedOnlyOnFirstContact("with `surface`")),
            ("fast_paint", Owned::AdoptedOnlyOnFirstContact("with `surface`")),
            ("peer_creation_enabled", Owned::DeployerOnly),
            ("name_resolver_max_ttl_ms", Owned::DeployerOnly),
            (
                "name_registry_pin",
                Owned::UserMayOverride(
                    "session_config::pinned_registry — the user's mirror above the \
                     deployment's seed, PinSource::{User, Deployment}",
                ),
            ),
        ];

        // The count is the gate. A field added to `DeploymentConfig` without a
        // row here fails, which is the only moment anyone is guaranteed to ask
        // the ownership question about it.
        assert_eq!(
            rows.len(),
            9,
            "a field was added to or removed from DeploymentConfig without classifying it. \
             MODEL-STAKEHOLDERS-AND-OWNERSHIP §5: answer 1 (which role owns it) and 5 (who \
             can fix it) before shipping"
        );

        // No row may claim a mechanism it does not name.
        for (field, owned) in &rows {
            match owned {
                Owned::UserMayOverride(m) | Owned::AdoptedOnlyOnFirstContact(m) => assert!(
                    !m.trim().is_empty(),
                    "{field} claims a mechanism without naming one — that is the shape the \
                     census exists to catch"
                ),
                Owned::DeployerOnly => {}
            }
        }

        // …and the two fields the reconcile actually adopts over an ESTABLISHED
        // value must both be `UserMayOverride`. This is the assertion that would
        // have failed before the fix.
        for field in ["home_site", "name_registry_pin"] {
            let row = rows.iter().find(|(f, _)| *f == field).expect("row present");
            assert!(
                matches!(row.1, Owned::UserMayOverride(_)),
                "{field} is adopted on a WARM boot over a value the user may have set, and \
                 nothing distinguishes the two. That is AP49, and it is how a deliberate \
                 setting gets silently overwritten"
            );
        }
    }
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

    // ── The three-state read (map-B3) ─────────────────────────────────────
    //
    // These are native because [`classify`] is pure — the split exists so every
    // outcome has a test instead of only the one a browser happened to produce.

    fn answered(status: u16, body: &str) -> Option<crate::net::BoundedResponse> {
        Some(crate::net::BoundedResponse {
            status,
            ok: (200..300).contains(&status),
            text: body.to_string(),
        })
    }

    /// **P — one variant per outcome, and each is a different KIND of fact.**
    #[test]
    fn every_outcome_of_a_document_read_has_its_own_variant() {
        // Served: the origin answered and declared something.
        let served = classify(answered(200, r#"{"surface":"site"}"#));
        assert!(matches!(served, DocumentRead::Served(_)), "got {served:?}");
        assert_eq!(served.clone().into_config().unwrap().surface.as_deref(), Some("site"));

        // 404: a FACT about the deployment — it serves no document, by choice.
        assert_eq!(classify(answered(404, "")), DocumentRead::NoDocument { status: 404 });
        assert_eq!(classify(answered(410, "")), DocumentRead::NoDocument { status: 410 });
        // Served-but-declares-nothing is the same fact by a different route.
        assert_eq!(classify(answered(200, "{}")), DocumentRead::NoDocument { status: 200 });

        // **And NOT any other non-2xx.** The first version of `classify` mapped
        // every `!ok` to `NoDocument`, so a CDN 502 reported as "this deployment
        // runs on build-time defaults BY CHOICE" — a false claim about the
        // deployer's intent, and the exact conflation this enum exists to
        // remove, one status family over. Found auditing this session's own
        // work; `PollError::NotFound` had already written the rule down.
        for status in [403u16, 500, 502, 503] {
            assert_eq!(
                classify(answered(status, "")),
                DocumentRead::OriginError { status },
                "a {status} says NOTHING about whether a document exists"
            );
        }

        // A fact about the BYTES, never about whether a document exists. A
        // truncated or half-written file, or a proxy error page served 200.
        assert_eq!(
            classify(answered(200, "{ this is not json")),
            DocumentRead::Unreadable { status: 200 }
        );

        // Nothing heard: unreachable, aborted, or D23's deadline. A fact about
        // nothing, and the only outcome from which no conclusion may be drawn.
        assert_eq!(classify(None), DocumentRead::Unheard);
    }

    /// **N2 — a deadline expiry must be distinguishable from a 404, in the line
    /// an incident is debugged from.**
    ///
    /// This is the whole deliverable. The previous single `None` produced one
    /// outcome for a domain that ships no config on purpose and for a domain
    /// nobody could reach — opposite problems, one message. Asserted on the
    /// rendered sentence rather than on the variant, because the variant being
    /// distinct is worth nothing if both render the same words.
    #[test]
    fn a_deadline_does_not_read_like_a_404() {
        let not_served = classify(answered(404, ""));
        let unheard = classify(None);
        let unreadable = classify(answered(200, "nonsense"));

        let origin_error = classify(answered(502, ""));
        let all = [&not_served, &unheard, &unreadable, &origin_error];
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                assert_ne!(a.describe(), b.describe(), "{a:?} vs {b:?} render the same words");
                assert_ne!(a.label(), b.label(), "{a:?} vs {b:?} share a structured field");
            }
        }
        assert!(
            origin_error.describe().contains("says NOTHING"),
            "a server fault must not read as a statement about the deployment: {}",
            origin_error.describe()
        );
        // And each says the RIGHT thing, not merely a different thing.
        assert!(
            not_served.describe().contains("by choice"),
            "a 404 is the deployment's decision: {}",
            not_served.describe()
        );
        assert!(
            unheard.describe().contains("NOT evidence"),
            "a deadline must not read as 'the origin has none': {}",
            unheard.describe()
        );
        assert!(
            unreadable.describe().contains("malformed, not absent"),
            "unreadable bytes are not an absent document: {}",
            unreadable.describe()
        );
    }

    /// **The only distinction a caller may branch on**: did the origin answer?
    ///
    /// Everything else is reporting. This is the line between a fact about the
    /// deployment and a fact about the network, and it is what stops a future
    /// caller from writing down "this origin has no document" because a boot
    /// happened to time out (AP30 corollary (a)).
    #[test]
    fn only_a_404_is_a_fact_about_the_deployment() {
        // `origin_answered` is the literal question: was there an HTTP response.
        assert!(classify(answered(200, r#"{"surface":"site"}"#)).origin_answered());
        assert!(classify(answered(404, "")).origin_answered());
        assert!(classify(answered(502, "")).origin_answered());
        assert!(classify(answered(200, "broken")).origin_answered());
        assert!(!classify(None).origin_answered(), "a deadline is not an answer");

        // **And it is NOT the predicate a caller may act on.** A 502 answered
        // and said nothing about whether a document exists. Only `NoDocument`
        // is a statement about the deployment — which is why the useful
        // predicate is narrower than "did it answer", and why conflating them
        // is what the first version of this enum did.
        assert!(classify(answered(404, "")).declares_no_document());
        assert!(classify(answered(200, "{}")).declares_no_document());
        for other in [
            classify(answered(200, r#"{"surface":"site"}"#)),
            classify(answered(502, "")),
            classify(answered(403, "")),
            classify(answered(200, "broken")),
            classify(None),
        ] {
            assert!(
                !other.declares_no_document(),
                "{other:?} is not the deployment saying it has no document"
            );
        }

        // And only `Served` yields a config — nothing else may shape a boot,
        // which `into_config` enforces at the type level rather than by
        // convention.
        assert!(classify(answered(404, "")).into_config().is_none());
        assert!(classify(answered(502, "")).into_config().is_none());
        assert!(classify(answered(200, "broken")).into_config().is_none());
        assert!(classify(None).into_config().is_none());
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
