//! `entity-browser publish` — the headless native invocation surface for
//! the publish pipeline (stage [C]).
//!
//! This is the thin CLI wrapper the map calls for: it does no rendering of
//! its own — it builds a peer, reads its sites off the tree
//! ([`super::read::read_all_sites`], workstream [A]), and projects them to
//! static HTML ([`super::static_export::export_owned_sites`], [B1]). One
//! read, one emitter today; more emitters slot in behind the same read.
//!
//! ## The peer-source seam (the open question — deliberately minimal today)
//!
//! *Which* peer's tree do we publish? A real deployment publishes a
//! **dedicated hosting peer** with a **stable identity** and **durable,
//! pre-seeded** content (its peer-id is the address, so it can't be random
//! per run). That peer lives either in the Tauri backend layout
//! (`~/.entity/peers/{name}/store.db`, SQLite) or browser Worker OPFS — and
//! loading one of those into a headless native process is the deferred piece
//! (it needs the native store + keypair load path, the same gap as durable
//! Direct mode). **Until then, `publish` operates on a fresh ephemeral peer
//! seeded with the bundled demo site set** — i.e. today it is a *demo / SSG
//! generator*, honestly so. The seam is [`resolve_publish_source`]: swap the
//! "fresh + seed demo" body for "load peer dir → read its real sites" and
//! every emitter downstream is unchanged.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use super::format::{NavItem, SiteManifest, SitePage};
use super::paths::SITE_URL_PREFIX;
use super::{paths, read, static_export};
use crate::peers::Peers;

/// Default output directory when none is given on the command line. Lives
/// under `dist/` (git-ignored), served by `make serve` / any static server.
const DEFAULT_OUT_DIR: &str = "dist/static-demo";

/// Second demo site id — a sibling of the bundled `demo` site that
/// cross-links into it, so a publish exercises multi-site + cross-site link
/// rewriting (not just one isolated site).
const INFO_SITE_ID: &str = "entity-info";

/// CLI entry: `entity-browser publish [OUT_DIR] [flags]`. `args[0]` is the
/// `publish` verb. Flags:
/// - `--bare-root` — render a **single** site at the domain root (the SSG
///   on-ramp, [F1]) instead of the multi-site `sites/{peer}/{site}/` projection.
/// - `--site=ID` — which site to render in bare-root mode (default: the demo
///   site, else the first site read). Ignored in projection mode (all sites).
/// - `--live=<origin>` — add the "open in live peer" banner ([F2]).
/// - `--html-only` — skip the entity-native `.bin` content data ([B2]);
///   projection mode emits both `.html` + `.bin` by default. (Bare-root is
///   always HTML-only — it's the dumb-CDN SSG surface.)
/// - `--allow-out-of-set-links` — downgrade an out-of-set `site:`/`entity://`
///   target from a **failure** to a warning. Such a link is resolved against
///   *this* peer and ships a 404 (measured on production; see
///   [`warn_out_of_set_links`]). Reported either way — the flag chooses
///   *failure* vs *warning*, and the default is failure. Projection mode only:
///   bare-root is a single site, so there is no set to be outside of.
///   (`--strict-links` is accepted and ignored — it asked for today's default.)
/// - `--plan` — resolve the sources and report what this publish **would**
///   add / keep / remove, writing nothing. Exit `0` = removes nothing,
///   `2` = removes sites, `1` = error, so `publish --plan … && publish …` is a
///   gate. See [`run_plan`] for the contract; pass the same flags you intend to
///   run, since a plan of a different command predicts a different publish.
/// - `--deployment-config` — also emit `/entity-deployment.json` (cut 2b): the
///   per-domain config that points a **generic** SPA bundle served from this
///   origin at the published home site. `--surface=<chrome|site|window>`
///   (default `window` + `--window-type="Site Browser"` — boots into a maximized
///   Site Browser window that keeps the chrome toggle so the author is never
///   locked in) sets the startup surface; `--surface=site --locked` is the
///   opt-in overlay kiosk (no toggle, no peer creation, escapable only via
///   `?chrome=1`). `--config-site=ID` (default: demo, else first) sets the home
///   site. The origin is the `--live` value if given, else `""` (same-origin —
///   the SPA expands it to its own origin at runtime). Projection mode only.
///
///   **It MERGES onto an existing document.** The file is domain-managed and
///   names every peer hosted here, so publishing a second peer under its own
///   `--prefix` adds an `origins` entry and leaves the domain's home alone. Use
///   `--set-home` to move the home onto the peer being published — a deliberate
///   act, because the flip also records the previous home peer as *retired* in
///   every returning visitor's browser. An existing document that cannot be
///   parsed stops the publish rather than being replaced.
/// - `--set-home` — with `--deployment-config`, take this domain's `home_site`
///   even though the existing document names a different peer. What a re-key
///   uses, and what the second peer on a shared domain must NOT do by accident.
/// - `--identity-seed=<64-hex>` — publish under a **specific system identity**
///   (the same 32-byte hex seed form as the runtime `entity_system_seed`), so
///   each site/deployment gets its own stable peer-id (`sites/{peer}/…`).
///   Default (absent) is the DURABLE publisher keypair
///   (`persistence::publisher_keypair`, load-or-generate under `ENTITY_DATA_DIR`);
///   `--demo-identity` uses the fixed demo seed. A malformed seed fails the build.
///
/// The first non-flag positional is the output dir (default
/// [`DEFAULT_OUT_DIR`]). Returns a process exit code.
pub fn run(args: &[String]) -> ExitCode {
    let bare_root = args.iter().any(|a| a == "--bare-root");
    let site_filter: Option<String> =
        args.iter().find_map(|a| a.strip_prefix("--site=").map(str::to_string));
    // `--live=<origin>` adds the dismissable "open in live peer" banner ([F2]),
    // deep-linking each page to `{origin}/?site=…` in the live SPA.
    let live_base: Option<String> =
        args.iter().find_map(|a| a.strip_prefix("--live=").map(str::to_string));
    // Projection publish emits BOTH the legacy-web `.html` AND the
    // entity-native `.bin` content data ([B2]) by default; `--html-only` skips
    // the `.bin` (the dumb-CDN-only case). Bare-root is always HTML-only.
    let html_only = args.iter().any(|a| a == "--html-only");
    // `--plan` — resolve the source and report what would change, writing
    // nothing. See [`run_plan`] for the exit-code contract.
    let plan_only = args.iter().any(|a| a == "--plan");
    // `--verify` — prove an ALREADY-published tree resolves. See [`run_verify`].
    let verify_only = args.iter().any(|a| a == "--verify");
    // An out-of-set `site:`/`entity://` target is a build **failure**. The
    // concession that made it a warning expired when the authored links were
    // swept (2026-08-23); see [`warn_out_of_set_links`]. `--strict-links` is
    // kept as an accepted no-op so a caller that still passes it — the Makefile
    // did, and so did other repos' pipelines — does not fail on an unknown flag
    // while asking for exactly what it now gets. `--allow-out-of-set-links` is
    // the deliberate way back to a warning.
    let strict_links = refuses_out_of_set_links(args);
    // Cut 2b: optionally emit the per-domain deployment config.
    let deployment_config = args.iter().any(|a| a == "--deployment-config");
    // Startup SURFACE (the axis that replaced `--config-profile`): `chrome` /
    // `site` / `window` (+ `--window-type` for window). Default `window` +
    // `Site Browser` — a maximized Site Browser window that keeps the chrome
    // toggle, so the author is never locked in. `--surface=site --locked` is the
    // opt-in overlay kiosk (escapable only via `?chrome=1`).
    let config_surface: String = args
        .iter()
        .find_map(|a| a.strip_prefix("--surface=").map(str::to_string))
        .unwrap_or_else(|| "window".to_string());
    let config_window_type: String = args
        .iter()
        .find_map(|a| a.strip_prefix("--window-type=").map(str::to_string))
        .unwrap_or_else(|| crate::session_config::SITE_BROWSER_WINDOW.to_string());
    let config_locked = args.iter().any(|a| a == "--locked");
    let config_site: Option<String> =
        args.iter().find_map(|a| a.strip_prefix("--config-site=").map(str::to_string));
    // `--set-home` — move this domain's home onto the peer being published.
    // Needed only when a document already names a DIFFERENT peer as home; see
    // `HomeClaim`. Without it a second publish defers, which is what stops the
    // last publish silently re-homing a domain (and writing a supersession
    // record against a peer that is still alive).
    let config_set_home = args.iter().any(|a| a == "--set-home");
    // `--registry-pin=PEER_ID@ORIGIN` — the §7.4 preloaded registry this
    // deployment seeds. Same `PEER_ID@ORIGIN` spelling as `registry --bind`,
    // because it is the same pair and a second spelling is a second thing to get
    // wrong. Validated below, before anything is written.
    let registry_pin_raw: Option<String> =
        args.iter().find_map(|a| a.strip_prefix("--registry-pin=").map(str::to_string));
    // Publisher identity resolution (see `resolve_publish_keypair`):
    //   default            → the DURABLE publisher keypair under `{ENTITY_DATA_DIR}/publish/`
    //   --identity-seed=hex → a SPECIFIC system identity (same hex form as `entity_system_seed`)
    //   --demo-identity     → the fixed demo publisher seed (dev/testing only)
    // Validated up front (fail on a bad seed rather than silently publishing
    // under the wrong identity).
    let keypair = match resolve_publish_keypair(args) {
        Ok(kp) => kp,
        Err(e) => {
            eprintln!("publish {e}");
            return ExitCode::FAILURE;
        }
    };
    // B14: the signed root is signed by the SAME identity the content is
    // published under — an explicit `clone_inner` (Keypair deliberately is not
    // `Clone`, so duplication stays audit-visible) rather than a second
    // resolve, so the two cannot diverge even if the durable keypair were
    // regenerated between the calls. `resolve_publish_source` consumes its copy
    // building the source peer; this one goes to the `RootProjector`.
    let publisher_key = keypair.clone_inner();
    // `--ingest=<dir>` sources the tree from a content-team `render/` emit
    // (one site dir, or a parent of site dirs) instead of the bundled demo
    // seed — the disk→tree half of the cross-team pipeline.
    let ingest_dir: Option<PathBuf> = args
        .iter()
        .find_map(|a| a.strip_prefix("--ingest=").map(PathBuf::from));
    // App sets (games + apps) ride along on EVERY publish (same `.bin` two-hop
    // content data, a different subgraph `{peer}/apps/{set}/…`; the live
    // Games/Apps window fetches a bundle on click-through like a site asset).
    // `--ingest-apps=<dir>` OVERRIDES the source with an entity-apps `dist/`
    // (split into games/apps by entry type); without it, a minimal demo seed is
    // published. `--ingest-games=` stays accepted as an alias.
    let ingest_apps: Option<PathBuf> = args.iter().find_map(|a| {
        a.strip_prefix("--ingest-apps=")
            .or_else(|| a.strip_prefix("--ingest-games="))
            .map(PathBuf::from)
    });
    // `--prefix=<path>` is the per-peer hosting scope: everything (`.html`
    // projection, `.bin` content data, deployment-config origin) nests under
    // `{out}/{PREFIX}/…`. Empty (the default) = the domain root, byte-identical
    // to the un-prefixed layout. Projection mode only (see the bare-root guard).
    let prefix_raw: String = args
        .iter()
        .find_map(|a| a.strip_prefix("--prefix=").map(str::to_string))
        .unwrap_or_default();
    let out_dir: PathBuf = args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_OUT_DIR));

    // Validate + normalize the hosting prefix at the CLI boundary (reject `..`,
    // leading/trailing `/`, reserved first segment). `--bare-root` IS the root,
    // so a prefix there is a contradiction — fail rather than silently ignore.
    //
    // This sits above the `--verify` return because verify *uses* the prefix:
    // it looks under `{out}/{prefix}/{peer}/`. The emit-only guards below it do
    // not.
    let prefix = match paths::normalize_prefix(&prefix_raw) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("publish --prefix: {e}");
            return ExitCode::FAILURE;
        }
    };
    if !prefix.is_empty() && bare_root {
        eprintln!(
            "publish: --prefix nests the multi-site projection under {prefix:?}; \
             --bare-root renders ONE site at the domain root — drop one."
        );
        return ExitCode::FAILURE;
    }

    // `--verify` inspects an ALREADY-published tree, so it deliberately runs
    // BEFORE the source is ingested: it needs the identity (to find
    // `{out}/{prefix}/{peer}/`) and nothing else. Ingesting first would make
    // verifying a shipped directory depend on still having its sources.
    //
    // **It also runs before every EMIT-time guard below**, and that ordering is
    // load-bearing rather than tidy. Verify writes nothing, so a rule about what
    // an emitted config would carry cannot be violated by it — and applying one
    // anyway refuses a verification for a reason that has no bearing on it. That
    // is not hypothetical: `make site-dist` publishes and then re-runs the same
    // target with `VERIFY=1`, inheriting the caller's knobs, so
    // `make site-dist REGISTRY_PIN=…` published correctly and then failed its
    // own verification pass — after the tree was already written. The other
    // guards never surfaced it because they key on `--deployment-config`, which
    // that pass does not pass; the pin guard keys on the *pin*, which it does.
    if verify_only {
        let peers = Peers::new_direct_with_keypair(keypair);
        let peer_id = peers.primary_peer_id().to_string();
        return run_verify(&out_dir, &peer_id, &prefix, &publisher_key);
    }

    // Validate the deployment-config surface up front (fail the build on a typo,
    // like `ENTITY_STARTUP_SURFACE` does) — before any filesystem work.
    if deployment_config && !matches!(config_surface.as_str(), "chrome" | "site" | "window") {
        eprintln!(
            "publish --deployment-config: unknown --surface={config_surface:?} \
             (expected chrome | site | window)"
        );
        return ExitCode::FAILURE;
    }
    if deployment_config && config_locked && config_surface != "site" {
        eprintln!(
            "publish --deployment-config: --locked only applies to --surface=site \
             (the overlay kiosk); got --surface={config_surface:?}"
        );
        return ExitCode::FAILURE;
    }
    // A pin with no config file to carry it does nothing at all, silently — the
    // shape where an operator ships a deployment believing it seeds a registry.
    if registry_pin_raw.is_some() && !deployment_config {
        eprintln!(
            "publish --registry-pin: the pin rides in /entity-deployment.json, so it needs \
             --deployment-config too (without it nothing would carry the pin)"
        );
        return ExitCode::FAILURE;
    }
    let registry_pin = match registry_pin_raw.as_deref().map(parse_registry_pin).transpose() {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("publish --registry-pin: {msg}");
            return ExitCode::FAILURE;
        }
    };
    if deployment_config && bare_root {
        eprintln!(
            "publish: --deployment-config is for the SPA projection (sites/{{peer}}/…), \
             not --bare-root (single site at domain root) — drop one."
        );
        return ExitCode::FAILURE;
    }

    // [A] Resolve the source peer + read all its sites AND app sets off the tree.
    let (peer_id, sites, app_sets) =
        match resolve_publish_source(keypair, ingest_dir.as_deref(), ingest_apps.as_deref()) {
            Ok(triple) => triple,
            Err(e) => {
                eprintln!("publish --ingest: {e}");
                return ExitCode::FAILURE;
            }
        };
    if sites.is_empty() {
        eprintln!("publish: no sites found on peer {peer_id} — nothing to publish.");
        return ExitCode::FAILURE;
    }

    // A deployment config naming a home site that isn't published would point
    // the SPA at a 404 — fail fast rather than emit a broken config.
    if deployment_config {
        if let Some(id) = &config_site {
            if !sites.iter().any(|s| &s.site_id == id) {
                eprintln!(
                    "publish --deployment-config: --config-site={id:?} not among published \
                     sites {:?}",
                    sites.iter().map(|s| s.site_id.as_str()).collect::<Vec<_>>()
                );
                return ExitCode::FAILURE;
            }
        }
    }

    // `--plan`: say what this publish WOULD do to the existing projection, and
    // write nothing. Placed here — after the source is resolved (so the plan is
    // about real sites, not a guess) and before any clean — because the clean is
    // the destructive step and a plan that ran after it would be a report.
    if plan_only {
        return run_plan(&out_dir, &peer_id, &sites, &prefix, bare_root, &app_sets, !html_only);
    }

    if bare_root {
        run_bare_root(&out_dir, &sites, site_filter.as_deref(), live_base.as_deref())
    } else {
        let deploy_spec = deployment_config.then(|| {
            // Origin: the hosting prefix rides inside the registered origin, so
            // http_poll resolves `{origin}/{peer}/sites/…` unchanged. Explicit
            // `--live` (cross-origin host) + prefix → `{live}/{prefix}`; no
            // `--live` → `/{prefix}` (root-relative same-origin, expanded by the
            // SPA at runtime) or `""` at the root (the portable same-origin value).
            let origin = deploy_origin(live_base.as_deref(), &prefix);
            // D13: a loopback origin baked into a shipped bundle is the footgun
            // this whole arc is about — surface it where it's created.
            warn_if_loopback_origin(&origin);
            DeployConfigSpec {
                surface: config_surface,
                window_type: config_window_type,
                locked: config_locked,
                site: config_site,
                origin,
                registry_pin,
                set_home: config_set_home,
            }
        });
        run_projection(
            &out_dir,
            &peer_id,
            &sites,
            &prefix,
            live_base.as_deref(),
            !html_only,
            deploy_spec,
            &app_sets,
            publisher_key,
            strict_links,
        )
    }
}

/// The deployment-config origin for a published peer, folding in the hosting
/// `prefix`. Empty prefix → today's value (`{live}` or `""` same-origin),
/// byte-identical. With a prefix: `{live}/{prefix}` for a cross-origin host, or
/// `/{prefix}` (root-relative) for same-origin so the SPA's `expand_origin`
/// prepends its own origin at runtime.
fn deploy_origin(live_base: Option<&str>, prefix: &str) -> String {
    match (live_base, prefix.is_empty()) {
        (Some(b), true) => b.to_string(),
        (Some(b), false) => format!("{}/{}", b.trim_end_matches('/'), prefix),
        (None, true) => String::new(),
        (None, false) => format!("/{prefix}"),
    }
}

/// Whether a deployment-config origin bakes a **loopback** host into the
/// bundle (`localhost` / `127.0.0.1`). Such an origin is a *dev* artifact: the
/// published `dist/` only resolves content on the publishing machine, so dropped
/// on a CDN/R2 it serves the app shell with **no content** (and an `https://`
/// page fetching `http://localhost` is additionally blocked as mixed content).
/// The portable production form is an **empty** origin (same-origin — the SPA
/// fetches from whatever host serves it). Pure predicate so it is unit-testable.
fn origin_is_loopback(origin: &str) -> bool {
    let o = origin.trim();
    o.contains("localhost") || o.contains("127.0.0.1")
}

/// D13 surface: loudly flag a loopback deployment-config origin at publish time
/// (the one place the footgun is created), with the portable alternative.
fn warn_if_loopback_origin(origin: &str) {
    if origin_is_loopback(origin) {
        eprintln!(
            "⚠️  publish --deployment-config: origin {origin:?} is a LOOPBACK address — \
             this bundle is NOT portable. Dropped on a CDN/R2 it serves the app shell with \
             NO content. For a production deploy, publish with an EMPTY --live (same-origin: \
             the SPA fetches from whatever host serves it; works at any domain ROOT). Only \
             pin an absolute origin for a deliberate cross-origin case."
        );
    }
}

/// What `--deployment-config` emits — the startup surface + home site + serving
/// origin for the published SPA. Resolved from the CLI flags in [`run`].
struct DeployConfigSpec {
    /// Startup surface (`chrome` / `site` / `window`), validated.
    surface: String,
    /// Window type when `surface == "window"` (e.g. `"Site Browser"`).
    window_type: String,
    /// Locked overlay kiosk (`surface == "site"` only): no chrome toggle, no
    /// peer creation. Emits the explicit `site_mode` + `peer_creation_enabled`.
    locked: bool,
    /// Home site id; `None` → the demo site, else the first published.
    site: Option<String>,
    /// Serving origin for the published peer; `""` = same-origin.
    origin: String,
    /// The §7.4 registry this deployment seeds as a pin, if any. Validated at
    /// the CLI boundary by [`parse_registry_pin`] — an unusable pin is refused
    /// where the operator can read the refusal, never emitted for a consumer to
    /// drop silently (audit F9's rule, one artifact along).
    registry_pin: Option<crate::session_config::RegistryPin>,
    /// **`--set-home`: this publish becomes the domain's home**, replacing a
    /// home an existing document already names for a *different* peer.
    ///
    /// Off by default, and that default is the whole fix: without it the second
    /// peer published to a domain silently re-homed every returning visitor onto
    /// itself (see [`merge_deployment_doc`]). Moving a domain's home is a
    /// deliberate act, so it gets a word.
    set_home: bool,
}

/// What a publish is allowed to do to a domain's existing
/// `/entity-deployment.json`.
///
/// **The document is DOMAIN-managed and describes the whole domain**, not the
/// last publish — `DESIGN-DEPLOYMENT-GENERATIONS` §7: *"`/entity-deployment.json`
/// ← names peers, their prefixes, their active generations."* A domain may host
/// several publishers, each under its own `--prefix`, and the tooling has
/// supported that since prefixes existed.
///
/// It did not survive contact with a second publish. Measured 2026-09-03 by
/// publishing two peers to one out-dir: both trees emitted correctly and
/// completely, and the document came out naming **only the second**, because
/// this emitter built a fresh single-entry `origins` map and `fs::write`-clobbered
/// the file. Three consequences, in ascending order of cost:
///
/// 1. The first peer's origin entry is **gone**, so a browser booting that
///    domain never learns where its artifacts live.
/// 2. `home_site` flips to whoever published **last** — a domain-level decision
///    made by publish order.
/// 3. **And that flip writes a false supersession record.** A returning profile
///    holds `home = alpha` marked `Deployment`; the document now says `beta`;
///    `decide_home` returns `AdoptDeclared`; boot persists `alpha → beta` — a
///    durable record asserting a peer that is **alive and serving** was retired,
///    after which `resolve()` silently rewrites every stored reference to alpha
///    onto beta. Revalidation *keeps* it, correctly by its own rule, because the
///    document does agree that beta is home. That is F2's brick reached through
///    a different door: not a bad document, but a correct-for-one-peer document
///    on a domain that has two.
///
/// So the rule is: **the home publish owns the domain-level fields; a secondary
/// publish contributes only its own `origins` entry.**
#[derive(Debug, PartialEq, Eq)]
enum HomeClaim {
    /// No document yet, or it names no usable home — this publish defines the
    /// domain.
    Defines,
    /// The existing document already names *this* peer as home. An ordinary
    /// republish; rewrite the domain-level fields as before.
    Republishes,
    /// The document names a **different** peer as home and `--set-home` was not
    /// given. Keep every domain-level field; contribute only `origins[peer]`.
    Defers { to: String },
    /// A different peer is home and the operator asked for it to move.
    Takes { from: String },
}

/// Decide what this publish may claim, given the home peer an existing document
/// names. **Pure**, because this branch is the whole safety argument and the
/// alternative is proving it through the filesystem.
///
/// `existing_home` is the `home_site.peer` already on the domain, if any.
fn home_claim(existing_home: Option<&str>, peer_id: &str, set_home: bool) -> HomeClaim {
    match existing_home.map(str::trim).filter(|p| !p.is_empty()) {
        None => HomeClaim::Defines,
        Some(h) if h == peer_id => HomeClaim::Republishes,
        Some(h) if set_home => HomeClaim::Takes { from: h.to_string() },
        Some(h) => HomeClaim::Defers { to: h.to_string() },
    }
}

/// Parse `--registry-pin=PEER_ID[@ORIGIN]`. `Err` is the operator-facing
/// message, verbatim — a value rather than an `eprintln!`, because an exit-code
/// assertion cannot tell "refused correctly" from "refused for the wrong
/// reason", which is exactly how the `--bind=` spelling bug survived (AP25).
///
/// **The peer-id must be canonical form**, and that is a stronger check than
/// `--bind`'s deliberately-tolerant `PeerId::validate`. The two are refusing
/// different things: a *binding target* is resolved by consumers we do not
/// speak for, so refusing the SHA-256 legacy form there would be us deciding
/// what they may resolve. A *pin* is consumed by this client, which derives the
/// verification key out of the peer-id itself — a legacy-form pin needs an
/// out-of-band key there is nowhere to put, so it cannot work here and the
/// operator should hear that now rather than as "the pinned registry is
/// unusable" in a browser console.
fn parse_registry_pin(raw: &str) -> Result<crate::session_config::RegistryPin, String> {
    let raw = raw.trim();
    // Split on the FIRST `@`, the same end as `--bind`: a Base58 peer-id cannot
    // contain one and an origin can (`http://user@host`).
    let (peer_id, origin) = match raw.split_once('@') {
        Some((p, o)) if !p.is_empty() && !o.is_empty() => (p, o),
        // Checked before the both-sides message, because the two halves fail for
        // different reasons and only one of them is a security statement.
        Some(("", _)) => {
            return Err("a registry pin needs a peer-id — an origin alone is not a pin, and \
                        pinning an origin would trust the origin"
                .into())
        }
        Some(_) => {
            return Err(format!(
                "expected PEER_ID@ORIGIN (or a bare PEER_ID for same-origin), got {raw:?}"
            ))
        }
        None => (raw, ""),
    };
    if peer_id.is_empty() {
        return Err("a registry pin needs a peer-id — an origin alone is not a pin, and \
                    pinning an origin would trust the origin"
            .into());
    }
    // The pin IS the key: canonical form embeds the 32 public-key bytes, which
    // is why nothing is fetched to learn who the registry is. Building the
    // pin here is the same call the consumer makes, so the two cannot disagree
    // about what a usable pin looks like.
    if crate::content_site::signed_fetch::PinnedPublisher::from_peer_id(origin, peer_id).is_none() {
        return Err(format!(
            "{peer_id} is not a canonical-form peer-id — it carries no public key, so a \
             consumer cannot pin it (the SHA-256 legacy form needs an out-of-band key, and \
             there is nowhere in a deployment config to put one)"
        ));
    }
    Ok(crate::session_config::RegistryPin {
        peer_id: peer_id.to_string(),
        origin: origin.to_string(),
    })
}

/// Projection mode: every site under `sites/{peer}/{site}/…` + a peer index
/// (legacy-web `.html`), and — when `emit_bin` — the entity-native `.bin`
/// content data alongside it ([B2]).
/// The in-site nav targets in `site`'s manifest that don't resolve to one of
/// its pages — the "deleted the page but left the nav entry" footgun, which
/// otherwise ships a **dangling 404 link with no warning** (nav is authored
/// separately from the pages, so nothing catches it). Returns `(label, target)`
/// per offender. Only IN-SITE targets are checkable here: cross-site /
/// cross-peer targets resolve against *other* sites (not knowable from one
/// site's pages) and external links leave the system. Section headers (empty
/// target) are skipped; `children` are walked recursively.
fn dangling_nav_targets(site: &read::OwnedSite) -> Vec<(String, String)> {
    use crate::content_site::{classify_link, LinkTarget, Location};
    let root = Location { peer_id: None, site_id: site.site_id.clone(), page: String::new() };
    let has_page = |page: &str| {
        // An empty in-site slug (`/`) means the manifest's declared root page.
        let slug = if page.is_empty() { site.manifest.root() } else { page };
        site.pages.iter().any(|(s, _)| s == slug)
    };
    let mut dangling = Vec::new();
    let mut stack: Vec<&crate::content_site::NavItem> = site.manifest.nav.iter().collect();
    while let Some(item) = stack.pop() {
        stack.extend(item.children.iter());
        if item.target.is_empty() {
            continue; // section header — no link to resolve
        }
        if let LinkTarget::InSite { page } = classify_link(&item.target, &root) {
            if !has_page(&page) {
                dangling.push((item.label.clone(), item.target.clone()));
            }
        }
    }
    dangling
}

/// Warn (loudly, one line per offender) for every dangling in-site nav link
/// across the published set — the publish-safety ratchet for the delete-a-page
/// footgun. Non-fatal: the site still ships, but the operator gets the signal
/// the old pipeline swallowed. Returns the total count (0 = clean).
fn warn_dangling_nav_links(sites: &[read::OwnedSite]) -> usize {
    let mut total = 0;
    for site in sites {
        for (label, target) in dangling_nav_targets(site) {
            eprintln!(
                "publish: WARNING — site {:?} nav item {label:?} → {target:?} resolves to no page \
                 (a dangling 404 link; delete the nav entry or add the page)",
                site.site_id
            );
            total += 1;
        }
    }
    if total > 0 {
        eprintln!("publish: {total} dangling nav link(s) — the site ships but those links 404.");
    }
    total
}

/// Report every link whose target was **not in the export set**, and say what
/// it means. Returns `false` when the publish must fail (`strict`).
///
/// This is the enforcement point the cross-site-link contract never had. Both
/// sides agreed years of prose ago that a genuinely cross-domain link is left
/// external (`https://…`) until the registry can resolve one
/// (`entity-core-papers/docs/CROSS-SITE-LINKS.md` §Type 2) — and nothing
/// anywhere checked it, so seven links quietly shipped 404s to production.
///
/// **Refuses by default. The concession expired on 2026-08-23 and was spent.**
///
/// This shipped as a warning for one stated reason, with one stated expiry:
/// flipping the default would have refused the per-domain builds shipping *at
/// that moment*, over seven authored links in content this repo does not own.
/// `entity-core-papers` swept them (`ef3f662`), and it was verified here rather
/// than taken on report — **0** cross-domain `site:` refs across all five
/// domains of the corpus, and all four published domains emit clean under the
/// refusal (412 pages, 0 dangling). So the reason is gone, and a concession
/// whose reason is gone is just the design.
///
/// `--allow-out-of-set-links` is the way back to a warning. It exists because
/// somebody publishing a genuinely partial set mid-migration needs a build more
/// than they need this guard — but it is now a thing you *ask for*, in a command
/// somebody can see, rather than what you get by forgetting.
/// Does this argv **refuse** an out-of-set link, or merely warn about one?
///
/// A free function with its own tests because **the default is the whole
/// point**, and until now the default lived in a `let` in the middle of a
/// several-hundred-line `run_publish` where no test could see it. That is
/// exactly how a concession becomes the design: the guard was a warning for a
/// stated reason with a stated expiry, and when the expiry was spent the only
/// thing standing between the refusal and a silent slide back was one `!`.
///
/// `--strict-links` is accepted and has no effect — it asked for what is now
/// the default, and other repos' pipelines still pass it. Removing it would
/// fail their builds on an unknown flag while they ask for the right thing.
pub(crate) fn refuses_out_of_set_links(args: &[String]) -> bool {
    !args.iter().any(|a| a == "--allow-out-of-set-links")
}

fn warn_out_of_set_links(dangling: &[static_export::DanglingLink], strict: bool) -> bool {
    let what = if strict { "ERROR" } else { "WARNING" };
    for d in dangling {
        eprintln!("publish: {what} — out-of-set link {d}");
    }
    eprintln!(
        "publish: {} out-of-set link(s). These targets are not in this export set, so they \
         were resolved against THIS peer and will 404. A cross-DOMAIN target cannot be \
         expressed as a `site:`/`entity://` link today (the projection href carries no host) \
         — author it as an absolute https:// URL, per the cross-site-link contract.",
        dangling.len()
    );
    if strict {
        eprintln!(
            "publish: refusing to emit a tree with dangling cross-site links. \
             If this set is deliberately partial, pass --allow-out-of-set-links \
             (make: ALLOW_OUT_OF_SET_LINKS=1) to downgrade this to a warning."
        );
        return false;
    }
    eprintln!(
        "publish: emitting anyway — --allow-out-of-set-links was passed, so these 404s ship."
    );
    true
}

/// The site ids already projected under `{out}/{prefix}/sites/{peer}/`, sorted.
///
/// Read **before** the projection clean, because that clean is wholesale (see
/// [`warn_replaced_sites`]) and afterwards there is nothing left to compare
/// against. An unreadable or absent directory is an empty set — a first publish
/// into a fresh dir is the common case, not an error.
fn projected_site_ids(out_dir: &Path, peer_id: &str, prefix: &str) -> Vec<String> {
    let root = paths::prefixed_root(out_dir, prefix)
        .join(SITE_URL_PREFIX)
        .join(peer_id);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    ids.sort();
    ids
}

/// Every peer-id that has a site projection under `{out}/{prefix}/sites/`.
///
/// Exists for [`run_verify`]'s "you resolved a different identity" hint: a
/// publish always leaves `sites/{peer}/`, so the identity a tree was written
/// under is recoverable from the tree itself rather than from the operator's
/// memory of which flags they passed.
fn projected_peer_ids(out_dir: &Path, prefix: &str) -> Vec<String> {
    let root = paths::prefixed_root(out_dir, prefix).join(SITE_URL_PREFIX);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    ids.sort();
    ids
}

/// *"You resolved a different identity"* — the sentence to add when a verify
/// found nothing, or `None` when there is nothing to say.
///
/// **Returned as a value rather than printed**, because an exit-code assertion
/// cannot tell "refused correctly" from "refused for the wrong reason" — both
/// exit 1 here, which is exactly how a CLI refusal that names the wrong cause
/// survives a green suite. Any new refusal on this surface owes the same shape.
///
/// `None` when the tree holds no other publish: a genuinely empty or wrong
/// directory has no better diagnosis available, and inventing one would point
/// the operator away from the real answer.
fn other_identity_hint(out_dir: &Path, prefix: &str, peer_id: &str) -> Option<String> {
    let found: Vec<String> =
        projected_peer_ids(out_dir, prefix).into_iter().filter(|p| p != peer_id).collect();
    match found.len() {
        0 => None,
        1 => Some(format!(
            "NOTE — this tree DOES hold a publish under {}, and you resolved {peer_id}. \
             Same identity flags as the publish that wrote it?",
            found[0]
        )),
        _ => Some(format!(
            "NOTE — this tree holds publishes under {} — none of them the {peer_id} you \
             resolved. Same identity flags as the publish that wrote it?",
            found.join(", ")
        )),
    }
}

/// The app sets already projected under `{out}/{prefix}/{peer}/apps/`, each
/// with the number of bundle pointers under it.
///
/// The apps counterpart of [`projected_site_ids`], and it exists for the same
/// reason: `run_projection` cleans `{base}/{peer}/` wholesale when emitting
/// `.bin`, and `{peer}/apps/**` is inside that. A publish invoked without
/// `--ingest-apps` therefore **deletes every app bundle in the output tree** —
/// measured on a real staging tree: 35 objects under `apps/` → 0, total
/// 304 → 225.
///
/// The count rides along because it is what makes the report legible: "18
/// bundle(s)" names what disappears in the units the publish itself printed
/// when it put them there.
fn projected_app_sets(out_dir: &Path, peer_id: &str, prefix: &str) -> Vec<(String, usize)> {
    let root = paths::prefixed_root(out_dir, prefix).join(peer_id).join("apps");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut sets: Vec<(String, usize)> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let bundles = std::fs::read_dir(e.path().join("bundles"))
                .map(|d| {
                    d.flatten()
                        .filter(|b| {
                            b.file_name().to_string_lossy().ends_with(".bin")
                        })
                        .count()
                })
                .unwrap_or(0);
            Some((name, bundles))
        })
        .collect();
    sets.sort();
    sets
}

/// The already-projected app sets this publish does **not** carry — exactly
/// what it would delete.
///
/// An **empty** incoming set counts as not carrying it: `run_projection` skips
/// a set whose catalog has no entries, so it is projected as absent, which the
/// wholesale clean makes indistinguishable from a removal.
fn dropped_app_sets<'a>(
    existing: &'a [(String, usize)],
    incoming: &crate::apps::ingest::IngestedSets,
) -> Vec<&'a (String, usize)> {
    existing
        .iter()
        .filter(|(set, _)| {
            incoming.get(set).map(|ing| ing.catalog.entries.is_empty()).unwrap_or(true)
        })
        .collect()
}

/// The already-projected site ids that this publish does **not** carry — i.e.
/// exactly what a publish would delete.
///
/// The single definition of "what disappears", shared by [`warn_replaced_sites`]
/// (which reports it during a real publish) and [`run_plan`] (which reports it
/// instead of publishing). A `--plan` that disagreed with the publish it
/// predicts would be worse than no plan at all.
fn dropped_site_ids<'a>(existing: &'a [String], incoming: &[read::OwnedSite]) -> Vec<&'a String> {
    existing
        .iter()
        .filter(|id| !incoming.iter().any(|s| &s.site_id == *id))
        .collect()
}

/// Warn, by name, for every site that is about to **disappear** from the
/// projection because this publish did not carry it.
///
/// **A publish is whole-tree-per-`(prefix, identity)`, and that is not
/// obvious from the command line.** `run_projection` removes
/// `{out}/{prefix}/sites` (plus `content` + `{peer}` when emitting `.bin`)
/// wholesale before rewriting, so publishing a *subset* of the source set
/// **deletes** the sites left out — silently, and in about seven seconds.
/// Measured on a real tree (2026-08-17): publishing one site into a populated
/// `dist/` took it from 7 site dirs / 1862 blobs / 103 MB to 2 / 68 / 2.7 MB.
///
/// The clean itself is correct — stale artifacts must not linger, and it is
/// deliberately scoped to this prefix so a sibling peer's tree survives
/// (`--prefix` is the escape hatch, at the cost of cross-site content dedup,
/// since `content/` sits under the prefix too). What was missing is the
/// *signal*: the driver has to know the complete source set, and nothing told
/// it when it didn't.
///
/// **Warn, don't fail** — the same posture as [`warn_dangling_nav_links`].
/// Removing a site *is* a legitimate publish, and a hard failure would make the
/// legitimate case unreachable. Returns the number of sites being dropped.
///
/// Shares [`dropped_site_ids`] with [`run_plan`] deliberately: the warning and
/// the plan must never disagree about what disappears, and two copies of that
/// predicate is exactly how they would.
fn warn_replaced_sites(existing: &[String], incoming: &[read::OwnedSite]) -> usize {
    let dropped = dropped_site_ids(existing, incoming);
    if dropped.is_empty() {
        return 0;
    }
    for id in &dropped {
        eprintln!(
            "publish: WARNING — site {id:?} is already published here but is NOT in this \
             publish; the projection is replaced wholesale, so it will be REMOVED"
        );
    }
    eprintln!(
        "publish: {} site(s) will disappear. A publish replaces the whole projection under this \
         prefix — ingest the COMPLETE source set, or publish this site under its own --prefix.",
        dropped.len()
    );
    dropped.len()
}

/// Warn, by name, for every already-projected **app set** this publish does not
/// carry — the apps twin of [`warn_replaced_sites`], and the one an operator
/// hits by omitting a flag rather than by omitting a source.
///
/// **Independent of `--plan` on purpose.** The plan is the gate you run
/// deliberately; this is the one that fires on the run you did not think needed
/// a gate. Forgetting `--ingest-apps` is a single missing flag on an otherwise
/// ordinary content publish, and its whole effect — 35 objects gone — is
/// invisible in a report that only counts sites.
///
/// Shares [`dropped_app_sets`] with [`run_plan`] for the same reason
/// [`warn_replaced_sites`] shares its predicate: a plan that disagreed with the
/// publish it predicts is worse than no plan.
fn warn_replaced_app_sets(
    existing: &[(String, usize)],
    incoming: &crate::apps::ingest::IngestedSets,
) -> usize {
    let dropped = dropped_app_sets(existing, incoming);
    if dropped.is_empty() {
        return 0;
    }
    let bundles: usize = dropped.iter().map(|(_, n)| *n).sum();
    for (set, n) in &dropped {
        eprintln!(
            "publish: WARNING — app set {set:?} ({n} bundle(s)) is already published here but \
             is NOT in this publish; it will be REMOVED"
        );
    }
    eprintln!(
        "publish: {} app set(s) / {bundles} bundle(s) will disappear. Pass \
         --ingest-apps=<dir> to carry them, or they are gone from this tree.",
        dropped.len()
    );
    dropped.len()
}

/// Exit code for a `--verify` that found a defect in the published tree.
/// Shares the value with [`PLAN_DESTRUCTIVE_EXIT`] on purpose: from a gate's
/// point of view both mean *"the tool ran fine and the answer is no"*, which is
/// the distinction that matters — `1` stays "could not run".
pub const VERIFY_DEFECT_EXIT: u8 = 2;

/// `--verify`: walk a **published output directory** and prove the two-hop
/// chain resolves — every `.bin` pointer cracks, names a blob that exists, and
/// whose bytes hash to the value the pointer claims.
///
/// **This is what "is the tree clean" means, mechanically.** Content-addressing
/// is only a *claim* until someone recomputes the hash.
///
/// **The paragraph that used to sit here said this publisher emits no signed
/// root, so content-addressing was "all the integrity there is".** That has been
/// false since B14: [`verify_signed_root`] runs at the end of this very function,
/// the emitter projects a root ([`signed_root::RootProjector`]), and the browser
/// consumes one ([`signed_fetch`]). So verification here covers **both** halves —
/// internal consistency *and* authenticity against the publisher key, plus the
/// closure being walkable. A tree with no signed root is now reported as
/// POINTER-TRUSTED rather than treated as the norm.
///
/// The browser already recomputes every hash on every fetch
/// ([`http_poll::verify_and_decode`] — a body that does not hash to its address
/// is rejected). This runs the **same function** over a directory instead of a
/// network, so a tree can be proven before it is uploaded rather than
/// discovered broken by a visitor.
///
/// Two defect classes, and they are not the same severity:
///
/// - **Broken** — a pointer that does not crack, names a missing blob, or names
///   a blob whose bytes do not hash to it. A reader hits this as a failed page.
/// - **Non-canonical** — a blob that hashes correctly but whose stored bytes are
///   not the canonical pre-image. **This is stricter than the browser**, on
///   purpose: `verify_and_decode` decodes with `ciborium::from_reader`, which
///   stops at the first complete CBOR item and *ignores trailing bytes*, then
///   hashes the canonical re-encode — so appended garbage validates. That is
///   safe to *read* (the canonicalized entity is rendered, never the raw bytes)
///   but it means **identical address does not imply identical bytes**, and a
///   publisher owns its store and should notice a blob that quietly grew.
/// - **Orphan** — a blob under `content/` that no pointer references. Harmless
///   to a reader (nothing links there) but it is dead weight the sync will keep
///   re-uploading, and it is the shape a missing prune leaves behind.
///
/// Exit `0` = clean, [`VERIFY_DEFECT_EXIT`] = broken pointers found, `1` = could
/// not run. **Orphans alone do not fail** — they are a hygiene report, and a
/// tree mid-blue-green legitimately holds blobs its pointers have not adopted
/// yet.
///
/// [`signed_root::RootProjector`]: super::signed_root::RootProjector
/// [`signed_fetch`]: super::signed_fetch
pub(crate) fn run_verify(
    out_dir: &Path,
    peer_id: &str,
    prefix: &str,
    publisher_key: &entity_crypto::Keypair,
) -> ExitCode {
    use super::http_poll::{content_url, crack_pointer, verify_and_decode};
    use entity_hash::Hash;

    let base = paths::prefixed_root(out_dir, prefix);
    let pointer_root = base.join(peer_id);
    let content_root = base.join("content");

    let mut pointers = Vec::new();
    collect_files(&pointer_root, "bin", &mut pointers);
    // NOTE: the published-root manifest is deliberately NOT a `.bin`, so this
    // sweep never sees it — see `paths::PUBLISHED_ROOT_REL`. It used to be, and
    // this sweep correctly reported it as a BROKEN pointer; the fix was the
    // layout, not an exclusion here. `verify_signed_root` below checks it.
    pointers.sort();

    let mut blobs = Vec::new();
    collect_files(&content_root, "", &mut blobs);
    let all_blobs: std::collections::BTreeSet<std::path::PathBuf> =
        blobs.iter().cloned().collect();
    let mut referenced: std::collections::BTreeSet<std::path::PathBuf> = Default::default();

    let mut broken = 0usize;
    let mut verified = 0usize;

    for p in &pointers {
        let Ok(bytes) = std::fs::read(p) else {
            eprintln!("publish --verify: BROKEN {} — unreadable", p.display());
            broken += 1;
            continue;
        };
        let h = match crack_pointer(&bytes) {
            Ok(h) => h,
            Err(e) => {
                eprintln!(
                    "publish --verify: BROKEN {} — not a hash pointer ({e:?})",
                    p.display()
                );
                broken += 1;
                continue;
            }
        };
        // `content_url` owns the sharded-2-4 layout; deriving it a second time
        // here would be a second source of truth for where a blob lives.
        let rel = content_url("", &h);
        let blob = base.join(rel.trim_start_matches('/'));
        let Ok(body) = std::fs::read(&blob) else {
            eprintln!(
                "publish --verify: BROKEN {} → {} — blob MISSING",
                p.display(),
                h.to_hex()
            );
            broken += 1;
            continue;
        };
        referenced.insert(blob.clone());
        match verify_and_decode(&body, &h) {
            Ok(ent) => {
                // Hash-valid is NOT byte-exact, and a publisher must hold its own
                // store to the tighter standard. `verify_and_decode` decodes with
                // `ciborium::from_reader` (which stops at the first complete CBOR
                // item and IGNORES trailing bytes) and hashes the CANONICAL
                // re-encode — so appended garbage decodes away and validates.
                // That is safe for a *reader* (the canonicalized entity is what
                // gets rendered, never the raw bytes) but it means identical
                // address does not imply identical bytes, and nothing else would
                // ever notice a blob quietly growing. Measured, not assumed:
                // appending one byte to a published blob passed verification
                // until this check existed.
                let canonical = entity_ecf::ecf_for_hash(&ent.entity_type, &ent.data);
                if canonical.as_slice() != body.as_slice() {
                    eprintln!(
                        "publish --verify: NON-CANONICAL {} → {} — hashes correctly but the \
                         stored bytes are not the canonical pre-image ({} stored vs {} \
                         canonical); something mutated this blob",
                        p.display(),
                        h.to_hex(),
                        body.len(),
                        canonical.len()
                    );
                    broken += 1;
                } else {
                    verified += 1;
                }
            }
            Err(e) => {
                eprintln!(
                    "publish --verify: BROKEN {} → {} — content does NOT hash to its address \
                     ({e:?})",
                    p.display(),
                    h.to_hex()
                );
                broken += 1;
            }
        }
    }

    // **EVERY BLOB MUST HASH TO ITS OWN ADDRESS — not only the ones a pointer
    // names.** The loop above checks integrity exactly where a `.bin` points, so
    // the blobs no pointer names were checked for EXISTENCE and never for
    // INTEGRITY. Two kinds live there, and both are load-bearing: HAMT interior
    // nodes (reachable only by hash from the signed root) and the manifest's own
    // content-store copy.
    //
    // Measured on the demo publish, 2026-08-20 — flip one byte in the trie root:
    //
    //   publish --verify: signed root verifies … 1 blob(s) in its closure, 0 missing
    //   publish --verify: 16 pointer(s), 16 verified, 0 broken; 18 blob(s), 0 orphaned
    //   publish --verify: every pointer resolves and every body hashes to its address.
    //   exit 0
    //
    // The closure silently collapsed **16 blobs → 1** and still reported `0
    // missing`, because a node that fails to decode declares no children and a
    // shorter walk was not a failure. Meanwhile a pinned consumer resolves
    // NOTHING from that tree — our own now refuses those bytes outright
    // (`entity-core-rust` 302b7f4 + cache admission). So the publisher's last
    // gate said clean about a tree no visitor can read.
    //
    // The check is total and needs no reachability analysis: the filename **is**
    // the wire hash, so a blob is self-describing and any file under `content/`
    // can be checked against its own name. That deliberately subsumes the trie
    // nodes, the manifest copy, and orphans in one rule rather than enumerating
    // which blobs deserve integrity.
    //
    // **Orphan leniency does NOT extend here.** "Nothing links to it, so no
    // reader can hit it" is an argument about *reachability*; it says nothing
    // about bytes that do not match their own address, which no legitimate emit
    // produces. A stale mid-cutover blob still hashes correctly.
    //
    // Sixth appearance of the seam AGENTS.md tracks (B14's `Ok(None)`, verify
    // over an incomplete closure, the structural fix that could not fire, the
    // terminal-vs-retryable split, the tampered interior node): **a shorter walk
    // and a clean one keep arriving as the same value.**
    for blob in &all_blobs {
        if referenced.contains(blob) {
            continue; // already hash-checked through the pointer that names it
        }
        let Some(hex) = blob.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // A file whose name is not a wire hash is not a content blob at all;
        // leave it to the orphan report rather than inventing a defect for it.
        let Ok(raw) = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
            .collect::<Result<Vec<u8>, _>>()
        else {
            continue;
        };
        let Ok(h) = Hash::from_bytes(&raw) else { continue };
        let Ok(body) = std::fs::read(blob) else {
            eprintln!("publish --verify: BROKEN {} — unreadable", blob.display());
            broken += 1;
            continue;
        };
        if let Err(e) = verify_and_decode(&body, &h) {
            eprintln!(
                "publish --verify: BROKEN {} — content does NOT hash to its address ({e:?}). No \
                 pointer names this blob, so it is a trie interior node or the manifest's own \
                 copy: a pinned consumer walking the signed root resolves NOTHING past it.",
                blob.display()
            );
            broken += 1;
        }
    }

    // B14: the signed root, and the blobs only IT references.
    //
    // **Why this belongs in the same pass as the orphan report.** A HAMT
    // interior node appears in no pointer and in no listing — it is reachable
    // only by hash from the signed root — so before this block every one of them
    // was reported as an orphan, and the hygiene report would have told an
    // operator to delete exactly the blobs that make the tree walkable.
    let root_report = verify_signed_root(&base, peer_id, publisher_key, &mut referenced);
    match &root_report {
        SignedRootCheck::Absent => eprintln!(
            "publish --verify: no signed root at {}/{} — this tree is POINTER-TRUSTED: a \
             consumer has to trust whoever serves it. (Not a defect; an --html-only or \
             pre-B14 publish is a legitimate state.)",
            peer_id,
            crate::content_site::signed_root::PUBLISHED_ROOT_REL,
        ),
        SignedRootCheck::Verified { root_hex, seq, closure, missing } => {
            eprintln!(
                "publish --verify: signed root verifies against the publisher key — trie root \
                 {} (seq {seq}), {closure} blob(s) in its closure, {missing} missing",
                &root_hex[..16.min(root_hex.len())],
            );
        }
        SignedRootCheck::Broken(why) => {
            eprintln!("publish --verify: BROKEN signed root — {why}");
        }
    }

    let orphans: Vec<&std::path::PathBuf> =
        all_blobs.iter().filter(|b| !referenced.contains(*b)).collect();

    eprintln!(
        "publish --verify: {} pointer(s), {verified} verified, {broken} broken; {} blob(s), {} \
         orphaned",
        pointers.len(),
        all_blobs.len(),
        orphans.len()
    );
    for o in orphans.iter().take(20) {
        eprintln!("publish --verify:   orphan {}", o.display());
    }
    if orphans.len() > 20 {
        eprintln!("publish --verify:   … and {} more", orphans.len() - 20);
    }

    if pointers.is_empty() {
        eprintln!(
            "publish --verify: no .bin pointers under {} — nothing to verify. (An --html-only \
             publish emits no entity-native tree; verify the origin you actually shipped.)",
            pointer_root.display()
        );
    }

    // **AN EMPTY TREE IS NOT A CLEAN TREE, and this is the last gate before an
    // operator ships.** Until this check, `--verify` on a directory holding
    // *nothing* printed "every pointer resolves and every body hashes to its
    // address" and exited 0 — because zero broken out of zero checked is zero
    // broken. Measured on an empty dir: that exact sentence, exit 0.
    //
    // Every way of pointing this verb at the wrong place lands here, and all of
    // them looked clean:
    //   - the wrong directory, or one whose upload never completed;
    //   - the wrong IDENTITY — `publish --verify` on a *registry* tree resolves
    //     the publisher keypair, looks under a peer-id nothing was written for,
    //     and finds an empty pointer root (the trap §3.3 of the publishing guide
    //     already warns about, which until now produced a PASS);
    //   - a containerized run whose OUT fell outside the bind mount (fixed
    //     separately by the Makefile's CHECK_IN_TREE, and this is the half that
    //     would have caught it anyway).
    //
    // Exit 1, not VERIFY_DEFECT_EXIT: nothing here is *defective*, the verifier
    // simply did not run against a tree. That is the documented meaning of 1
    // ("could not run") and keeps 2 meaning "I checked, and it is broken" —
    // two outcomes an operator's `&&` chain must be able to tell apart.
    //
    // This is the same seam AGENTS.md tracks through B14, `--verify` over an
    // incomplete closure, the structural fix that could not fire, the terminal
    // vs retryable split, and the tampered interior node: **"absent" and
    // "verified" keep arriving as the same value.** Here it was the publisher's
    // own last check saying yes about nothing.
    if pointers.is_empty() && matches!(root_report, SignedRootCheck::Absent) {
        eprintln!(
            "publish --verify: NOTHING WAS VERIFIED — no .bin pointers and no signed root under \
             {}. This is not a clean tree, it is an absent one: check the directory, and check \
             that you are using the verb whose identity owns it (`registry --verify` for a \
             registry tree, not `publish --verify`). (exit 1)",
            pointer_root.display()
        );
        // NAME THE TREE THAT IS ACTUALLY THERE. The message above is correct and
        // still reads as "my tree is corrupt" when the truth is "I pointed
        // verify at a different identity" — the two are one flag apart
        // (`DEMO_IDENTITY=1`, `--identity-seed=`, or the standing trap that
        // `make site` and a host-run `entity-browser publish` resolve different
        // ENTITY_DATA_DIRs and so hold different keys).
        //
        // The evidence is sitting in the directory: a publish leaves
        // `sites/{peer}/`, so a tree published under another identity is
        // *visible from here* and can be named. Reporting the peer we resolved
        // beside the peer(s) we found turns a diagnosis into a comparison the
        // operator can make at a glance.
        if let Some(hint) = other_identity_hint(out_dir, prefix, peer_id) {
            eprintln!("publish --verify: {hint}");
        }
        return ExitCode::from(1);
    }

    // A signed root that does not verify, or one whose closure is incomplete, is
    // a DEFECT and not a hygiene note: the tree advertises a trust chain it
    // cannot honour, and a pinned consumer gets `Ok(None)` — "that page does not
    // exist" — for content that is sitting right there. That failure is worse
    // than having no signed root at all, which is why it is fatal here while
    // `Absent` is merely reported.
    let root_broken = match &root_report {
        SignedRootCheck::Broken(_) => 1,
        SignedRootCheck::Verified { missing, .. } if *missing > 0 => *missing,
        _ => 0,
    };

    if broken > 0 || root_broken > 0 {
        if broken > 0 {
            eprintln!(
                // "pointer" was accurate until unreferenced blobs (trie nodes,
                // the manifest copy) started being checked too — those are
                // counted here and are not pointers. Say "entr(ies)", since the
                // per-item lines above already name exactly what each one was.
                "publish --verify: {broken} BROKEN entr{} — this tree is not safe to serve. \
                 (exit {VERIFY_DEFECT_EXIT})",
                if broken == 1 { "y" } else { "ies" }
            );
        }
        if root_broken > 0 {
            eprintln!(
                "publish --verify: the signed root is not walkable as published — a pinned \
                 consumer resolves NOTHING from this tree. (exit {VERIFY_DEFECT_EXIT})"
            );
        }
        ExitCode::from(VERIFY_DEFECT_EXIT)
    } else {
        eprintln!("publish --verify: every pointer resolves and every body hashes to its address.");
        ExitCode::SUCCESS
    }
}

/// What `--verify` found where the signed root should be.
enum SignedRootCheck {
    /// No manifest served. A legitimate state (`--html-only`, or a tree
    /// published before B14) — reported, never fatal.
    Absent,
    Verified {
        /// The **trie root** the manifest commits to — not the manifest's own
        /// hash (the head). Two different hashes; the signature targets the
        /// head, the walk starts at the root.
        root_hex: String,
        seq: u64,
        closure: usize,
        /// Blobs the closure names that are NOT on disk. Any is fatal: the walk
        /// dead-ends as `Ok(None)`, indistinguishable from an absent page.
        missing: usize,
    },
    Broken(String),
}

/// Verify the published root against the **publisher's own key** and mark every
/// blob its closure reaches as referenced.
///
/// Pinning the publisher key is the consumer's job; here we hold the key that
/// signed the tree, so this answers the publisher's question — *did I emit a
/// chain that a pinned consumer can actually walk?* — rather than the
/// consumer's. Those are different questions and only one of them can be
/// answered before upload.
fn verify_signed_root(
    base: &Path,
    peer_id: &str,
    publisher_key: &entity_crypto::Keypair,
    referenced: &mut std::collections::BTreeSet<std::path::PathBuf>,
) -> SignedRootCheck {
    use crate::content_site::signed_root::{DirFetcher, PUBLISHED_ROOT_REL};
    use entity_peer::published_root::{ContentFetcher, PublishedRootClient};

    let manifest_path = base.join(peer_id).join(PUBLISHED_ROOT_REL);
    if !manifest_path.exists() {
        return SignedRootCheck::Absent;
    }

    let client = PublishedRootClient::new(
        DirFetcher::new(base, peer_id),
        publisher_key.public_key_bytes().to_vec(),
        publisher_key.key_type(),
        Some(peer_id.to_string()),
    );
    let root = match client.fetch_root() {
        Ok(r) => r,
        Err(e) => return SignedRootCheck::Broken(format!("{e:?}")),
    };

    // Walk the closure off disk. `signed_root::trie_closure` needs a store; the
    // fetcher IS the store here, so walk the projected blobs directly by the
    // same structure-agnostic rule the projector used.
    let fetcher = DirFetcher::new(base, peer_id);
    let mut seen: std::collections::BTreeSet<String> = Default::default();
    // The root hash is the ONLY one the discovery scan enqueues unconditionally,
    // so this counter alone could only ever see a missing root (audit F8).
    let mut missing = 0usize;
    // Distinct hashes a trie node declares and the projection does not hold.
    // A set, not a counter: a shared sub-node is declared by more than one
    // parent, and reporting it twice would overstate the damage.
    let mut missing_hashes: std::collections::BTreeSet<String> = Default::default();
    let mut queue = vec![root.root_hash];
    while let Some(h) = queue.pop() {
        if !seen.insert(h.to_hex()) {
            continue;
        }
        let Ok(bytes) = fetcher.content(&h) else {
            missing += 1;
            eprintln!(
                "publish --verify: BROKEN signed-root closure — {} is referenced from the root \
                 but not projected",
                h.to_hex()
            );
            continue;
        };
        let rel = super::http_poll::content_url("", &h);
        referenced.insert(base.join(rel.trim_start_matches('/')));
        let Ok(entity) = super::http_poll::verify_and_decode(&bytes, &h) else { continue };

        // Discovery: the 33-byte-window scan mirrors the projector exactly, so
        // the `referenced` set this builds matches what the projector chose to
        // emit and a publish does not start reporting phantom orphans. It is a
        // heuristic — random bytes can parse as a `Hash` — which is why it
        // filters on presence, and **why it cannot detect absence.**
        for w in entity.data.windows(33) {
            if let Ok(c) = entity_hash::Hash::from_bytes(w) {
                if !seen.contains(&c.to_hex()) && fetcher.content(&c).is_ok() {
                    queue.push(c);
                }
            }
        }

        // **Completeness: the structural check, and the one that can actually
        // fail.** Audit F8 — the `missing` counter above was unreachable for
        // every hash below the root, because a candidate was only ever enqueued
        // when it was *already present*. So withholding one interior trie node
        // shortened the closure and reported `0 missing`: measured on a 24-name
        // registry, 56 → 54 blobs "in its closure", exit 0. The check existed,
        // was recorded as landed, and could not fire. That is the fourth time
        // this seam has turned "withheld" into "absent" (B14's `Ok(None)`, the
        // upstream `collect_bindings_into` short walk, and this).
        //
        // A heuristic scan cannot close it: it has no way to tell a hash that is
        // missing from bytes that merely look like a hash. Structure can. A trie
        // node DECLARES its children, so gate on the entity type and decode —
        // the decoder is upstream and public, so "a publisher must not need to
        // know the HAMT encoding" (the old comment here) buys nothing and cost
        // us the guard.
        if entity.entity_type != entity_tree::trie::TYPE_TREE_SNAPSHOT_NODE {
            continue;
        }
        let Some(node) = entity_tree::trie::SnapshotNodeData::from_cbor(&entity.data) else {
            continue;
        };
        for child in node.data.iter().flat_map(|e| match e {
            entity_tree::trie::Entry::Bucket(b) => {
                b.iter().map(|(_k, vh)| *vh).collect::<Vec<_>>()
            }
            entity_tree::trie::Entry::Link(sub) => vec![*sub],
        }) {
            if fetcher.content(&child).is_ok() {
                // Enqueue even if the window scan already found it — `seen`
                // dedupes, and relying on the heuristic to have spotted a
                // declared child is how a branch goes unwalked.
                queue.push(child);
            } else if missing_hashes.insert(child.to_hex()) {
                eprintln!(
                    "publish --verify: BROKEN signed-root closure — {} is DECLARED by trie node \
                     {} but not projected. A consumer walking from the signed root stops here, \
                     and every key below it reads as \"does not exist\".",
                    child.to_hex(),
                    h.to_hex()
                );
            }
        }
    }
    let missing = missing + missing_hashes.len();

    // The head entity and its signature are referenced too — by the manifest
    // file, not by any pointer. Without this they read as orphans, and an
    // operator following the hygiene report would delete the trust chain.
    if let Ok(bytes) = std::fs::read(&manifest_path) {
        if let Ok(head_entity) = entity_wire::decode_entity(&bytes) {
            let rel = super::http_poll::content_url("", &head_entity.content_hash);
            referenced.insert(base.join(rel.trim_start_matches('/')));
        }
    }
    let sig_ptr = base.join(peer_id).join("system/signature");
    let mut sig_ptrs = Vec::new();
    collect_files(&sig_ptr, "bin", &mut sig_ptrs);
    for p in sig_ptrs {
        if let Ok(bytes) = std::fs::read(&p) {
            if let Ok(h) = super::http_poll::crack_pointer(&bytes) {
                let rel = super::http_poll::content_url("", &h);
                referenced.insert(base.join(rel.trim_start_matches('/')));
            }
        }
    }

    SignedRootCheck::Verified {
        root_hex: root.root_hash.to_hex(),
        seq: root.seq,
        closure: seen.len(),
        missing,
    }
}

/// Every file below `dir` whose extension is `ext` (empty `ext` = all files),
/// recursively. An absent directory yields nothing — an `--html-only` tree has
/// no pointer root, and that is a reportable state, not an error here.
fn collect_files(dir: &Path, ext: &str, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_files(&p, ext, out);
        } else if ext.is_empty()
            || p.extension().and_then(|s| s.to_str()) == Some(ext)
        {
            out.push(p);
        }
    }
}

/// Exit code for a `--plan` whose publish would **remove** something. Distinct
/// from [`ExitCode::FAILURE`] (a real error) so a caller can tell "this plan is
/// destructive, confirm it" from "the plan could not be produced" — the `diff`
/// convention. See [`run_plan`].
pub const PLAN_DESTRUCTIVE_EXIT: u8 = 2;

/// `--plan`: report what this publish would do to the existing projection, and
/// **write nothing**.
///
/// **Why this exists.** A publish is whole-tree-per-`(prefix, identity)`: the
/// projection roots are removed wholesale before rewriting, so publishing a
/// subset of the source set deletes the rest, and re-publishing in place opens a
/// destructive window on a live tree. Both are recoverable — *if* you know
/// before you run. Nothing let you know before you ran.
///
/// **Deliberately mechanism, not policy.** It reports; it does not decide. The
/// process that consumes it — which sources are canonical, whether a removal is
/// intended, when to cut over — belongs to whoever operates the pipeline, and
/// this is backend-agnostic on purpose: it reads the output directory and says
/// what would change there, whatever produced it and wherever it will be synced.
///
/// **Exit-code contract** (the interface a gate binds to — keep it stable):
///
/// | code | meaning |
/// |---|---|
/// | `0` | the plan removes nothing — safe to publish |
/// | [`PLAN_DESTRUCTIVE_EXIT`] (2) | the plan **removes** at least one site **or app set** |
/// | `1` | the plan could not be produced (a real error, upstream of here) |
///
/// So `publish --plan … || confirm` is a working gate with no extra flags, and a
/// legitimate removal is *reported*, never refused — a plan that could veto a
/// deliberate act would just get bypassed.
///
/// **THE PLAN MUST COVER EVERY NAMESPACE THE PUBLISH REPLACES, not just the one
/// it was written for.** This started as a sites-only accounting while the same
/// clean was already removing `{peer}/apps/**`, so the exact sequence an
/// operator runs — plan a content fix, forget `--ingest-apps`, publish —
/// reported *"nothing would be removed"* and then deleted every app bundle on
/// the domain. **A gate that exits 0 is trusted, which makes a blind spot in
/// one worse than having no gate at all.** If a future emitter adds a third
/// subgraph under the peer prefix, it owes a term here in the same commit; the
/// clean is wholesale, so anything it can delete is in scope by construction.
fn run_plan(
    out_dir: &Path,
    peer_id: &str,
    sites: &[read::OwnedSite],
    prefix: &str,
    bare_root: bool,
    app_sets: &crate::apps::ingest::IngestedSets,
    emit_bin: bool,
) -> ExitCode {
    // Bare-root renders ONE site at the domain root and owns no `sites/{peer}/`
    // projection, so the add/remove comparison has no subject there. Say so
    // rather than printing a confidently empty plan.
    if bare_root {
        eprintln!(
            "publish --plan: --bare-root renders a single site at the domain root and does not \
             manage a sites/{{peer}}/ projection — there is no add/remove set to plan."
        );
        return ExitCode::SUCCESS;
    }

    let existing = projected_site_ids(out_dir, peer_id, prefix);
    let removed = dropped_site_ids(&existing, sites);
    let added: Vec<&str> = sites
        .iter()
        .map(|s| s.site_id.as_str())
        .filter(|id| !existing.iter().any(|e| e == id))
        .collect();
    let kept = sites.len() - added.len();

    let where_ = paths::prefixed_root(out_dir, prefix).join(SITE_URL_PREFIX).join(peer_id);
    eprintln!("publish --plan: {} (peer {peer_id})", where_.display());
    eprintln!(
        "publish --plan: {} present, {} incoming — {kept} kept, {} added, {} REMOVED",
        existing.len(),
        sites.len(),
        added.len(),
        removed.len()
    );
    for id in &added {
        eprintln!("publish --plan:   + {id}");
    }
    for id in &removed {
        eprintln!("publish --plan:   - {id}  (REMOVED — not in this publish)");
    }

    // The apps axis. Only when emitting `.bin`: `--html-only` never cleans
    // `{base}/{peer}/`, so it cannot remove an app set and saying it would be
    // a false alarm on the one mode that is safe.
    let app_removed: Vec<(String, usize)> = if emit_bin {
        let existing_apps = projected_app_sets(out_dir, peer_id, prefix);
        let dropped: Vec<(String, usize)> =
            dropped_app_sets(&existing_apps, app_sets).into_iter().cloned().collect();
        let incoming_apps: usize =
            app_sets.values().filter(|i| !i.catalog.entries.is_empty()).count();
        if !existing_apps.is_empty() || incoming_apps > 0 {
            eprintln!(
                "publish --plan: {} app set(s) present, {incoming_apps} incoming — {} REMOVED",
                existing_apps.len(),
                dropped.len()
            );
            for (set, n) in &dropped {
                eprintln!(
                    "publish --plan:   - apps/{set}  ({n} bundle(s) REMOVED — not in this \
                     publish; pass --ingest-apps)"
                );
            }
        }
        dropped
    } else {
        Vec::new()
    };

    if removed.is_empty() && app_removed.is_empty() {
        eprintln!("publish --plan: nothing would be removed.");
        ExitCode::SUCCESS
    } else {
        if !removed.is_empty() {
            eprintln!(
                "publish --plan: {} site(s) would be REMOVED. A publish replaces the whole \
                 projection under this prefix — ingest the complete source set, or publish this \
                 one under its own --prefix.",
                removed.len()
            );
        }
        if !app_removed.is_empty() {
            let bundles: usize = app_removed.iter().map(|(_, n)| *n).sum();
            eprintln!(
                "publish --plan: {} app set(s) / {bundles} bundle(s) would be REMOVED. The \
                 clean covers {peer_id}/apps/** too — re-run with --ingest-apps=<dir> to carry \
                 them.",
                app_removed.len()
            );
        }
        eprintln!("publish --plan: (exit {PLAN_DESTRUCTIVE_EXIT})");
        ExitCode::from(PLAN_DESTRUCTIVE_EXIT)
    }
}

#[allow(clippy::too_many_arguments)] // cohesive publish knobs; a struct would just shuffle them
fn run_projection(
    out_dir: &Path,
    peer_id: &str,
    sites: &[read::OwnedSite],
    prefix: &str,
    live_base: Option<&str>,
    emit_bin: bool,
    deploy_spec: Option<DeployConfigSpec>,
    app_sets: &crate::apps::ingest::IngestedSets,
    publisher_key: entity_crypto::Keypair,
    strict_links: bool,
) -> ExitCode {
    // Publish-safety: surface dangling nav links before we project (the nav is
    // authored separately from the pages, so a stale entry otherwise ships a
    // silent 404). Warn, don't fail — the site still publishes.
    warn_dangling_nav_links(sites);

    // Publish-safety: the clean below is wholesale, so a publish carrying a
    // SUBSET of the source set silently deletes the rest. Read what is already
    // projected FIRST (afterwards there is nothing to compare against) and name
    // every site about to vanish. Warn, don't fail — removing a site is a
    // legitimate publish.
    warn_replaced_sites(&projected_site_ids(out_dir, peer_id, prefix), sites);
    // Same read-before-the-clean rule, for the apps subgraph the clean also
    // covers. Only when emitting `.bin` — `--html-only` leaves `{peer}/` alone.
    if emit_bin {
        warn_replaced_app_sets(&projected_app_sets(out_dir, peer_id, prefix), app_sets);
    }

    // Clean only the projection roots we own, UNDER the hosting prefix — the
    // `.html` tree (`{out}/{prefix}/sites`) and, when emitting it, the `.bin`
    // content data (`{out}/{prefix}/content` + `{out}/{prefix}/{peer}`). Not the
    // whole output dir, so publishing into a populated dir doesn't nuke
    // unrelated files (e.g. a sibling peer's prefix), while stale artifacts
    // don't linger.
    let base = paths::prefixed_root(out_dir, prefix);
    // **The §3.3a sequence marker lives in what we are about to delete.** The
    // clean below removes `{base}/{peer}/`, which holds `system/peer/
    // published-root` — the only record of what `seq` this directory has reached.
    // Read it FIRST and carry it across, or every republish emits `seq 0` and a
    // consumer's rollback floor can never see a rollback (it did, for the whole
    // arc: two different trees, same key, both zero).
    let carried_head = match crate::content_site::signed_root::read_prior_head(&base, peer_id) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("publish: {e}");
            return ExitCode::FAILURE;
        }
    };
    // **Peers already published at THIS base, other than us.** The publish
    // layout is peer-scoped where it matters — `{peer}/…` and `sites/{peer}/…` —
    // but `content/` and the `sites/` parent are **shared at a prefix root**, so
    // a clean scoped to the *prefix* rather than to the *peer* deletes a
    // sibling's bytes. The comment above used to say this clean protects "a
    // sibling peer's prefix", and it does: what it did not protect is a sibling
    // peer at the SAME prefix, which is the topology where several publishers
    // share one origin and their trees tell them apart.
    //
    // Measured 2026-09-03, two peers published into one out-dir with no prefix:
    // peer A's signature blob was deleted, its `sites/` projection was gone, and
    // `publish --verify` on A reported *"1 BROKEN entry — this tree is not safe
    // to serve"* and *"a pinned consumer resolves NOTHING from this tree."*
    // Publishing B destroyed A.
    let siblings: Vec<String> = projected_peer_ids(out_dir, prefix)
        .into_iter()
        .filter(|p| p != peer_id)
        .collect();

    // `sites/{peer}` — not `sites/`. Our projection only; a sibling's stays.
    let mut clean: Vec<std::path::PathBuf> = vec![base.join(SITE_URL_PREFIX).join(peer_id)];
    if emit_bin {
        clean.push(base.join(peer_id));
        // **`content/` is a SHARED, content-addressed store** — design §7:
        // *"a hash is a self-certifying name, so two peers referencing the same
        // hash are referencing the same bytes, and neither can affect the other
        // by writing."* Writing into it is therefore always safe; **deleting
        // from it is not**, and this clean is a republish-hygiene mechanism that
        // predates anyone publishing two peers here.
        //
        // With a sibling present we do not delete it. The cost is that this
        // peer's own superseded blobs accumulate as orphans — which
        // `publish --verify` already enumerates, and which §7's origin-wide GC
        // (keep-set = the union across every peer's retained generations) is the
        // real answer to. **Accumulating bytes is recoverable; deleting another
        // publisher's signature is not.**
        if siblings.is_empty() {
            clean.push(base.join("content"));
        }
    }
    // D13: a publish that is sharing a hosting scope must say so, and say what
    // that costs. Silence here is how the destructive version went unnoticed —
    // the operator saw a clean, successful publish and a tree that no longer
    // served.
    if !siblings.is_empty() {
        println!(
            "  shared hosting scope: {} other publisher(s) already at this {} — \
             cleaning only this peer's tree and projection, leaving the shared \
             content store alone (superseded blobs accumulate as orphans; \
             `--verify` lists them)",
            siblings.len(),
            if prefix.is_empty() { "origin root".to_string() } else { format!("prefix /{prefix}") }
        );
        println!(
            "  NOTE: `transport-profile` and `sites/index.html` are ONE artifact per \
             hosting scope and this publish rewrites both — they will describe this \
             peer. Give each publisher its own --prefix if that matters."
        );
    }

    for root in clean {
        if root.exists() {
            if let Err(e) = std::fs::remove_dir_all(&root) {
                eprintln!("publish: could not clean {}: {e}", root.display());
                return ExitCode::FAILURE;
            }
        }
    }

    // [B1] Legacy-web `.html` projection.
    let report = match static_export::export_owned_sites(out_dir, sites, prefix, live_base) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("publish: HTML export failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    let pages = report.pages;
    // Publish-safety: a `site:`/`entity://` target outside this export set was
    // rewritten to a path under THIS peer, where nothing exists. **Refuses by
    // default** since 2026-08-23, when the authored links this used to warn
    // about were swept upstream; `--allow-out-of-set-links` is the way back.
    if !report.dangling.is_empty() && !warn_out_of_set_links(&report.dangling, strict_links) {
        return ExitCode::FAILURE;
    }

    // [B2] Entity-native `.bin` content data (the form a live peer ingests).
    if emit_bin {
        // B14: every entity written below is recorded into the projector, and
        // the signed root is built over exactly that set at the end. The
        // projector is fed by `write_entity` itself (the choke point), so a
        // future emitter that adds a subgraph gets committed to for free rather
        // than silently publishing content outside the root.
        let mut root = match crate::content_site::signed_root::RootProjector::new(publisher_key) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("publish: signed-root publisher failed: {e}");
                return ExitCode::FAILURE;
            }
        };
        // Continue this directory's sequence (the bytes read before the clean).
        if let Some(raw) = &carried_head {
            if let Err(e) = root.adopt_prior_head_bytes(raw) {
                eprintln!("publish: {e}");
                return ExitCode::FAILURE;
            }
        }
        if let Err(e) = crate::content_site::publish_fixture::emit_owned_sites(
            out_dir,
            sites,
            prefix,
            Some(&mut root),
        ) {
            eprintln!("publish: content-data (.bin) export failed: {e}");
            return ExitCode::FAILURE;
        }
        // App sets (games, apps, …) ride along on the same `.bin` content data —
        // emitted under the SAME publish peer, so the live window fetches
        // `{peer}/apps/{set}/…` from the same origin as the sites. Read off the
        // tree (every available app set), so EVERY publish carries every set, no
        // flag required.
        for (set, ing) in app_sets {
            if ing.catalog.entries.is_empty() {
                continue;
            }
            match crate::content_site::publish_fixture::emit_app_set(
                out_dir,
                peer_id,
                set,
                &ing.catalog,
                &ing.bundles,
                prefix,
                Some(&mut root),
            ) {
                Ok(n) => println!("  apps[{set}]: {n} bundle(s) → {}/apps/{}/", peer_id, set),
                Err(e) => {
                    eprintln!("publish: app-set '{set}' (.bin) export failed: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }

        // The root is signed LAST — after sites and every app set — because it
        // must commit to the whole projection. Signing per-subgraph would emit
        // a root that is already stale by the time the next one lands.
        let base = paths::prefixed_root(out_dir, prefix);
        match root.finish(&base) {
            Ok(r) => println!(
                "  signed root: {} (seq {}, {} keys, {} trie nodes) → {}/{}",
                &r.head_hex[..16.min(r.head_hex.len())],
                r.seq,
                r.keys,
                r.trie_nodes,
                peer_id,
                crate::content_site::signed_root::PUBLISHED_ROOT_REL,
            ),
            Err(e) => {
                eprintln!("publish: signed root failed: {e}");
                return ExitCode::FAILURE;
            }
        }

        // The endpoint, beside the site — emitted only here, after a root was
        // signed, because it advertises `signed_pointer` and that obliges the
        // closure to be present. Without it a consumer has nothing to discover
        // and must fall back to our convention, which §6.5.3 v1.8 forbids.
        let profile_origin = deploy_origin(live_base, prefix);
        if let Err(e) =
            crate::content_site::signed_root::write_transport_profile(&base, peer_id, &profile_origin)
        {
            eprintln!("publish: transport profile emit failed: {e}");
            return ExitCode::FAILURE;
        }
        println!(
            "  transport profile: {} → {}",
            if profile_origin.is_empty() { "(same-origin)" } else { &profile_origin },
            crate::content_site::signed_root::TRANSPORT_PROFILE_REL,
        );
    }

    // Cut 2b: emit the per-domain deployment config alongside the content, so a
    // generic SPA bundle served from this origin boots into the published home.
    let config_path = match deploy_spec {
        Some(spec) => match emit_deployment_config(out_dir, peer_id, sites, &spec) {
            Ok(p) => Some(p),
            Err(e) => {
                eprintln!("publish: deployment-config emit failed: {e}");
                return ExitCode::FAILURE;
            }
        },
        None => None,
    };

    report_projection(out_dir, peer_id, sites, pages, emit_bin, prefix);
    if let Some(p) = config_path {
        println!("  deployment config: {}", p.display());
    }
    ExitCode::SUCCESS
}

/// Emit `{out}/entity-deployment.json` (cut 2b) — the per-domain config that
/// points a generic SPA bundle at this publish. The home `peer` is the publish
/// peer-id (content lives under `sites/{peer}/…`); `origins[peer]` is the
/// serving origin (`""` = same-origin, the SPA expands it at runtime). The
/// startup `surface` is emitted directly (with `window_type` for a window), and
/// a locked overlay kiosk (`surface=site --locked`) spells out the granular
/// `site_mode` + `peer_creation_enabled` explicitly — there is no preset
/// bundling anymore, so the file carries exactly what the consumer applies. The
/// output is round-trip-verified by `DeploymentConfig::parse` in tests.
fn emit_deployment_config(
    out_dir: &Path,
    peer_id: &str,
    sites: &[read::OwnedSite],
    spec: &DeployConfigSpec,
) -> std::io::Result<PathBuf> {
    // Home site: explicit `--config-site` (must exist), else demo, else first.
    let site_id = match &spec.site {
        Some(id) => id.clone(),
        None => pick_bare_site(sites, None).map(|s| s.site_id.clone()).unwrap_or_default(),
    };

    let mut origins = serde_json::Map::new();
    origins.insert(peer_id.to_string(), serde_json::Value::String(spec.origin.clone()));

    let mut doc = serde_json::json!({
        "surface": spec.surface,
        "home_site": { "peer": peer_id, "site": site_id, "loc": "" },
        "origins": origins,
    });
    let obj = doc.as_object_mut().expect("json! built an object");
    // The §7.4 preload: the registry this deployment seeds. Emitted only when
    // asked for — a build that ships no pin is the fail-closed default, and an
    // empty object here would read as "a pin that resolves nothing".
    if let Some(pin) = &spec.registry_pin {
        obj.insert(
            "name_registry_pin".into(),
            serde_json::json!({ "peer_id": pin.peer_id, "origin": pin.origin }),
        );
    }
    // Window surface carries its window type.
    if spec.surface == "window" {
        obj.insert("window_type".into(), serde_json::Value::String(spec.window_type.clone()));
    }
    // Emit the overlay posture (`site_mode`) explicitly per surface — the surface
    // field alone no longer bundles it. A `window` / `chrome` deployment turns the
    // overlay **off**: its content lives in the window (or the workspace), so the
    // status-bar "View Site" toggle into the overlay is redundant, and on a fresh
    // peer it resolves the overlay's default home to a MISSING local site
    // (`No site manifest at 'demo'` — bug report 2026-07-02). Only a `site`
    // deployment enables the overlay.
    match spec.surface.as_str() {
        // Locked kiosk overlay: on, no toggle, no peer creation.
        "site" if spec.locked => {
            obj.insert(
                "site_mode".into(),
                serde_json::json!({ "enabled": true, "show_toggle": false, "locked": true }),
            );
            obj.insert("peer_creation_enabled".into(), serde_json::Value::Bool(false));
        }
        // Escapable overlay: on, toggle shown, unlocked.
        "site" => {
            obj.insert(
                "site_mode".into(),
                serde_json::json!({ "enabled": true, "show_toggle": true, "locked": false }),
            );
        }
        // window / chrome: overlay OFF — no "View Site" toggle in the status bar.
        _ => {
            obj.insert(
                "site_mode".into(),
                serde_json::json!({ "enabled": false, "show_toggle": false }),
            );
        }
    }

    let path = out_dir.join("entity-deployment.json");

    // MERGE onto whatever this domain already declares — never clobber it. See
    // `HomeClaim` for what a clobber cost and why the default is to defer.
    let existing = read_existing_deployment(&path)?;
    let existing_home = existing.as_ref().and_then(|o| {
        o.get("home_site")
            .and_then(|v| v.as_object())
            .and_then(|h| h.get("peer"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    });
    let claim = home_claim(existing_home.as_deref(), peer_id, spec.set_home);

    let merged = match (&claim, existing) {
        // This publish defines or rewrites the domain's own fields. Keep any
        // origins siblings already registered — they are other peers' routing
        // facts and none of this publish's business.
        (HomeClaim::Defines | HomeClaim::Republishes | HomeClaim::Takes { .. }, prior) => {
            let mut out = doc.as_object().expect("json! built an object").clone();
            if let Some(prior) = prior {
                if let Some(prior_origins) = prior.get("origins").and_then(|v| v.as_object()) {
                    let out_origins =
                        out.get_mut("origins").and_then(|v| v.as_object_mut()).expect("origins");
                    for (k, v) in prior_origins {
                        if k != peer_id {
                            out_origins.insert(k.clone(), v.clone());
                        }
                    }
                }
            }
            out
        }
        // A secondary peer. Touch exactly one key.
        (HomeClaim::Defers { .. }, Some(mut prior)) => {
            let origins = prior
                .entry("origins")
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            if !origins.is_object() {
                *origins = serde_json::Value::Object(serde_json::Map::new());
            }
            origins
                .as_object_mut()
                .expect("origins is an object")
                .insert(peer_id.to_string(), serde_json::Value::String(spec.origin.clone()));
            prior
        }
        // Unreachable: `Defers` is only produced from an existing document.
        (HomeClaim::Defers { .. }, None) => {
            unreachable!("home_claim cannot defer with no existing document")
        }
    };

    // D13: say what was done to a document this publish did not author. Silence
    // here is how the clobber went unnoticed for as long as it did.
    match &claim {
        HomeClaim::Defers { to } => println!(
            "  deployment config: MERGED as a secondary peer — this domain's home stays \
             {to} (pass --set-home to move it)"
        ),
        HomeClaim::Takes { from } => println!(
            "  deployment config: HOME MOVED from {from} to this peer (--set-home)"
        ),
        HomeClaim::Republishes | HomeClaim::Defines => {}
    }

    std::fs::write(&path, format!("{}\n", serde_json::to_string_pretty(&merged)?))?;
    Ok(path)
}

/// Read the domain's existing deployment document, if it has one.
///
/// **A malformed document is a hard stop, never an implicit fresh start.** Same
/// rule as `BuildsManifest::from_json`, and for a sharper reason here: starting
/// fresh on an unreadable document is *exactly* the clobber this merge exists to
/// prevent, so the one error path must not quietly perform it. *"There is none"*
/// and *"there is one and I cannot read it"* decide different things.
fn read_existing_deployment(
    path: &Path,
) -> std::io::Result<Option<serde_json::Map<String, serde_json::Value>>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{} exists but is not valid JSON ({e}). Refusing to overwrite it — a domain \
                 document names every peer hosted here, and replacing an unreadable one with \
                 a single-peer document is how the other peers get dropped. Fix or remove it.",
                path.display()
            ),
        )
    })?;
    match value {
        serde_json::Value::Object(o) => Ok(Some(o)),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} exists but is not a JSON object — refusing to overwrite it", path.display()),
        )),
    }
}

/// Bare-root mode: ONE site rendered at the domain root (no prefix, no
/// branding). Picks the site by `--site=ID`, else the demo site, else the
/// first read.
fn run_bare_root(
    out_dir: &Path,
    sites: &[read::OwnedSite],
    site_filter: Option<&str>,
    live_base: Option<&str>,
) -> ExitCode {
    let site = match pick_bare_site(sites, site_filter) {
        Some(s) => s,
        None => {
            eprintln!(
                "publish --bare-root: site {:?} not found (have: {:?})",
                site_filter,
                sites.iter().map(|s| &s.site_id).collect::<Vec<_>>()
            );
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = std::fs::create_dir_all(out_dir) {
        eprintln!("publish: could not create {}: {e}", out_dir.display());
        return ExitCode::FAILURE;
    }
    match static_export::export_bare_root(out_dir, site, live_base) {
        Ok(pages) => {
            report_bare_root(out_dir, site, pages);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("publish: export failed: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Select the bare-root site: explicit `--site=ID` wins, else the demo site,
/// else the first read.
fn pick_bare_site<'a>(
    sites: &'a [read::OwnedSite],
    site_filter: Option<&str>,
) -> Option<&'a read::OwnedSite> {
    if let Some(id) = site_filter {
        return sites.iter().find(|s| s.site_id == id);
    }
    sites
        .iter()
        .find(|s| s.site_id == crate::views::content_site::DEMO_SITE_ID)
        .or_else(|| sites.first())
}

/// Well-known **demo publisher seed** — a fixed 32-byte seed so the demo
/// publishes under a **stable, reproducible peer-id every run**. Before this,
/// `publish` minted a random peer per run, so the static URL
/// (`/sites/{peer}/…`) shifted between publishes and any link/bookmark broke
/// (reported as the ids being scrambled, plus the live→static 404). A fixed seed makes
/// the demo *addressable*. **This is the demo identity, not a real deployment
/// identity** — a real hosting peer loads its own durable keypair (the deferred
/// native peer-load seam below); this just stops the demo's id from drifting.
/// The bytes are arbitrary-but-fixed ("entity-demo-publisher" padded).
const DEMO_PUBLISH_SEED: [u8; 32] = *b"entity-demo-publisher-seed-v1\0\0\0";

/// The *explicit* publisher seed a CLI invocation asked for, if any — the pure,
/// side-effect-free part of identity resolution (so it stays unit-testable
/// without touching the filesystem):
/// - `--identity-seed=<64-hex>` → that specific system seed (same hex form as
///   the runtime `entity_system_seed`); `Err` on malformed hex so a typo fails
///   the build rather than silently publishing under the wrong identity.
/// - `--demo-identity` → the fixed `DEMO_PUBLISH_SEED` (dev/testing only).
/// - neither → `Ok(None)`, meaning "use the durable publisher keypair".
///
/// `--identity-seed` wins over `--demo-identity` (an explicit hex is the most
/// specific request).
fn explicit_publish_seed(args: &[String]) -> Result<Option<[u8; 32]>, String> {
    if let Some(hex) = args.iter().find_map(|a| a.strip_prefix("--identity-seed=")) {
        return crate::vault_codec::hex_to_seed(hex.trim())
            .map(Some)
            .ok_or_else(|| {
                format!(
                    "--identity-seed expects 64 hex chars (a 32-byte system seed), got {:?}",
                    hex
                )
            });
    }
    if args.iter().any(|a| a == "--demo-identity") {
        return Ok(Some(DEMO_PUBLISH_SEED));
    }
    Ok(None)
}

/// Resolve the publisher keypair for this invocation. Default → the **durable
/// publisher identity** ([`crate::persistence::publisher_keypair`], load-or-
/// generate under `{ENTITY_DATA_DIR}/publish/`), so a real deployment publishes
/// under one stable peer-id across runs. `--identity-seed` / `--demo-identity`
/// override it with an explicit seed (see [`explicit_publish_seed`]).
fn resolve_publish_keypair(args: &[String]) -> Result<entity_crypto::Keypair, String> {
    match explicit_publish_seed(args)? {
        Some(seed) => Ok(entity_crypto::Keypair::from_seed(seed)),
        None => Ok(crate::persistence::publisher_keypair()),
    }
}

/// The same resolution for the **registry** verb (B16), defaulting to the
/// separate durable registry identity. Shares `explicit_publish_seed`, so
/// `--identity-seed` / `--demo-identity` mean the same thing on both verbs —
/// including the one case where sharing a key across the two is intended.
pub(crate) fn resolve_registry_keypair(args: &[String]) -> Result<entity_crypto::Keypair, String> {
    match explicit_publish_seed(args)? {
        Some(seed) => Ok(entity_crypto::Keypair::from_seed(seed)),
        None => Ok(crate::persistence::registry_keypair()),
    }
}

/// Resolve which peer to publish and read all its sites. **The seam** (see
/// the module docs): today it builds a Direct peer under the supplied
/// **publisher `keypair`** (durable by default; `--identity-seed` / `--demo-identity`
/// override) and seeds the bundled demo site set, then reads it back off the
/// tree — so publish is a faithful end-to-end exercise of the [A] reader on
/// demo data, under a *stable* peer-id.
/// Replace this body with "open a persisted peer dir → read its real sites"
/// when the durable native peer-load path lands; nothing downstream changes,
/// and the publisher identity becomes the loaded peer's own (still stable).
fn resolve_publish_source(
    keypair: entity_crypto::Keypair,
    ingest_dir: Option<&Path>,
    ingest_apps: Option<&Path>,
) -> Result<(String, Vec<read::OwnedSite>, crate::apps::ingest::IngestedSets), String> {
    let peers = Peers::new_direct_with_keypair(keypair);
    let peer_id = peers.primary_peer_id().to_string();
    match ingest_dir {
        // Real content: ingest a `render/` emit (disk→tree), then read it
        // back through the same path the demo source uses.
        Some(dir) => {
            let ids = super::ingest::ingest_path(&peers, &peer_id, dir)?;
            eprintln!(
                "publish --ingest: ingested {} site(s) from {} — {:?}",
                ids.len(),
                dir.display(),
                ids
            );
        }
        // Default: the bundled demo site set (the SSG / demo generator).
        None => seed_demo_site_set(&peers, &peer_id),
    }
    // App sets (games + apps) ride a publish only when there are real apps to
    // ship: an explicit `--ingest-apps=<dir>` (entity-apps `dist/`) ingests +
    // splits the whole set by type, landing in the tree and read back through
    // the same [A] reader, exactly like sites. Without it, a publish emits NO
    // apps — the launcher's empty-state contract — rather than fake demo
    // placeholders (the baked fixtures are now an e2e-only `demo-apps` gate).
    if let Some(dir) = ingest_apps {
        let n = crate::apps::ingest::ingest_into(&peers, &peer_id, dir)?;
        eprintln!("publish --ingest-apps: ingested {n} app(s) from {}", dir.display());
    }
    let sites = read::read_all_sites(&peers, &peer_id);
    let app_sets = crate::apps::read::read_all_app_sets(&peers, &peer_id);
    Ok((peer_id, sites, app_sets))
}

/// Seed the demo **set** into `peer_id`'s tree: the bundled deep `demo` site
/// ([`crate::views::content_site::ensure_demo_site`]) plus a second
/// `entity-info` site that cross-links into it. Shared by the publish
/// command and the `emit_live_tree_demo` test so both seed identically.
/// Uses the arm-aware [`Peers::seed_write`] router (Direct → sync L0 put, so
/// the same-pass read in [`read::read_all_sites`] sees it).
pub fn seed_demo_site_set(peers: &Peers, peer_id: &str) {
    crate::views::content_site::ensure_demo_site(peers, peer_id);

    let info = SiteManifest::new(
        INFO_SITE_ID,
        "Entity Info",
        "index",
        vec![NavItem::new("Overview", "/index"), NavItem::new("Why", "/why")],
    );
    peers.seed_write(peer_id, paths::manifest_path(peer_id, INFO_SITE_ID), info.to_entity());
    peers.seed_write(
        peer_id,
        paths::page_path(peer_id, INFO_SITE_ID, "index"),
        SitePage::markdown(
            "What is the entity system?",
            "# Entity System\n\nA content-addressed tree projected onto the web. \
             Jump into the [Demo](site:demo/index), or read [Why](./why).\n",
        )
        .to_entity(),
    );
    peers.seed_write(
        peer_id,
        paths::page_path(peer_id, INFO_SITE_ID, "why"),
        SitePage::markdown(
            "Why",
            "# Why\n\nBecause the same site renders from the local tree, a peer, or a CDN.\n\n\
             See the demo's [Guide](site:demo/guide/intro). Back to [Overview](./index).\n",
        )
        .to_entity(),
    );
}

/// Print a human summary of a projection publish + how to view it.
fn report_projection(
    out_dir: &Path,
    peer_id: &str,
    sites: &[read::OwnedSite],
    pages: usize,
    emit_bin: bool,
    prefix: &str,
) {
    println!("published {} site(s), {pages} page(s) → {}", sites.len(), out_dir.display());
    for s in sites {
        println!("  · {} ({} pages) — {}", s.site_id, s.pages.len(), s.manifest.title);
    }
    let forms = if emit_bin {
        "legacy-web .html + entity-native .bin content data"
    } else {
        "legacy-web .html only"
    };
    println!("  forms: {forms}");
    println!("  peer: {peer_id}");
    if !prefix.is_empty() {
        println!("  prefix: {prefix} (hosting scope)");
    }
    // The sites index lands at `{prefix}/sites/` (just `sites/` at the root).
    let view_path = paths::href_prefix(prefix);
    println!(
        "  view: (cd {} && python3 -m http.server 8099) → http://localhost:8099{}/{}/",
        out_dir.display(),
        view_path,
        SITE_URL_PREFIX,
    );
}

/// Print a human summary of a bare-root publish (single site at root).
fn report_bare_root(out_dir: &Path, site: &read::OwnedSite, pages: usize) {
    println!(
        "published 1 site at root (bare-root SSG), {pages} page(s) → {}",
        out_dir.display()
    );
    println!("  · {} — {}", site.site_id, site.manifest.title);
    println!(
        "  view: (cd {} && python3 -m http.server 8099) → http://localhost:8099/",
        out_dir.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The publish source resolver seeds + reads both demo sites whole off
    /// the tree (the [A] read), with the second site's cross-link intact.
    #[test]
    fn publish_source_reads_the_seeded_demo_set() {
        let demo_kp = || entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED);
        let (peer_id, sites, app_sets) = resolve_publish_source(demo_kp(), None, None).unwrap();
        assert!(!peer_id.is_empty());

        // The publisher identity is STABLE across runs for a given seed — the
        // whole point (no more shifting peer-ids / broken permalinks).
        let (peer_id_again, _, _) = resolve_publish_source(demo_kp(), None, None).unwrap();
        assert_eq!(peer_id, peer_id_again, "publish peer-id must be reproducible");

        // A DIFFERENT system seed → a different (but still reproducible) peer-id,
        // so each deployment can publish under its own identity (`--identity-seed`).
        let other_kp = || entity_crypto::Keypair::from_seed(*b"different-publisher-seed-here!!!");
        let (other_pid, _, _) = resolve_publish_source(other_kp(), None, None).unwrap();
        assert_ne!(peer_id, other_pid, "a custom seed must yield its own peer-id");
        let (other_pid_again, _, _) = resolve_publish_source(other_kp(), None, None).unwrap();
        assert_eq!(other_pid, other_pid_again, "custom-seed peer-id must be reproducible");

        let ids: Vec<&str> = sites.iter().map(|s| s.site_id.as_str()).collect();
        assert!(ids.contains(&"demo"), "site ids: {ids:?}");
        assert!(ids.contains(&INFO_SITE_ID), "site ids: {ids:?}");

        // Without `--ingest-apps`, a bare publish emits NO app sets — the
        // empty-state contract: real deployments ingest apps (entity-apps
        // `dist/`) or serve them off an origin, and an app-less publish ships
        // nothing rather than fake demo placeholders. (The baked war/calculator
        // fixtures are now an e2e-only `demo-apps` gate, never published.)
        use crate::apps::paths::{APPS_SET, GAMES_SET};
        assert!(app_sets.get(GAMES_SET).is_none(), "no games without --ingest-apps");
        assert!(app_sets.get(APPS_SET).is_none(), "no apps without --ingest-apps");

        // The bundled demo read with its nested guide page (recursive [A]).
        let demo = sites.iter().find(|s| s.site_id == "demo").unwrap();
        assert!(demo.pages.iter().any(|(slug, _)| slug == "guide/advanced/internals"));

        // The second site's cross-link into the demo is present (proves the
        // multi-site link the exporter rewrites).
        let info = sites.iter().find(|s| s.site_id == INFO_SITE_ID).unwrap();
        assert!(info.pages.iter().any(|(_, p)| p.body.contains("site:demo/guide/intro")));
    }

    /// An out-of-set `site:` link is a **failure by default**, and the bare
    /// argv is what says so.
    ///
    /// This is the gate on a concession being spent rather than forgotten. It
    /// was a warning for one stated reason — refusing would have broken the
    /// per-domain builds shipping at that moment, over seven authored links in
    /// content this repo does not own — with one stated expiry: flip it when
    /// they land. They landed (`entity-core-papers` `ef3f662`; verified here as
    /// 0 remaining across the whole corpus, and all four domains publishing
    /// clean under the refusal). A concession whose reason is gone is just the
    /// design, so the *default* is the assertion.
    #[test]
    fn an_out_of_set_link_is_refused_unless_the_caller_asks_for_a_warning() {
        let argv = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        // The bare publish — the case every pipeline hits, and the one that
        // silently shipped 404s for as long as this was a warning.
        assert!(
            refuses_out_of_set_links(&argv(&[])),
            "the DEFAULT must refuse; a guard nobody opts into is the warning it replaced"
        );
        assert!(
            refuses_out_of_set_links(&argv(&["--ingest=x", "--deployment-config"])),
            "unrelated flags must not weaken the default"
        );

        // The deliberate way back, for a genuinely partial set mid-migration.
        assert!(!refuses_out_of_set_links(&argv(&["--allow-out-of-set-links"])));
        assert!(!refuses_out_of_set_links(&argv(&["--ingest=x", "--allow-out-of-set-links"])));

        // Back-compat: `--strict-links` asked for what is now the default, and
        // other repos' pipelines still pass it. It must keep meaning "refuse"
        // rather than becoming an unknown flag that fails their build.
        assert!(
            refuses_out_of_set_links(&argv(&["--strict-links"])),
            "a caller still passing --strict-links asked for exactly this"
        );
    }

    /// The dangling-nav-link validator flags in-site nav targets with no
    /// backing page (the delete-a-page 404 footgun) and ONLY those — the site
    /// root resolves, cross-site / external targets are left alone, and nested
    /// `children` are walked.
    #[test]
    fn dangling_nav_targets_flags_only_unresolved_in_site_links() {
        use crate::content_site::{NavItem, SiteManifest, SitePage};
        let manifest = SiteManifest::new(
            "s",
            "S",
            "index",
            vec![
                NavItem::new("Home", "/index"),                 // resolves (root page)
                NavItem::new("About", "/about"),                // resolves
                NavItem::new("Gone", "/ghost"),                 // DANGLING
                NavItem::new("Other site", "site:elsewhere/x"), // cross-site — not checked
                NavItem::new("Web", "https://example.com"),     // external — not checked
                NavItem::section(
                    "Guide",
                    "",
                    vec![
                        NavItem::new("Intro", "/guide/intro"),  // resolves
                        NavItem::new("Missing", "/guide/nope"), // DANGLING (nested)
                    ],
                ),
            ],
        );
        let pages = vec![
            ("index".to_string(), SitePage::markdown("Home", "hi")),
            ("about".to_string(), SitePage::markdown("About", "hi")),
            ("guide/intro".to_string(), SitePage::markdown("Intro", "hi")),
        ];
        let site = read::OwnedSite {
            peer_id: "P".into(),
            site_id: "s".into(),
            manifest,
            pages,
            assets: Vec::new(),
        };
        let mut dangling = dangling_nav_targets(&site);
        dangling.sort();
        assert_eq!(
            dangling,
            vec![
                ("Gone".to_string(), "/ghost".to_string()),
                ("Missing".to_string(), "/guide/nope".to_string()),
            ]
        );
    }

    /// A site already projected here but absent from this publish is named as
    /// about to be REMOVED — the "you cannot publish just one thing" trap,
    /// measured on a real tree (7 site dirs → 2 by publishing one site).
    ///
    /// Both directions matter: a publish carrying MORE than is there (a first
    /// publish, or an addition) must warn about nothing, or the signal becomes
    /// noise the operator learns to ignore.
    #[test]
    fn a_site_left_out_of_a_publish_is_named_as_about_to_be_removed() {
        use crate::content_site::{SiteManifest, SitePage};
        let site = |id: &str| read::OwnedSite {
            peer_id: "P".into(),
            site_id: id.into(),
            manifest: SiteManifest::new(id, id, "index", Vec::new()),
            pages: vec![("index".to_string(), SitePage::markdown("I", "hi"))],
            assets: Vec::new(),
        };
        let existing = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];

        // Publishing only `beta` drops the other two.
        assert_eq!(warn_replaced_sites(&existing, &[site("beta")]), 2);
        // The complete set drops nothing.
        assert_eq!(
            warn_replaced_sites(&existing, &[site("alpha"), site("beta"), site("gamma")]),
            0
        );
        // A superset (adding a site) drops nothing either — additions are not
        // removals, and warning on them would train the operator to ignore this.
        assert_eq!(
            warn_replaced_sites(
                &existing,
                &[site("alpha"), site("beta"), site("gamma"), site("delta")]
            ),
            0
        );
        // A first publish into a fresh dir has nothing to lose.
        assert_eq!(warn_replaced_sites(&[], &[site("alpha")]), 0);
    }

    /// `--plan` reports the delta and **writes nothing** — the whole contract.
    /// Driven through the real CLI entry (`run`) so the flag wiring is covered,
    /// not just the helper: a `--plan` that was parsed but not honoured would
    /// publish, and every assertion below would still be about a plan.
    #[test]
    fn plan_reports_the_delta_and_writes_nothing() {
        let tmp = std::env::temp_dir().join(format!("entity-publish-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.to_string_lossy().to_string();
        let plan = |extra: &[&str]| {
            let mut args = vec!["publish".to_string(), out.clone(), "--demo-identity".into()];
            args.extend(extra.iter().map(|s| s.to_string()));
            run(&args)
        };

        // Into an EMPTY dir: nothing present, so nothing can be removed → 0.
        assert_eq!(plan(&["--plan"]), ExitCode::SUCCESS);
        // …and it wrote nothing. This is the assertion that matters: a plan that
        // published would otherwise pass every other check here.
        assert!(
            std::fs::read_dir(&tmp).unwrap().next().is_none(),
            "--plan must not write into the output directory"
        );

        // Now seed a projection holding a site the demo set does NOT carry, so a
        // real publish would delete it, and the plan must say so with the
        // destructive exit code rather than SUCCESS.
        let demo_kp = entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED);
        let (pid, _sites, _apps) = resolve_publish_source(demo_kp, None, None).unwrap();
        let root = tmp.join(SITE_URL_PREFIX).join(&pid);
        std::fs::create_dir_all(root.join("a-site-nobody-is-publishing")).unwrap();

        assert_eq!(plan(&["--plan"]), ExitCode::from(PLAN_DESTRUCTIVE_EXIT));
        // Still no publish: the seeded dir is untouched and no `content/`
        // appeared beside it.
        assert!(root.join("a-site-nobody-is-publishing").is_dir());
        assert!(!tmp.join("content").exists(), "--plan must not emit content/");

        // `--bare-root` manages no sites/{peer}/ projection, so it has no
        // add/remove set — it must say so and NOT inherit the destructive code
        // from a projection it does not own.
        assert_eq!(plan(&["--plan", "--bare-root"]), ExitCode::SUCCESS);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// **The gate said GO while the publish deleted every app bundle.** This
    /// pins the exact three-command sequence measured on a real staging tree
    /// (`HANDOFF-TO-BROWSER-RUST-2026-08-20-PUBLISH` §1): publish WITH apps →
    /// plan the same publish WITHOUT `--ingest-apps` → run it. Step 2 reported
    /// *"1 present, 1 incoming — 1 kept, 0 added, 0 REMOVED · nothing would be
    /// removed"* and exited **0**; step 3 took `apps/` from 35 objects to 0.
    /// The site accounting was right the whole time — `apps/` simply was not in
    /// it, and a gate is trusted in proportion to how quiet it is.
    ///
    /// The last step is what makes this a regression test rather than a
    /// tautology: it proves the removal the plan now predicts is **real**. A
    /// test that only checked the exit code would still pass if the publish had
    /// quietly stopped cleaning apps, which is a different bug wearing this
    /// one's clothes.
    #[test]
    fn a_plan_that_omits_apps_reports_the_bundles_it_would_delete() {
        let tmp =
            std::env::temp_dir().join(format!("entity-publish-plan-apps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let apps_dist = tmp.join("apps-dist");
        std::fs::create_dir_all(&apps_dist).unwrap();
        let out_dir = tmp.join("out");
        std::fs::create_dir_all(&out_dir).unwrap();

        // A dist with BOTH sets, so "some apps survived" cannot be mistaken for
        // success: a per-set bug would leave one behind.
        std::fs::write(
            apps_dist.join("index.json"),
            r#"[{"id":"war","name":"War","description":"w","saves":false,"type":"canvas-game"},
                {"id":"calc","name":"Calc","description":"c","saves":false,"type":"tool"}]"#,
        )
        .unwrap();
        std::fs::write(apps_dist.join("war.html"), "<html>war</html>").unwrap();
        std::fs::write(apps_dist.join("calc.html"), "<html>calc</html>").unwrap();

        let out = out_dir.to_string_lossy().to_string();
        let ingest_apps = format!("--ingest-apps={}", apps_dist.display());
        let call = |extra: &[&str]| {
            let mut args = vec!["publish".to_string(), out.clone(), "--demo-identity".into()];
            args.extend(extra.iter().map(|s| s.to_string()));
            run(&args)
        };

        let demo_kp = entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED);
        let (pid, _sites, _apps) = resolve_publish_source(demo_kp, None, None).unwrap();

        // 1. Publish WITH apps.
        assert_eq!(call(&[&ingest_apps]), ExitCode::SUCCESS);
        let with_apps = projected_app_sets(&out_dir, &pid, "");
        assert_eq!(
            with_apps.iter().map(|(_, n)| *n).sum::<usize>(),
            2,
            "the fixture's two bundles should be projected: {with_apps:?}"
        );

        // 2. Plan the SAME publish WITHOUT apps. This is the step that used to
        //    exit 0 while promising nothing would be removed.
        assert_eq!(
            call(&["--plan"]),
            ExitCode::from(PLAN_DESTRUCTIVE_EXIT),
            "a plan that drops every app set must report the destructive exit"
        );
        // …and planning is still not publishing: the apps are still there.
        assert_eq!(projected_app_sets(&out_dir, &pid, ""), with_apps);

        // 3. Planning the publish you actually intend is clean — or the gate
        //    would cry wolf on every ordinary republish and get bypassed.
        assert_eq!(call(&["--plan", &ingest_apps]), ExitCode::SUCCESS);

        // 4. The removal it predicts is REAL: run the apps-less publish.
        assert_eq!(call(&[]), ExitCode::SUCCESS);
        assert!(
            projected_app_sets(&out_dir, &pid, "").is_empty(),
            "a publish without --ingest-apps really does delete the apps tree — if this ever \
             stops being true the plan above is warning about nothing"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A verify that finds nothing must say **which** identity the tree is
    /// actually under, because "absent tree" reads as "corrupt tree" and the
    /// real cause is one flag away.
    ///
    /// Asserted on the MESSAGE, not the exit code: pointing verify at the wrong
    /// identity and pointing it at an empty directory both exit 1, so an
    /// exit-code test cannot tell a good refusal from a misleading one. That is
    /// the same lesson `parse_registry_args` was rewritten for.
    #[test]
    fn a_verify_that_finds_nothing_names_the_identity_the_tree_is_under() {
        let tmp = std::env::temp_dir()
            .join(format!("entity-publish-verify-hint-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let sites = tmp.join(SITE_URL_PREFIX);

        // An empty tree has no better diagnosis, and must not invent one.
        std::fs::create_dir_all(&sites).unwrap();
        assert_eq!(other_identity_hint(&tmp, "", "PEER-WE-RESOLVED"), None);

        // One other publish → name it.
        std::fs::create_dir_all(sites.join("PEER-THAT-WROTE-IT").join("demo")).unwrap();
        let one = other_identity_hint(&tmp, "", "PEER-WE-RESOLVED").expect("a hint");
        assert!(one.contains("PEER-THAT-WROTE-IT"), "must name what it found: {one}");
        assert!(one.contains("PEER-WE-RESOLVED"), "must name what we resolved: {one}");

        // The peer we resolved is never reported back to us as "another"
        // identity — that would be a hint pointing at the thing we already are.
        std::fs::create_dir_all(sites.join("PEER-WE-RESOLVED")).unwrap();
        let still = other_identity_hint(&tmp, "", "PEER-WE-RESOLVED").expect("a hint");
        assert!(
            !still.contains("under PEER-WE-RESOLVED,"),
            "the resolved peer must not be listed as the other one: {still}"
        );

        // Several → list them all rather than picking one arbitrarily.
        std::fs::create_dir_all(sites.join("PEER-THIRD")).unwrap();
        let many = other_identity_hint(&tmp, "", "PEER-WE-RESOLVED").expect("a hint");
        assert!(many.contains("PEER-THAT-WROTE-IT") && many.contains("PEER-THIRD"), "{many}");

        // Prefix-scoped, like every other read of this layout.
        assert_eq!(other_identity_hint(&tmp, "scope", "PEER-WE-RESOLVED"), None);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// `--html-only` never cleans `{peer}/`, so it cannot remove an app set —
    /// and must not claim it would. A false destructive exit on the one mode
    /// that is safe is how a gate gets routed around with `|| true`.
    #[test]
    fn an_html_only_plan_does_not_claim_it_would_remove_apps() {
        let tmp = std::env::temp_dir()
            .join(format!("entity-publish-plan-htmlonly-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let demo_kp = entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED);
        let (pid, _sites, _apps) = resolve_publish_source(demo_kp, None, None).unwrap();
        // Seed an apps projection by hand — the shape a previous publish left.
        let bundles = tmp.join(&pid).join("apps").join("games").join("bundles");
        std::fs::create_dir_all(&bundles).unwrap();
        std::fs::write(bundles.join("war.bin"), b"x").unwrap();
        assert_eq!(projected_app_sets(&tmp, &pid, ""), vec![("games".to_string(), 1)]);

        let out = tmp.to_string_lossy().to_string();
        let args = vec![
            "publish".to_string(),
            out,
            "--demo-identity".into(),
            "--plan".into(),
            "--html-only".into(),
        ];
        assert_eq!(run(&args), ExitCode::SUCCESS);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// `--verify` proves a real published tree resolves, and **catches a
    /// tampered body** — the property the whole content-addressing story rests
    /// on. Publishes for real, verifies clean, then flips one byte in one blob
    /// and requires the defect exit.
    ///
    /// A verifier that only ever sees good input is not a verifier, so the
    /// corruption half is the point: it is the same `verify_and_decode` the
    /// browser runs per fetch, which is why a clean result here means the
    /// visitor's client will not reject anything either.
    /// **`--verify` writes nothing, so an EMIT-time rule cannot gate it.**
    ///
    /// `make site-dist` publishes and then re-runs the same target with
    /// `VERIFY=1`, inheriting whatever knobs the caller set — so
    /// `make site-dist REGISTRY_PIN=…` published a correct tree and then failed
    /// its own verification pass, on a guard about what an emitted deployment
    /// config would carry. Found by running it, after the flag it is about had
    /// already been tested end to end on `make site`: the two-step target is a
    /// different caller, and the emit knobs it forwards to a read-only pass are
    /// not a set anyone enumerates.
    ///
    /// Both directions, because the guard is worth keeping where it applies: a
    /// pin with no config to carry it is still refused on a real publish.
    #[test]
    fn verify_is_not_gated_by_the_emit_time_rules_it_cannot_violate() {
        let tmp = std::env::temp_dir()
            .join(format!("entity-publish-verify-emit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.to_string_lossy().to_string();
        let call = |extra: &[&str]| {
            let mut args = vec!["publish".to_string(), out.clone(), "--demo-identity".into()];
            args.extend(extra.iter().map(|s| s.to_string()));
            run(&args)
        };
        // A canonical-form peer-id, so nothing here turns on the pin's validity.
        let pin = "--registry-pin=2KGyHLRBuykJPz7XCH9N21ACPZ28K7J5tbwRmpfFoCrrf5";

        assert_eq!(call(&["--deployment-config", pin]), ExitCode::SUCCESS);
        assert_eq!(
            call(&["--verify", pin]),
            ExitCode::SUCCESS,
            "verify reads a tree; a pin it will never write cannot make it fail"
        );
        // The guard still binds on the path that actually emits.
        assert_ne!(
            call(&[pin]),
            ExitCode::SUCCESS,
            "a pin with no --deployment-config to carry it is still refused on a publish"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ── The domain document is DOMAIN-managed, not last-publish-managed ──────

    /// The four claims, enumerated. `Defers` is the one that did not exist and
    /// whose absence was the defect.
    #[test]
    fn a_second_peer_defers_to_the_domains_existing_home() {
        assert_eq!(home_claim(None, "2KAlpha", false), HomeClaim::Defines);
        assert_eq!(home_claim(Some(""), "2KAlpha", false), HomeClaim::Defines);
        assert_eq!(home_claim(Some("2KAlpha"), "2KAlpha", false), HomeClaim::Republishes);
        assert_eq!(
            home_claim(Some("2KAlpha"), "2KBeta", false),
            HomeClaim::Defers { to: "2KAlpha".into() },
            "a second peer published to a domain must NOT take its home — that flip \
             re-homes every returning visitor AND writes a supersession record against \
             a peer that is still alive"
        );
        assert_eq!(
            home_claim(Some("2KAlpha"), "2KBeta", true),
            HomeClaim::Takes { from: "2KAlpha".into() },
            "moving a domain's home is available, it just has to be asked for"
        );
    }

    /// **The measured defect, as a test.** Publish two peers, each under its own
    /// `--prefix`, into one out-dir — the multi-peer-at-one-domain shape the
    /// tooling has supported since prefixes existed
    /// (`DESIGN-DEPLOYMENT-GENERATIONS` §7: the document *"names peers, their
    /// prefixes"*). Before the merge, the second publish clobbered the document
    /// and the first peer's origin vanished.
    #[test]
    fn two_peers_on_one_domain_both_survive_in_the_document() {
        let tmp = std::env::temp_dir()
            .join(format!("entity-publish-multipeer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.to_string_lossy().to_string();

        let publish = |seed_hex: &str, prefix: &str| {
            run(&[
                "publish".to_string(),
                out.clone(),
                "--deployment-config".to_string(),
                format!("--identity-seed={seed_hex}"),
                format!("--prefix={prefix}"),
            ])
        };
        let a = "c1".repeat(32);
        let b = "c2".repeat(32);
        assert_eq!(publish(&a, "alpha"), ExitCode::SUCCESS);
        let doc: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(tmp.join("entity-deployment.json")).unwrap(),
        )
        .unwrap();
        let home_a = doc["home_site"]["peer"].as_str().unwrap().to_string();
        assert_eq!(doc["origins"][&home_a].as_str(), Some("/alpha"));

        assert_eq!(publish(&b, "beta"), ExitCode::SUCCESS);
        let doc: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(tmp.join("entity-deployment.json")).unwrap(),
        )
        .unwrap();
        let origins = doc["origins"].as_object().expect("origins is an object");

        assert_eq!(
            origins.len(),
            2,
            "both peers hosted on this domain must be declared — the document is \
             domain-managed, not a record of the last publish. Got: {origins:?}"
        );
        assert_eq!(
            origins.get(&home_a).and_then(|v| v.as_str()),
            Some("/alpha"),
            "the FIRST peer's origin was dropped by the second publish"
        );
        assert_eq!(
            doc["home_site"]["peer"].as_str(),
            Some(home_a.as_str()),
            "the second publish silently re-homed the domain onto itself"
        );

        // …and `--set-home` still moves it, deliberately.
        assert_eq!(
            run(&[
                "publish".to_string(),
                out.clone(),
                "--deployment-config".to_string(),
                "--set-home".to_string(),
                format!("--identity-seed={b}"),
                "--prefix=beta".to_string(),
            ]),
            ExitCode::SUCCESS
        );
        let doc: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(tmp.join("entity-deployment.json")).unwrap(),
        )
        .unwrap();
        assert_ne!(doc["home_site"]["peer"].as_str(), Some(home_a.as_str()));
        assert_eq!(
            doc["origins"].as_object().unwrap().len(),
            2,
            "moving the home must not drop the peer that used to hold it"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// **Publishing a second peer at a shared hosting scope must not destroy the
    /// first one — asserted with `--verify`, this repo's own walker.**
    ///
    /// The topology: several publishers on one origin, no prefix, told apart by
    /// their tree paths (`{peer}/…`). Before the fix, peer B's publish deleted
    /// the shared `content/` store and the whole `sites/` parent, so peer A lost
    /// its signature blob and its projection — `--verify` on A reported *"1
    /// BROKEN entry — this tree is not safe to serve"* and *"a pinned consumer
    /// resolves NOTHING from this tree."*
    ///
    /// **`--verify` is the assertion deliberately**, rather than counting files:
    /// it walks the signed root and every pointer the way a consumer does, so it
    /// answers *is A still serveable*, which is the actual property. A file count
    /// would have passed the whole time the tree was broken — both peers publish
    /// the same demo set, so the blob count is identical before and after.
    #[test]
    fn a_second_publisher_at_one_origin_does_not_break_the_first() {
        let tmp = std::env::temp_dir()
            .join(format!("entity-publish-sharedscope-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.to_string_lossy().to_string();

        let seed_a = "d1".repeat(32);
        let seed_b = "d2".repeat(32);
        let publish = |seed: &str| {
            run(&[
                "publish".to_string(),
                out.clone(),
                "--deployment-config".to_string(),
                format!("--identity-seed={seed}"),
            ])
        };
        let verify = |seed: &str| {
            run(&[
                "publish".to_string(),
                out.clone(),
                "--verify".to_string(),
                format!("--identity-seed={seed}"),
            ])
        };

        assert_eq!(publish(&seed_a), ExitCode::SUCCESS);
        assert_eq!(
            verify(&seed_a),
            ExitCode::SUCCESS,
            "PRECONDITION: the first publish must verify, or this gate measures nothing"
        );

        assert_eq!(publish(&seed_b), ExitCode::SUCCESS);
        assert_eq!(
            verify(&seed_a),
            ExitCode::SUCCESS,
            "publishing a SECOND peer at the same hosting scope broke the FIRST one's \
             tree — its signature blob or its projection was deleted by a clean scoped \
             to the prefix instead of to the peer. A pinned consumer of peer A now \
             resolves nothing."
        );
        assert_eq!(verify(&seed_b), ExitCode::SUCCESS, "the second peer must verify too");

        // And the first peer is still *addressable*: its projection survives, so
        // a deep link into it is not a 404.
        let a_peer = std::fs::read_dir(tmp.join("sites"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .count();
        assert_eq!(a_peer, 2, "both publishers must keep a site projection under sites/");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A malformed document is a **hard stop**, never an implicit fresh start —
    /// the same rule `BuildsManifest::from_json` follows, and sharper here:
    /// silently starting fresh on an unreadable document performs exactly the
    /// clobber this merge exists to prevent.
    #[test]
    fn an_unreadable_domain_document_is_not_overwritten() {
        let tmp = std::env::temp_dir()
            .join(format!("entity-publish-baddoc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("entity-deployment.json"), b"{not json at all").unwrap();

        assert_ne!(
            run(&[
                "publish".to_string(),
                tmp.to_string_lossy().to_string(),
                "--deployment-config".to_string(),
                "--demo-identity".to_string(),
            ]),
            ExitCode::SUCCESS,
            "an unreadable domain document must stop the publish, not be replaced"
        );
        assert_eq!(
            std::fs::read_to_string(tmp.join("entity-deployment.json")).unwrap(),
            "{not json at all",
            "the operator's file must still be there to fix"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn verify_proves_a_published_tree_and_catches_a_tampered_body() {
        let tmp =
            std::env::temp_dir().join(format!("entity-publish-verify-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.to_string_lossy().to_string();
        let call = |extra: &[&str]| {
            let mut args = vec!["publish".to_string(), out.clone(), "--demo-identity".into()];
            args.extend(extra.iter().map(|s| s.to_string()));
            run(&args)
        };

        // A real publish (both forms — the .bin tree is what verify walks).
        assert_eq!(call(&[]), ExitCode::SUCCESS);
        // The freshly published tree must verify clean.
        assert_eq!(call(&["--verify"]), ExitCode::SUCCESS);

        // Flip a byte in one content blob. The path still exists and still
        // *claims* that hash — only the bytes lie, which is exactly the case a
        // path-existence check cannot see and a hash check must.
        let mut blobs = Vec::new();
        collect_files(&tmp.join("content"), "", &mut blobs);
        blobs.sort();
        // **Pick the victim by EFFECT, not by position.** This was
        // `blobs.first()`, and it made the test load-sensitive in a way that took
        // a suite failure to see: the manifest's address changes between runs, so
        // which blob sorts first changes too. When the manifest happened to sort
        // first the flip landed on a blob no pointer named — unchecked at the
        // time — and `--verify` returned 0. It passed alone and failed in the
        // suite, which reads as flake and was a real hole (now closed by the
        // all-blobs check, and pinned by
        // `a_blob_no_pointer_names_must_still_hash_to_its_address`).
        //
        // This test is about a tampered PAGE BODY, so it takes a blob a `.bin`
        // actually names. Under hash-keyed addressing, "the first one" is not a
        // stable referent for anything.
        let mut pointer_files = Vec::new();
        collect_files(&tmp, "bin", &mut pointer_files);
        let pointer_bytes: Vec<Vec<u8>> =
            pointer_files.iter().filter_map(|p| std::fs::read(p).ok()).collect();
        let victim = blobs
            .iter()
            .find(|b| {
                let hex = b.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                let raw: Vec<u8> = (0..hex.len())
                    .step_by(2)
                    .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
                    .collect();
                !raw.is_empty()
                    && pointer_bytes.iter().any(|pb| pb.windows(raw.len()).any(|w| w == raw))
            })
            .expect("a publish emits content blobs that pointers name")
            .clone();
        let original = std::fs::read(&victim).unwrap();
        let mut bytes = original.clone();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(&victim, &bytes).unwrap();

        assert_eq!(
            call(&["--verify"]),
            ExitCode::from(VERIFY_DEFECT_EXIT),
            "a body that no longer hashes to its address must fail verification"
        );

        // APPENDING is the case that slipped through and is why the canonical
        // check exists. `ciborium::from_reader` stops at the first complete CBOR
        // item and ignores what follows, and the hash is over the canonical
        // re-encode — so a blob with junk appended DECODES and VALIDATES.
        // Measured: this passed --verify until the byte-exactness check landed.
        std::fs::write(&victim, &original).unwrap();
        assert_eq!(call(&["--verify"]), ExitCode::SUCCESS, "restored tree is clean again");
        let mut appended = original.clone();
        appended.push(0xff);
        std::fs::write(&victim, &appended).unwrap();
        assert_eq!(
            call(&["--verify"]),
            ExitCode::from(VERIFY_DEFECT_EXIT),
            "a blob with trailing bytes still hashes correctly — the canonical check is what \
             catches it, and without it this tree reads as clean"
        );

        // Deleting a blob outright is the other broken shape.
        std::fs::remove_file(&victim).unwrap();
        assert_eq!(call(&["--verify"]), ExitCode::from(VERIFY_DEFECT_EXIT));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// **A blob no pointer names must still hash to its address.**
    ///
    /// The pointer sweep checks integrity exactly where a `.bin` points, which
    /// left two kinds unchecked: HAMT interior nodes (reachable only by hash from
    /// the signed root) and the manifest's own content-store copy. Measured
    /// before the fix — flipping one byte in the trie root took the closure from
    /// **16 blobs to 1**, still reported `0 missing`, printed *"every pointer
    /// resolves and every body hashes to its address"* and exited **0**, while a
    /// pinned consumer resolves nothing at all from that tree.
    ///
    /// **Every unreferenced blob is flipped in turn, and the victim list is
    /// taken from this publish's own output** — not `blobs.first()`. Under
    /// hash-keyed addressing "the first blob" is not a stable referent: the
    /// manifest's address moves between runs, so which blob sorts first changes,
    /// which is how the sibling test passed alone and failed in the suite. A
    /// victim picked by position proves one path and reports on all of them.
    #[test]
    fn a_blob_no_pointer_names_must_still_hash_to_its_address() {
        let tmp =
            std::env::temp_dir().join(format!("entity-publish-unref-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.to_string_lossy().to_string();
        let call = |extra: &[&str]| {
            let mut args = vec!["publish".to_string(), out.clone(), "--demo-identity".into()];
            args.extend(extra.iter().map(|s| s.to_string()));
            run(&args)
        };
        assert_eq!(call(&[]), ExitCode::SUCCESS);
        assert_eq!(call(&["--verify"]), ExitCode::SUCCESS);

        // Which blobs does no `.bin` name? Derived from THIS publish, so the set
        // tracks the emitter instead of being pinned to a shape that will drift.
        let mut blobs = Vec::new();
        collect_files(&tmp.join("content"), "", &mut blobs);
        let mut pointers = Vec::new();
        collect_files(&tmp, "bin", &mut pointers);
        let pointer_bytes: Vec<Vec<u8>> =
            pointers.iter().filter_map(|p| std::fs::read(p).ok()).collect();

        let unreferenced: Vec<_> = blobs
            .iter()
            .filter(|b| {
                let hex = b.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                let raw: Vec<u8> = (0..hex.len())
                    .step_by(2)
                    .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
                    .collect();
                !pointer_bytes.iter().any(|pb| {
                    pb.windows(raw.len().max(1)).any(|w| w == raw.as_slice())
                })
            })
            .cloned()
            .collect();
        assert!(
            !unreferenced.is_empty(),
            "a signed publish emits blobs no pointer names (trie nodes, the manifest copy) — \
             if this is empty the fixture stopped covering the case"
        );

        for victim in &unreferenced {
            let original = std::fs::read(victim).unwrap();
            let mut bytes = original.clone();
            let last = bytes.len() - 1;
            bytes[last] ^= 0xff;
            std::fs::write(victim, &bytes).unwrap();

            let code = call(&["--verify"]);
            std::fs::write(victim, &original).unwrap();

            assert_eq!(
                code,
                ExitCode::from(VERIFY_DEFECT_EXIT),
                "a flipped byte in {} — which no pointer names — must fail verification; \
                 before this check the closure silently shortened and verify exited 0",
                victim.display()
            );
        }

        // Control: with every byte restored the tree verifies clean again, so the
        // failures above were the tampering and not the loop.
        assert_eq!(call(&["--verify"]), ExitCode::SUCCESS);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// **An empty tree must not verify clean** — the publisher's own last gate
    /// before shipping said yes about nothing.
    ///
    /// Zero broken out of zero checked is zero broken, so `--verify` on a
    /// directory holding *nothing* printed *"every pointer resolves and every
    /// body hashes to its address"* and exited 0. Every way of aiming this verb
    /// at the wrong place produces exactly this state — the wrong directory, an
    /// upload that never finished, the wrong *identity* (`publish --verify` on a
    /// registry tree, the trap the publishing guide §3.3 already documents), or a
    /// containerized run whose OUT fell outside the bind mount. All of them
    /// passed.
    ///
    /// Exit **1**, not `VERIFY_DEFECT_EXIT`: nothing here is defective, the
    /// verifier did not run against a tree. Keeping 2 for "I checked and it is
    /// broken" is what lets an operator's `&&` chain tell the two apart.
    ///
    /// Mutation check, both directions: drop the `pointers.is_empty()` guard and
    /// the empty case returns SUCCESS (red here); make the guard unconditional
    /// and the real publish fails (red on the second assertion). The second half
    /// is the one that matters — a check that fires on everything is not a check.
    #[test]
    fn an_empty_tree_is_not_a_clean_tree() {
        let tmp =
            std::env::temp_dir().join(format!("entity-publish-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.to_string_lossy().to_string();
        let call = |extra: &[&str]| {
            let mut args = vec!["publish".to_string(), out.clone(), "--demo-identity".into()];
            args.extend(extra.iter().map(|s| s.to_string()));
            run(&args)
        };

        // Nothing published yet: the directory exists and is empty. This is the
        // shape a mistyped path, a failed upload and a wrong-identity verify all
        // land in, and it used to be indistinguishable from success.
        assert_eq!(
            call(&["--verify"]),
            ExitCode::from(1),
            "an empty directory must report that nothing was verified, not that all is well"
        );

        // The control, and the half that keeps the guard honest: a REAL publish
        // into the same directory must still verify clean. A guard that also
        // fires here would be trading a false pass for a false failure.
        assert_eq!(call(&[]), ExitCode::SUCCESS);
        assert_eq!(
            call(&["--verify"]),
            ExitCode::SUCCESS,
            "a real published tree must still verify clean"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// An orphaned blob is reported but does **not** fail the tree: nothing
    /// links to it, so no reader can hit it, and a tree mid-cutover legitimately
    /// holds blobs its pointers have not adopted yet. Failing on this would make
    /// the verifier unusable exactly where it is most useful.
    #[test]
    fn verify_reports_an_orphan_blob_without_failing_the_tree() {
        let tmp =
            std::env::temp_dir().join(format!("entity-publish-orphan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.to_string_lossy().to_string();
        let call = |extra: &[&str]| {
            let mut args = vec!["publish".to_string(), out.clone(), "--demo-identity".into()];
            args.extend(extra.iter().map(|s| s.to_string()));
            run(&args)
        };
        assert_eq!(call(&[]), ExitCode::SUCCESS);

        // A blob nothing points at — the shape a missing prune leaves behind.
        let orphan = tmp.join("content").join("ab").join("cd");
        std::fs::create_dir_all(&orphan).unwrap();
        std::fs::write(orphan.join("abcdnot-a-real-hash"), b"stale").unwrap();

        assert_eq!(
            call(&["--verify"]),
            ExitCode::SUCCESS,
            "an orphan is hygiene, not a broken tree"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// **We are discoverable now — the publisher half of §6.5.3 v1.8.**
    ///
    /// A publish emits its own `http-poll` endpoint beside the site, and a
    /// consumer holding **nothing but the peer-id and the directory** enters
    /// through it: reads the profile, takes `manifest_url_prefix`, and walks to
    /// an authored page. No convention anywhere on that path.
    ///
    /// This is the direction the cross-implementation run could not test. It
    /// found *us* unable to consume *them*; the mirror image — them unable to
    /// consume us, because we advertised the endpoint only inside a registry
    /// binding and a bare origin carried no endpoint document at all — was
    /// invisible from either side and is what this closes
    /// (`ROUTING-2026-08-19-c` §4).
    ///
    /// Mutation check: drop the `write_transport_profile` call and this fails on
    /// the profile's absence, not on the walk — the two are asserted separately
    /// so a missing artifact cannot read as a broken tree.
    #[test]
    fn a_publish_advertises_its_own_endpoint_and_a_consumer_enters_through_it() {
        use crate::content_site::publish_layout::PublishLayout;
        use crate::content_site::signed_root::{DirFetcher, TRANSPORT_PROFILE_REL};
        use entity_peer::published_root::PublishedRootClient;

        let tmp =
            std::env::temp_dir().join(format!("entity-publish-profile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.to_string_lossy().to_string();
        assert_eq!(
            run(&["publish".to_string(), out.clone(), "--demo-identity".into()]),
            ExitCode::SUCCESS
        );

        let profile_bytes = std::fs::read(tmp.join(TRANSPORT_PROFILE_REL))
            .expect("a publish ships its endpoint beside the site");
        let layout = PublishLayout::from_profile_artifact(&profile_bytes)
            .expect("it decodes as an http-poll endpoint");
        assert!(
            layout.manifest_url.ends_with("/system/peer/published-root"),
            "we advertise where our root actually is: {}",
            layout.manifest_url
        );

        // The consumer's half: nothing here knows our layout — `DirFetcher`
        // reads the profile off the directory and enters at what it names.
        let kp = entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED);
        let peer_id = kp.peer_id().as_str().to_string();
        let pid = entity_crypto::PeerId::from(peer_id.clone());
        let (pubkey, kt) = pid.derive_public_key().expect("demo identity is canonical form");
        let client = PublishedRootClient::new(
            DirFetcher::new(&tmp, &peer_id),
            pubkey,
            entity_crypto::KeyType::from_byte(kt).unwrap(),
            Some(peer_id.clone()),
        );
        let root = client.fetch_root().expect("the root verifies, entered via the profile");
        assert_eq!(root.peer_id, peer_id, "the root we reached is this publisher's");
        assert_eq!(
            root.prefix,
            format!("/{peer_id}/"),
            "and it declares the extent we published"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The plan and the publish-time warning must never disagree about what
    /// disappears — they share one predicate, and this pins that they do.
    #[test]
    fn the_plan_and_the_warning_share_one_definition_of_what_disappears() {
        use crate::content_site::{SiteManifest, SitePage};
        let site = |id: &str| read::OwnedSite {
            peer_id: "P".into(),
            site_id: id.into(),
            manifest: SiteManifest::new(id, id, "index", Vec::new()),
            pages: vec![("index".to_string(), SitePage::markdown("I", "hi"))],
            assets: Vec::new(),
        };
        let existing = vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()];
        let incoming = [site("beta"), site("delta")];

        let dropped = dropped_site_ids(&existing, &incoming);
        assert_eq!(dropped, vec!["alpha", "gamma"]);
        // The warning's count is that same set, not a second computation.
        assert_eq!(warn_replaced_sites(&existing, &incoming), dropped.len());
    }

    /// [`projected_site_ids`] reads the ids off the real projection layout
    /// (`{out}/{prefix}/sites/{peer}/{site}/`) and treats an absent tree as
    /// empty — it runs on every publish, including the first one into a fresh
    /// directory, where the path does not exist yet.
    #[test]
    fn projected_site_ids_reads_the_layout_and_tolerates_an_absent_tree() {
        let tmp = std::env::temp_dir().join(format!(
            "entity-publish-projected-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);

        // Absent tree → empty, not an error.
        assert!(projected_site_ids(&tmp, "P", "").is_empty());
        assert!(projected_site_ids(&tmp, "P", "scope").is_empty());

        let root = tmp.join("scope").join(SITE_URL_PREFIX).join("P");
        std::fs::create_dir_all(root.join("beta")).unwrap();
        std::fs::create_dir_all(root.join("alpha")).unwrap();
        // A stray FILE beside the site dirs is not a site.
        std::fs::write(root.join("index.html"), b"x").unwrap();

        assert_eq!(projected_site_ids(&tmp, "P", "scope"), vec!["alpha", "beta"]);
        // Scoped to the prefix AND the peer: neither the bare root nor another
        // peer sees these.
        assert!(projected_site_ids(&tmp, "P", "").is_empty());
        assert!(projected_site_ids(&tmp, "Q", "scope").is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Regression: the bundled demo SET the publisher ships must itself carry NO
    /// dangling nav links — the demo is the canonical clean baseline the ratchet
    /// guards (and it now spans demo + demo-notes + entity-info).
    #[test]
    fn bundled_demo_set_has_no_dangling_nav_links() {
        let demo_kp = entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED);
        let (_pid, sites, _apps) = resolve_publish_source(demo_kp, None, None).unwrap();
        assert_eq!(
            warn_dangling_nav_links(&sites),
            0,
            "the bundled demo set must have no dangling nav links"
        );
    }

    /// Explicit-seed parsing: absent → `None` (use the durable publisher key);
    /// `--demo-identity` → the demo seed; a 64-hex `--identity-seed` parses to
    /// those exact bytes and wins over `--demo-identity`; a malformed seed is a
    /// loud error (not a silent fall-back to any identity).
    #[test]
    fn explicit_publish_seed_parses_identity_or_defaults() {
        // Absent → None ⇒ the caller uses the durable publisher keypair.
        assert_eq!(explicit_publish_seed(&[]).unwrap(), None);

        // `--demo-identity` → the fixed demo seed (dev/testing opt-in).
        assert_eq!(
            explicit_publish_seed(&["--demo-identity".to_string()]).unwrap(),
            Some(DEMO_PUBLISH_SEED)
        );

        // A round-tripped hex seed parses back to the exact bytes.
        let custom = *b"per-site-publisher-identity-0001";
        let hex = crate::vault_codec::seed_to_hex(&custom);
        let arg = vec![format!("--identity-seed={hex}")];
        assert_eq!(explicit_publish_seed(&arg).unwrap(), Some(custom));

        // `--identity-seed` (explicit hex) wins over `--demo-identity`.
        let both = vec![format!("--identity-seed={hex}"), "--demo-identity".to_string()];
        assert_eq!(explicit_publish_seed(&both).unwrap(), Some(custom));

        // Malformed (wrong length / non-hex) → Err, so a typo fails the build.
        assert!(explicit_publish_seed(&["--identity-seed=deadbeef".to_string()]).is_err());
        assert!(explicit_publish_seed(&["--identity-seed=not-hex".to_string()]).is_err());
    }

    /// E2E fixture generator (run by `tests/e2e_worker.rs` Phase 27 via
    /// `cargo test --bin entity-browser emit_deployment_config_fixture -- --ignored`,
    /// NOT the unit suite — hence `#[ignore]`). Publishes the demo set plus a
    /// locked-site, same-origin deployment config into `dist/`, so the served
    /// SPA (a generic chrome build) can `GET /entity-deployment.json`, apply it,
    /// and boot into the published home over same-origin HTTP-poll. Mirrors the
    /// `emit_e2e_fixture` pattern. Asserts the config landed so a silent publish
    /// failure surfaces here, not as a confusing phase-27 boot.
    #[test]
    #[ignore = "e2e fixture generator; run by the e2e harness via --ignored"]
    fn emit_deployment_config_fixture() {
        // Same as `make site dist DEPLOY_CONFIG=1 SURFACE=site LOCKED=1`:
        // emits the `.bin` content (sites/{peer}/… + content/…) AND
        // dist/entity-deployment.json pointing a generic SPA at this origin's
        // published demo (origin "" = same-origin).
        //
        // `--surface=site --locked` is EXPLICIT (not the publish default): the
        // e2e consumers (Phase 27, `default_idb_boots_into_remote_deployment_home`)
        // assert the config boots into the SITE OVERLAY (`mode-site`, home in
        // `#site-layer`). Only `surface=site` maps to `BootSurface::Site` — the
        // default `--surface=window` boots a maximized Site Browser *window*
        // (`mode-dom`), so relying on the default here would (silently) emit the
        // wrong surface for those overlay assertions.
        // **`--set-home` on every fixture emitter, and it is semantics.** Each of
        // these DEFINES the domain its scenario boots against, and the document
        // merges rather than clobbers now — so without the flag a fixture run
        // into a tree that already carries a document (a staged copy of `dist/`
        // hardlinks one in as soon as any earlier fixture has published there)
        // would DEFER, keep the other scenario's home, and quietly emit a domain
        // that is not the one under test. Measured: 24 unfiltered failures, all
        // publish-fixture-driven, every one of them green when run alone.
        let _ = run(&[
            "publish".to_string(),
            "dist".to_string(),
            "--deployment-config".to_string(),
            "--set-home".to_string(),
            "--surface=site".to_string(),
            "--locked".to_string(),
        ]);
        assert!(
            std::path::Path::new("dist/entity-deployment.json").exists(),
            "deployment config not emitted into dist/ — publish --deployment-config failed"
        );
    }

    // ── The re-key reproduction fixtures ──────────────────────────────────
    //
    // Two identities, so `dist/` can be published as one publisher and then
    // RE-published as a different one — the `ecdeos.org` 2026-08-24 re-key,
    // reproduced offline. Consumed by `tests/e2e_worker.rs`
    // `rekeyed_domain_heals_on_next_boot`.
    //
    // They are FIXED seeds rather than generated ones because the e2e reads the
    // resulting peer-ids back out of the emitted `entity-deployment.json`: the
    // test must never re-derive a key itself, or a change to key derivation
    // would make the fixture and the app agree with each other while both drift
    // from the published artifact.

    /// The publisher a returning visitor's browser learned at first contact.
    const REKEY_SEED_BEFORE: [u8; 32] = *b"entity-rekey-before-seed-v1\0\0\0\0\0";
    /// The durable identity the domain is re-keyed onto. Distinct from
    /// [`DEMO_PUBLISH_SEED`] AND from `REKEY_SEED_BEFORE`, so a fixture that
    /// silently failed to switch identity cannot pass as a re-key.
    const REKEY_SEED_AFTER: [u8; 32] = *b"entity-rekey-after-seed-v1\0\0\0\0\0\0";

    /// Publish the demo set + a locked same-origin deployment config into
    /// `dist/` under an EXPLICIT publisher identity.
    ///
    /// `publish` only cleans its *own* peer's subtree (`base.join(peer_id)`), so
    /// calling this twice with different seeds leaves BOTH trees standing —
    /// which is the honest starting point. Production's re-key then had a second
    /// half: *"the abandoned demo peer 404s"* was a listed success criterion. The
    /// e2e performs that deletion itself rather than having the fixture do it,
    /// so the two halves of a re-key stay separately observable.
    /// `surface_args` is what makes this cover BOTH deployment shapes. The
    /// re-key repair must not depend on which surface the domain ships — the
    /// first version of the fix healed `surface=site` and left `surface=window`
    /// broken, which is how `entitychurchfoundation.org` deploys, and a fix that
    /// works on one surface is not a fix (AP26).
    /// The output directory is taken from `ENTITY_REKEY_OUT` (default `dist`)
    /// so the e2e can publish into an **isolated copy** of the served tree.
    ///
    /// That is not tidiness. Publishing these fixtures into the shared `dist/`
    /// broke a phase that has nothing to do with re-keying: the monolith's
    /// Phase 27 fixture then failed with *"publisher bound no signature"*,
    /// because a publish whose content is already present takes the engine's
    /// idempotent path and expects a prior signed head that a different
    /// publisher's artifacts cannot supply. It passed when the monolith ran
    /// alone and failed only when these tests ran first — i.e. it presented as
    /// a flaky suite, in the wrong file.
    fn emit_rekey_fixture(seed: [u8; 32], surface_args: &[&str]) {
        let out = std::env::var("ENTITY_REKEY_OUT").unwrap_or_else(|_| "dist".to_string());
        let hex = crate::vault_codec::seed_to_hex(&seed);
        // `--set-home` is REQUIRED here and is not a workaround: the `_after`
        // publish puts a *different* identity into a tree whose document already
        // names the `_before` peer as home, and a re-key is precisely the
        // deliberate home move that flag names. Without it the publish would
        // (correctly) defer, the document would keep naming the retired peer,
        // and the fixture would emit a domain that never re-keyed — a fixture
        // silently not reproducing its own incident. Harmless on `_before`,
        // which publishes into a tree with no document at all (`make wasm`'s
        // `dist/` carries none) and so DEFINES rather than takes.
        let mut args = vec![
            "publish".to_string(),
            out.clone(),
            "--deployment-config".to_string(),
            "--set-home".to_string(),
            format!("--identity-seed={hex}"),
        ];
        args.extend(surface_args.iter().map(|s| s.to_string()));
        let _ = run(&args);
        assert!(
            std::path::Path::new(&out).join("entity-deployment.json").exists(),
            "re-key fixture: deployment config not emitted into {out}/"
        );
    }

    /// The locked-kiosk overlay surface.
    ///
    /// **This is no longer "`ecdeos`-shaped", and the old label was stale** — measured
    /// 2026-08-28, `ecdeos.org` serves `surface: chrome` with `site_mode.enabled: false`,
    /// and so does every other production domain: overlays are **off everywhere**. Site
    /// mode was turned off deliberately after the kiosk accumulated defects that were hard
    /// to attribute (most of them the peer/persistence bugs this thread has been chasing,
    /// not the overlay), and window mode replaced it because the Site Browser shows the
    /// other sites and can be closed.
    ///
    /// It is kept because it is the **worst case** for anything that fails to resolve — the
    /// overlay is the whole screen and the toggle is suppressed — so a repair proven here is
    /// proven on the surface with the least room to report. Do not read a passing gate on it
    /// as a statement about a shipped deployment.
    const REKEY_SURFACE_SITE: &[&str] = &["--surface=site", "--locked"];
    /// The maximized Site Browser window — `entitychurchfoundation.org`- and
    /// `entitychurchregistry.org`-shaped, and the surface production actually ships.
    const REKEY_SURFACE_WINDOW: &[&str] = &["--surface=window", "--window-type=Site Browser"];

    #[test]
    #[ignore = "e2e fixture generator; run by the e2e harness via --ignored"]
    fn emit_rekey_fixture_before() {
        emit_rekey_fixture(REKEY_SEED_BEFORE, REKEY_SURFACE_SITE);
    }

    #[test]
    #[ignore = "e2e fixture generator; run by the e2e harness via --ignored"]
    fn emit_rekey_fixture_after() {
        emit_rekey_fixture(REKEY_SEED_AFTER, REKEY_SURFACE_SITE);
    }

    #[test]
    #[ignore = "e2e fixture generator; run by the e2e harness via --ignored"]
    fn emit_rekey_fixture_before_window() {
        emit_rekey_fixture(REKEY_SEED_BEFORE, REKEY_SURFACE_WINDOW);
    }

    #[test]
    #[ignore = "e2e fixture generator; run by the e2e harness via --ignored"]
    fn emit_rekey_fixture_after_window() {
        emit_rekey_fixture(REKEY_SEED_AFTER, REKEY_SURFACE_WINDOW);
    }

    /// The **Apps-surface** re-key pair — AP54's browser half.
    ///
    /// The site re-key fixtures above could never have caught the 2026-09-05
    /// incident: they publish sites, and the surface that failed to heal was
    /// Apps. This pair publishes a **different app under each identity**, so the
    /// assertion downstream is *which publisher are we sourcing from* rather
    /// than *did anything error* — which matters because the `_before` peer's
    /// tree SURVIVES the second publish (a publish cleans only its own peer's
    /// roots), so under the defect the old catalog still answers 200 and a
    /// gate asserting "no error" would pass with the bug fully present.
    ///
    /// `--surface=chrome` so the launcher's `+ Apps` button exists to click.
    fn emit_rekey_apps_fixture(seed: [u8; 32], app_id: &str, app_name: &str) {
        let out = std::env::var("ENTITY_REKEY_OUT").unwrap_or_else(|_| "dist".to_string());
        let hex = crate::vault_codec::seed_to_hex(&seed);
        let apps_dist = std::env::temp_dir().join(format!("entity-rekey-apps-{app_id}"));
        let _ = std::fs::remove_dir_all(&apps_dist);
        std::fs::create_dir_all(&apps_dist).unwrap();
        std::fs::write(
            apps_dist.join("index.json"),
            format!(
                r#"[{{"id":"{app_id}","name":"{app_name}","description":"rekey fixture","saves":false,"type":"tool"}}]"#
            ),
        )
        .unwrap();
        std::fs::write(
            apps_dist.join(format!("{app_id}.html")),
            format!("<html><body><h1>{app_name}</h1></body></html>"),
        )
        .unwrap();
        let args = vec![
            "publish".to_string(),
            out.clone(),
            "--deployment-config".to_string(),
            "--set-home".to_string(),
            "--surface=chrome".to_string(),
            format!("--identity-seed={hex}"),
            format!("--ingest-apps={}", apps_dist.display()),
        ];
        let _ = run(&args);
        assert!(
            std::path::Path::new(&out).join("entity-deployment.json").exists(),
            "re-key apps fixture: deployment config not emitted into {out}/"
        );
    }

    #[test]
    #[ignore = "e2e fixture generator; run by the e2e harness via --ignored"]
    fn emit_rekey_apps_before() {
        emit_rekey_apps_fixture(REKEY_SEED_BEFORE, "alphatool", "AlphaFromA");
    }

    #[test]
    #[ignore = "e2e fixture generator; run by the e2e harness via --ignored"]
    fn emit_rekey_apps_after() {
        emit_rekey_apps_fixture(REKEY_SEED_AFTER, "betatool", "BetaFromB");
    }

    // ── The demo-site PULL fixture (B-7) ──────────────────────────────────
    //
    // **Not a re-key, and the difference is the finding.** The publisher
    // identity does not move, `/entity-deployment.json` is not touched, and the
    // origin keeps serving. What changes is that the publisher's next publish
    // ships a site set that no longer carries `demo` — which is the site every
    // profile that was never told otherwise is pointed at, because
    // `home_site_from` defaults the id to `DEMO_SITE_ID` ("the site id is never
    // empty — the overlay always needs a site to point at") and the bundled
    // offline fallback is gated behind a **local** home. A deployment that
    // declares a REMOTE home named `demo` takes the thin-lens path and seeds
    // nothing, so when the site leaves the publisher's tree there is no copy to
    // fall back to.
    //
    // So pulling the demo id is a routing change, not a content edit, and it
    // lands on exactly the profiles that never chose a home. Two live domains
    // declare a remote home named `demo` today, which is why this is a gate and
    // not a note.
    //
    // Devops measured the production shape: every tree publishes `demo` under
    // its own peer, so there is **no overlap window** — the old one vanishes at
    // the instant of the flip. A cliff, not a window.
    //
    // Emitted through the REAL publish path, never by deleting files out of the
    // served tree, and that is load-bearing: `run_projection` cleans
    // `{base}/{peer}/` wholesale and re-projects a fresh signed root over the
    // new set, carrying the prior `seq`. A hand-deletion would leave a root that
    // still claims `demo` exists, and the browser would then be measured against
    // a broken-tree failure rather than an honestly-published one — a different
    // bug wearing the same symptom.
    //
    // Deliberately NO `--deployment-config`: a document still naming a site that
    // is gone is the whole point. Re-emitting it would silently re-home the
    // deployment onto whatever survived and gate nothing.

    /// The site the publisher ships INSTEAD of `demo`. Any id but `demo`; named
    /// for what it is so a failure message reads.
    const DEMO_PULL_SITE_ID: &str = "after-the-pull";

    #[test]
    #[ignore = "e2e fixture generator; run by the e2e harness via --ignored"]
    fn emit_demo_pull_fixture() {
        let out = std::env::var("ENTITY_REKEY_OUT").unwrap_or_else(|_| "dist".to_string());
        let peer = entity_crypto::Keypair::from_seed(REKEY_SEED_BEFORE).peer_id().to_string();
        let out_root = std::path::Path::new(&out);
        let demo_dir = out_root.join(&peer).join("sites").join("demo");

        // STAGING. This fixture only means anything as the SECOND publish under
        // an identity that already shipped `demo`. Run first, it pulls nothing,
        // and every assertion downstream would be about a site that was never
        // there — the vacuous-pass shape that `--exact` and the `1 passed` check
        // exist to prevent one layer up.
        assert!(
            demo_dir.is_dir(),
            "demo-pull fixture: {} must already be published — run \
             emit_rekey_fixture_before first, or this pulls nothing",
            demo_dir.display()
        );

        // A minimal `render/` emit — one site, and it is not `demo`. This is the
        // same disk→tree route a content team's publish takes (`--ingest`), so
        // the resulting tree is the shape production actually serves.
        let render = std::env::temp_dir().join("entity-demo-pull-render");
        let _ = std::fs::remove_dir_all(&render);
        let site = render.join(DEMO_PULL_SITE_ID);
        std::fs::create_dir_all(site.join("pages")).expect("stage the render dir");
        std::fs::write(
            site.join("site.manifest.json"),
            format!(r#"{{"site_id":"{DEMO_PULL_SITE_ID}","title":"After The Pull"}}"#),
        )
        .expect("write site.manifest.json");
        std::fs::write(
            site.join("pages").join("index.md"),
            "# After The Pull\n\nThis publisher no longer carries the demo site.\n",
        )
        .expect("write the landing page");

        let hex = crate::vault_codec::seed_to_hex(&REKEY_SEED_BEFORE);
        let _ = run(&[
            "publish".to_string(),
            out.clone(),
            format!("--identity-seed={hex}"),
            format!("--ingest={}", render.display()),
        ]);

        // Asserted on the artifact, not on the exit code: what the browser meets
        // is the served tree, and a publish that reported success while leaving
        // `demo` standing would stage a scenario that proves nothing.
        assert!(
            !demo_dir.exists(),
            "demo-pull fixture: {} survived the republish — the site was not pulled",
            demo_dir.display()
        );
        assert!(
            out_root
                .join(&peer)
                .join("sites")
                .join(DEMO_PULL_SITE_ID)
                .join("manifest.bin")
                .is_file(),
            "demo-pull fixture: the replacement site did not publish, so this is an EMPTY \
             publisher rather than one that pulled a site — a different scenario"
        );

        // And the document must still name the site that is now gone. If a
        // future change makes `publish` re-emit the config unasked, this fires
        // here, at the cause, instead of as a confusing boot two steps later.
        let raw = std::fs::read_to_string(out_root.join("entity-deployment.json"))
            .expect("the BEFORE fixture's deployment config must still be in place");
        let v: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON");
        assert_eq!(
            v["home_site"]["site"].as_str(),
            Some(crate::views::content_site::DEMO_SITE_ID),
            "demo-pull fixture: the deployment document must still declare the pulled site"
        );
        assert_eq!(
            v["home_site"]["peer"].as_str(),
            Some(peer.as_str()),
            "demo-pull fixture: the publisher identity must NOT have moved — a moved identity \
             is the re-key scenario, which heals by a different mechanism"
        );
    }

    // ── The app-republish fixtures (D24 / AP30, the cache shape) ──────────
    //
    // The scenario devops met in production, reduced to two publishes: **the
    // same publisher ships new app code under a stable identity.** Not a re-key
    // (the identity is fixed) and not a withdrawal (nothing leaves the tree) —
    // an ordinary content update, which is the case no gate in this repo has
    // ever covered, because **nothing here visits an origin twice across a
    // publish.**
    //
    // The catalog is deliberately IDENTICAL across the two publishes. That is
    // not a simplification, it is the production shape: `AppEntry` carries no
    // content hash and no version (`src/apps/format.rs`), so two publishes with
    // entirely different app code emit byte-identical catalogs — devops measured
    // exactly that, 139/139 objects byte-identical at the edge. The **only**
    // thing that moves between v1 and v2 is the bundle, which is precisely the
    // artifact a returning profile never re-reads.
    //
    // Consumed by `tests/e2e_worker.rs`
    // `an_app_republished_under_a_stable_identity_reaches_a_returning_profile`.

    /// The publisher that republishes. Distinct from the re-key and demo seeds
    /// (asserted by `rekey_fixture_seeds_are_three_distinct_identities`'s
    /// sibling below) so a fixture that published under the wrong identity
    /// cannot pass as this scenario.
    const APP_REPUBLISH_SEED: [u8; 32] = *b"entity-app-republish-seed-v1\0\0\0\0";

    /// The app id both publishes ship. Stable across the pair — a moved id would
    /// make this an "app added" test, which already passes today (the catalog
    /// refresh covers it) and is not the defect.
    const APP_REPUBLISH_ID: &str = "marker-app";

    /// Publish the demo site set + a locked-free `chrome` deployment config +
    /// ONE app whose bundle body carries `marker`, under a FIXED identity.
    ///
    /// `--surface=chrome` because the consumer opens the Apps window from the
    /// palette: a `site`/`window` surface boots into the overlay or the Site
    /// Browser and the spawn button is not where the test reaches for it.
    ///
    /// The deployment config is re-emitted on both publishes (unlike the
    /// demo-pull fixture, which deliberately withholds it) because nothing about
    /// it changes here — the routing is stable and only the bytes move. Emitting
    /// it twice keeps the second publish a pure content update.
    fn emit_app_republish_fixture(marker: &str) {
        let out = std::env::var("ENTITY_REKEY_OUT").unwrap_or_else(|_| "dist".to_string());

        // An entity-apps `dist/`: `index.json` (the catalog) + one
        // `<id>.html` self-contained bundle. See `crate::apps::ingest::read_dist`.
        let apps_dist = std::env::temp_dir().join(format!("entity-app-republish-{marker}"));
        let _ = std::fs::remove_dir_all(&apps_dist);
        std::fs::create_dir_all(&apps_dist).expect("stage the apps dist dir");
        std::fs::write(
            apps_dist.join("index.json"),
            format!(
                r#"[{{"id":"{APP_REPUBLISH_ID}","name":"Marker App",
                     "description":"Prints the build it was published from",
                     "type":"tool","saves":false}}]"#
            ),
        )
        .expect("write index.json");
        // The marker is in the BODY, so it reaches the iframe `srcdoc` the
        // consumer reads. A marker in a comment or an attribute would be just as
        // detectable and far less honest about whether the app actually renders.
        std::fs::write(
            apps_dist.join(format!("{APP_REPUBLISH_ID}.html")),
            format!(
                "<!doctype html><html><head><title>Marker App</title></head>\
                 <body><h1>{marker}</h1></body></html>"
            ),
        )
        .expect("write the bundle");

        let hex = crate::vault_codec::seed_to_hex(&APP_REPUBLISH_SEED);
        let _ = run(&[
            "publish".to_string(),
            out.clone(),
            "--deployment-config".to_string(),
            // Defines this scenario's domain — see `emit_deployment_config_fixture`.
            "--set-home".to_string(),
            "--surface=chrome".to_string(),
            format!("--identity-seed={hex}"),
            format!("--ingest-apps={}", apps_dist.display()),
        ]);

        // Asserted on the ARTIFACT, not on the exit code: what the browser meets
        // is the served tree. A publish that reported success while emitting no
        // bundle would stage a scenario that proves nothing (the same
        // vacuous-pass shape `--exact` guards one layer up).
        let peer = entity_crypto::Keypair::from_seed(APP_REPUBLISH_SEED).peer_id().to_string();
        let out_root = std::path::Path::new(&out);
        let bundle = out_root
            .join(&peer)
            .join("apps")
            .join("apps") // set id: `type: "tool"` → `set_for_type` → the `apps` set
            .join("bundles")
            .join(format!("{APP_REPUBLISH_ID}.bin"));
        assert!(
            bundle.is_file(),
            "app-republish fixture: no bundle at {} — the app did not publish",
            bundle.display()
        );
        assert!(
            out_root.join("entity-deployment.json").is_file(),
            "app-republish fixture: deployment config not emitted into {out}/"
        );
    }

    // ── A3: a site MANIFEST that moves under a stable identity ──────────
    //
    // The artifact A3 is about. `precache_origin_sites` used to skip any
    // manifest it already held (`if already.contains(…) { continue; }`) while
    // its sibling `warm_peer_sites` refetched every one — two functions that
    // their own comments call siblings, disagreeing about the trigger.
    //
    // **What the stale copy costs, corrected against the code** (the audit said
    // "the directory listing — which sites a peer appears to have, and their
    // titles"; the rail renders `SiteEntry`, which carries no title at all):
    //
    //   * NOT page bodies — `resolve_closure_via` is a pure-network two-hop with
    //     no store read, so a live navigation always re-resolves.
    //   * NOT which sites appear — a NEWLY published site was never in the skip
    //     set, so the old code fetched it too. A gate built on "a new site shows
    //     up" would be green with the fix reverted (AP31).
    //   * The one surface a *cached* manifest reaches is the **manifest-pinned
    //     shell**: the site chrome (title + nav) rendered from the durable copy
    //     when the live resolve cannot answer — offline, or an origin that is
    //     down. That is where a stale manifest is visible, and it is what the
    //     gate asserts.
    //
    // So this fixture publishes TWO sites under one identity and moves the
    // second one's title. The second site is the discriminator on purpose: the
    // consumer never navigates to it, so the resolver's own cache write-through
    // never touches it and the ONLY thing that can refresh its stored manifest
    // is the boot sweep — which is A3.
    const MANIFEST_REPUBLISH_SEED: [u8; 32] = *b"entity-manifest-republish-seed\0\0";

    /// The site the consumer lands on (its home). Navigated, so its manifest is
    /// refreshed by the resolver regardless of the sweep — which is exactly why
    /// it is NOT the site under test.
    const MR_HOME_SITE: &str = "front";
    /// The site the consumer never opens until the origin is dead. Only the boot
    /// sweep can have refreshed this one.
    const MR_QUIET_SITE: &str = "quiet";

    fn emit_manifest_republish_fixture(quiet_title: &str) {
        let out = std::env::var("ENTITY_REKEY_OUT").unwrap_or_else(|_| "dist".to_string());
        let render = std::env::temp_dir().join("entity-manifest-republish-render");
        let _ = std::fs::remove_dir_all(&render);

        for (id, title) in [(MR_HOME_SITE, "Front Desk"), (MR_QUIET_SITE, quiet_title)] {
            let dir = render.join(id);
            std::fs::create_dir_all(dir.join("pages")).expect("stage the render dir");
            std::fs::write(
                dir.join("site.manifest.json"),
                format!(r#"{{"site_id":"{id}","title":"{title}"}}"#),
            )
            .expect("write site.manifest.json");
            std::fs::write(
                dir.join("pages").join("index.md"),
                format!("# {title}\n\nThe {id} site.\n"),
            )
            .expect("write the landing page");
        }

        let hex = crate::vault_codec::seed_to_hex(&MANIFEST_REPUBLISH_SEED);
        let _ = run(&[
            "publish".to_string(),
            out.clone(),
            "--deployment-config".to_string(),
            // Defines this scenario's domain — see `emit_deployment_config_fixture`.
            "--set-home".to_string(),
            "--surface=site".to_string(),
            format!("--config-site={MR_HOME_SITE}"),
            format!("--identity-seed={hex}"),
            format!("--ingest={}", render.display()),
        ]);

        // Asserted on the ARTIFACT: a publish that reported success while
        // emitting one site would stage a scenario with no discriminator in it.
        let peer = entity_crypto::Keypair::from_seed(MANIFEST_REPUBLISH_SEED).peer_id().to_string();
        let root = std::path::Path::new(&out).join(&peer).join("sites");
        for id in [MR_HOME_SITE, MR_QUIET_SITE] {
            assert!(
                root.join(id).join("manifest.bin").is_file(),
                "manifest-republish fixture: no manifest for '{id}' — the publish emitted \
                 fewer sites than the scenario needs"
            );
        }
        let doc = std::fs::read_to_string(std::path::Path::new(&out).join("entity-deployment.json"))
            .expect("the deployment config must be emitted");
        assert!(
            doc.contains(MR_HOME_SITE),
            "manifest-republish fixture: the document must home on '{MR_HOME_SITE}', so the \
             quiet site stays un-navigated: {doc}"
        );
    }

    /// The build the returning visitor cached.
    #[test]
    #[ignore = "e2e fixture generator; run by the e2e harness via --ignored"]
    fn emit_manifest_republish_v1() {
        emit_manifest_republish_fixture("Quiet Corner");
    }

    /// The republish. Same identity, same site ids, the quiet site RETITLED.
    #[test]
    #[ignore = "e2e fixture generator; run by the e2e harness via --ignored"]
    fn emit_manifest_republish_v2() {
        // **Disjoint from v1's title, not an extension of it.** "Quiet Corner
        // Renamed" would make the gate's negative half ("the OLD title is not
        // still on screen") unsatisfiable by construction, because the new
        // string contains the old one. Measured, by writing it that way first —
        // and the same trap the app fixture avoids with V1/V2 markers.
        emit_manifest_republish_fixture("Back Room");
    }

    /// The build a returning visitor met first.
    #[test]
    #[ignore = "e2e fixture generator; run by the e2e harness via --ignored"]
    fn emit_app_republish_v1() {
        emit_app_republish_fixture("APP-MARKER-V1");
    }

    /// The republish. **Same identity, same catalog, new bundle bytes.**
    #[test]
    #[ignore = "e2e fixture generator; run by the e2e harness via --ignored"]
    fn emit_app_republish_v2() {
        emit_app_republish_fixture("APP-MARKER-V2");
    }

    /// The app-republish publisher must not collide with any other fixture
    /// identity. Same rationale as the re-key seed check: a collision would let
    /// the e2e publish over another fixture's tree and pass while testing
    /// something else.
    #[test]
    fn app_republish_seed_is_a_distinct_identity() {
        let pid = |s: [u8; 32]| entity_crypto::Keypair::from_seed(s).peer_id();
        let mine = pid(APP_REPUBLISH_SEED);
        assert_ne!(mine, pid(REKEY_SEED_BEFORE), "collides with REKEY_SEED_BEFORE");
        assert_ne!(mine, pid(REKEY_SEED_AFTER), "collides with REKEY_SEED_AFTER");
        assert_ne!(mine, pid(DEMO_PUBLISH_SEED), "collides with the demo publisher");
        assert_ne!(mine, pid(MANIFEST_REPUBLISH_SEED), "collides with the manifest fixture");
    }

    /// Same rule for the manifest-republish publisher, and the same reason: a
    /// collision would let one fixture publish over another's tree and pass
    /// while testing something else.
    #[test]
    fn manifest_republish_seed_is_a_distinct_identity() {
        let pid = |s: [u8; 32]| entity_crypto::Keypair::from_seed(s).peer_id();
        let mine = pid(MANIFEST_REPUBLISH_SEED);
        assert_ne!(mine, pid(REKEY_SEED_BEFORE), "collides with REKEY_SEED_BEFORE");
        assert_ne!(mine, pid(REKEY_SEED_AFTER), "collides with REKEY_SEED_AFTER");
        assert_ne!(mine, pid(DEMO_PUBLISH_SEED), "collides with the demo publisher");
        assert_ne!(mine, pid(APP_REPUBLISH_SEED), "collides with the app fixture");
    }

    /// The fixtures must name two DIFFERENT publishers, and neither may collide
    /// with the demo seed. Cheap, but it is the one property the whole
    /// reproduction rests on: if these ever converged, the e2e would publish,
    /// "re-key" to the same identity, and pass while testing nothing.
    #[test]
    fn rekey_fixture_seeds_are_three_distinct_identities() {
        let pid = |s: [u8; 32]| entity_crypto::Keypair::from_seed(s).peer_id();
        let (before, after, demo) =
            (pid(REKEY_SEED_BEFORE), pid(REKEY_SEED_AFTER), pid(DEMO_PUBLISH_SEED));
        assert_ne!(before, after, "the re-key fixtures must not share a publisher");
        assert_ne!(before, demo, "REKEY_SEED_BEFORE collides with the demo publisher");
        assert_ne!(after, demo, "REKEY_SEED_AFTER collides with the demo publisher");
    }

    /// Cut 2b producer↔consumer: the `--deployment-config` JSON `publish`
    /// emits must parse back through the SPA's [`crate::deployment_config`]
    /// reader with the home/origin/posture intact (round-trip safety — the file
    /// is the contract between the two halves).
    #[test]
    fn deployment_config_emit_round_trips_through_consumer() {
        use crate::deployment_config::DeploymentConfig;

        let dir = tempfile::tempdir().unwrap();
        let (peer_id, sites, _games) =
            resolve_publish_source(entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED), None, None)
                .unwrap();
        let spec = DeployConfigSpec {
            surface: "site".to_string(),
            window_type: String::new(),
            locked: true,
            site: None,            // → demo
            origin: String::new(), // same-origin
            registry_pin: None,
            set_home: false,
        };
        let path = emit_deployment_config(dir.path(), &peer_id, &sites, &spec).unwrap();
        assert_eq!(path.file_name().unwrap(), "entity-deployment.json");

        let json = std::fs::read_to_string(&path).unwrap();
        let cfg = DeploymentConfig::parse(&json).expect("emitted config parses");
        assert!(!cfg.is_empty());
        assert_eq!(cfg.surface.as_deref(), Some("site"));
        let home = cfg.home_site.clone().expect("home_site present");
        assert_eq!(home.peer_id, peer_id, "home peer = the publish peer");
        assert_eq!(home.id, "demo", "default home = demo site");
        assert_eq!(
            cfg.origins.get(&peer_id).map(String::as_str),
            Some(""),
            "same-origin: empty origin (SPA expands at runtime)"
        );
        // The locked kiosk spells out its posture explicitly (no preset).
        assert_eq!(cfg.site_mode.show_toggle, Some(false));
        assert_eq!(cfg.site_mode.locked, Some(true));
        assert_eq!(cfg.peer_creation_enabled, Some(false));

        // And it applies cleanly over a default (chrome) build config — proving a
        // generic bundle would adopt the locked-site posture + this home.
        let applied = cfg.apply_to(crate::session_config::SessionConfig::default());
        assert_eq!(applied.boot_surface, crate::session_config::BootSurface::Site);
        assert_eq!(applied.home_site.peer_id, peer_id);
        assert!(!applied.site_mode.show_toggle);
        assert!(!applied.peer_creation_enabled);
    }

    /// **The §7.4 pin, emitter to consumer, plus the refusals** — one test,
    /// because the pin only means anything if the same bytes make the whole trip.
    ///
    /// The refusals are asserted **on the message**, not on an exit code. That
    /// is the F2 lesson applied to a new flag: `--bind`'s help taught a spelling
    /// the parser rejected and the refusal blamed a missing argument, and every
    /// path exits 1, so no exit-code test could ever have caught it. A refusal
    /// that is right for the wrong reason sends the operator to the wrong file.
    #[test]
    fn a_registry_pin_is_validated_at_the_emitter_and_survives_to_the_consumer() {
        use crate::deployment_config::DeploymentConfig;

        // A real canonical-form peer-id — the same derivation a registry emit
        // prints, so this is the string an operator would actually paste.
        let registry_pid =
            entity_crypto::Keypair::from_seed([0xB0; 32]).peer_id().as_str().to_string();

        // (a) Accepted, both spellings.
        let pin = parse_registry_pin(&format!("{registry_pid}@https://reg.example"))
            .expect("PEER_ID@ORIGIN parses");
        assert_eq!(pin.peer_id, registry_pid);
        assert_eq!(pin.origin, "https://reg.example");
        assert_eq!(
            parse_registry_pin(&registry_pid).expect("a bare peer-id is same-origin").origin,
            "",
            "a bare PEER_ID means same-origin"
        );
        // An origin containing `@` survives: the split is on the FIRST one.
        assert_eq!(
            parse_registry_pin(&format!("{registry_pid}@http://user@host")).unwrap().origin,
            "http://user@host"
        );

        // (b) Refused, each for its own stated reason.
        let half = parse_registry_pin(&format!("{registry_pid}@")).unwrap_err();
        assert!(half.contains("PEER_ID@ORIGIN"), "the half-written form names both forms: {half}");
        let no_peer = parse_registry_pin("@https://reg.example").unwrap_err();
        assert!(
            no_peer.contains("peer-id"),
            "an origin alone must be refused as not-a-pin, not silently accepted: {no_peer}"
        );
        // The one that matters most: a peer-id that is well-formed but carries
        // no public key. It would emit happily and fail in a browser console as
        // "the pinned registry is unusable" — on a machine the operator is not
        // sitting at (audit F9's shape: refuse where someone can fix it).
        let legacy = parse_registry_pin("not-a-canonical-peer-id").unwrap_err();
        assert!(
            legacy.contains("canonical-form") && legacy.contains("public key"),
            "the refusal must say WHY a pin needs canonical form: {legacy}"
        );

        // (c) Emitted, and read back by the consumer that has to act on it.
        let dir = tempfile::tempdir().unwrap();
        let (peer_id, sites, _apps) =
            resolve_publish_source(entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED), None, None)
                .unwrap();
        let spec = DeployConfigSpec {
            surface: "site".to_string(),
            window_type: String::new(),
            locked: false,
            site: None,
            origin: String::new(),
            registry_pin: Some(pin.clone()),
            set_home: false,
        };
        let path = emit_deployment_config(dir.path(), &peer_id, &sites, &spec).unwrap();
        let cfg = DeploymentConfig::parse(&std::fs::read_to_string(&path).unwrap())
            .expect("emitted config parses");
        assert_eq!(cfg.name_registry_pin.as_ref(), Some(&pin), "the pin must survive the emit");
        // …and all the way onto the durable spine a warm boot reads.
        let applied = cfg.apply_to(crate::session_config::SessionConfig::default());
        assert_eq!(applied.name_registry_pin, Some(pin));

        // (d) No pin asked for, no key emitted — a deployment that seeds nothing
        // must not ship an empty pin that reads as a registry resolving nothing.
        let spec_none = DeployConfigSpec {
            surface: "site".to_string(),
            window_type: String::new(),
            locked: false,
            site: None,
            origin: String::new(),
            registry_pin: None,
            set_home: false,
        };
        let path = emit_deployment_config(dir.path(), &peer_id, &sites, &spec_none).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("name_registry_pin"), "no pin asked for, no key emitted: {raw}");
    }

    /// A `--surface=window` deployment must emit the overlay **off**
    /// (`site_mode.enabled=false, show_toggle=false`) so the status-bar
    /// "View Site" toggle does NOT appear — the window IS the content surface,
    /// and on a fresh peer the overlay's default home resolves to a missing local
    /// site (`No site manifest at 'demo'`, bug report 2026-07-02). The applied
    /// config must therefore expose no toggle.
    #[test]
    fn window_emit_disables_the_overlay_toggle() {
        use crate::deployment_config::DeploymentConfig;

        let dir = tempfile::tempdir().unwrap();
        let (peer_id, sites, _games) =
            resolve_publish_source(entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED), None, None)
                .unwrap();
        let spec = DeployConfigSpec {
            surface: "window".to_string(),
            window_type: "Site Browser".to_string(),
            locked: false,
            site: None,
            origin: String::new(),
            registry_pin: None,
            set_home: false,
        };
        let path = emit_deployment_config(dir.path(), &peer_id, &sites, &spec).unwrap();
        let cfg = DeploymentConfig::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(cfg.surface.as_deref(), Some("window"));
        assert_eq!(cfg.window_type.as_deref(), Some("Site Browser"));
        assert_eq!(cfg.site_mode.enabled, Some(false), "window: overlay disabled");
        assert_eq!(cfg.site_mode.show_toggle, Some(false), "window: no status-bar toggle");

        // Applied over a default build config: the surface is the window, and the
        // overlay exposes NO toggle (the bug: a stray View Site button).
        let applied = cfg.apply_to(crate::session_config::SessionConfig::default());
        assert!(
            matches!(applied.boot_surface, crate::session_config::BootSurface::Window { .. }),
            "boots into the Site Browser window"
        );
        assert!(
            !applied.site_mode.exposes_toggle(),
            "a window deployment must expose no 'View Site' toggle"
        );
    }

    /// The deployment-config origin folds the hosting prefix in. Empty prefix →
    /// today's value (byte-identical); a prefix nests it — `{live}/{prefix}` for
    /// a cross-origin host, `/{prefix}` (root-relative) for same-origin.
    #[test]
    fn deploy_origin_folds_in_the_prefix() {
        // Root: unchanged from before the knob existed.
        assert_eq!(deploy_origin(None, ""), "");
        assert_eq!(deploy_origin(Some("https://host"), ""), "https://host");
        // Same-origin + prefix → root-relative (SPA's expand_origin prepends).
        assert_eq!(deploy_origin(None, "hosted-peers/PEERX"), "/hosted-peers/PEERX");
        // Cross-origin host + prefix → concrete `{host}/{prefix}` (trailing
        // slash on the host trimmed, no double slash).
        assert_eq!(deploy_origin(Some("https://host"), "alice"), "https://host/alice");
        assert_eq!(deploy_origin(Some("https://host/"), "alice"), "https://host/alice");
    }

    /// The D13 loopback guard: a published deployment-config origin is "not
    /// portable" exactly when it bakes a loopback host. Empty (same-origin),
    /// root-relative, and real hosts are all portable; localhost/127.0.0.1 — in
    /// any position, with or without a port/prefix — is the footgun we flag.
    #[test]
    fn loopback_origin_is_detected_for_the_guard() {
        // Portable: the production forms.
        assert!(!origin_is_loopback(""), "empty = same-origin = portable");
        assert!(!origin_is_loopback("/hosted/PEERX"), "root-relative same-origin");
        assert!(!origin_is_loopback("https://docs.example.com"), "real host");
        assert!(!origin_is_loopback("https://host.example/app"), "real host + base path");
        // The footgun: loopback in any shape.
        assert!(origin_is_loopback("http://localhost:8081"));
        assert!(origin_is_loopback("http://127.0.0.1:8099"));
        assert!(origin_is_loopback("http://localhost:8081/tenant-a"), "loopback + prefix");
    }

    /// Bare-root site selection: explicit `--site=` wins; otherwise the demo
    /// site is preferred over the first read; an unknown id yields None.
    #[test]
    fn pick_bare_site_prefers_explicit_then_demo() {
        let (_pid, sites, _games) =
            resolve_publish_source(entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED), None, None)
                .unwrap();
        // No filter → the demo site (even though entity-info may sort first).
        assert_eq!(pick_bare_site(&sites, None).map(|s| s.site_id.as_str()), Some("demo"));
        // Explicit id wins.
        assert_eq!(
            pick_bare_site(&sites, Some(INFO_SITE_ID)).map(|s| s.site_id.as_str()),
            Some(INFO_SITE_ID)
        );
        // Unknown id → None.
        assert!(pick_bare_site(&sites, Some("ghost")).is_none());
    }
}
