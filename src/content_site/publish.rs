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
/// - `--deployment-config` — also emit `/entity-deployment.json` (cut 2b): the
///   per-domain config that points a **generic** SPA bundle served from this
///   origin at the published home site. `--config-profile=<full|tutorial|
///   strict-site>` (default `tutorial` — boots into the site but keeps the
///   chrome toggle so the author is never locked in; `strict-site` is the
///   opt-in kiosk, escapable only via `?chrome=1`) sets the posture;
///   `--config-site=ID`
///   (default: demo, else first) sets the home site. The origin is the `--live`
///   value if given, else `""` (same-origin — the SPA expands it to its own
///   origin at runtime). Projection mode only.
/// - `--identity-seed=<64-hex>` — publish under a **specific system identity**
///   (the same 32-byte hex seed form as the runtime `entity_system_seed`), so
///   each site/deployment gets its own stable peer-id (`sites/{peer}/…`).
///   Default (absent) is the fixed demo publisher seed — the first-push
///   identity. A malformed seed fails the build.
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
    // Cut 2b: optionally emit the per-domain deployment config.
    let deployment_config = args.iter().any(|a| a == "--deployment-config");
    let config_profile: String = args
        .iter()
        .find_map(|a| a.strip_prefix("--config-profile=").map(str::to_string))
        .unwrap_or_else(|| "tutorial".to_string());
    let config_site: Option<String> =
        args.iter().find_map(|a| a.strip_prefix("--config-site=").map(str::to_string));
    // `--identity-seed=<64-hex>` publishes under a SPECIFIC system identity (the
    // same hex seed form as the runtime `entity_system_seed`), so each site /
    // deployment gets its own stable peer-id. Absent → the fixed demo publisher
    // identity (the first-push default). Validated up front (fail on a bad seed
    // rather than silently publishing under the wrong identity).
    let seed = match publish_seed(args) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("publish {e}");
            return ExitCode::FAILURE;
        }
    };
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

    // Validate the deployment-config profile up front (fail the build on a typo,
    // like `ENTITY_PROFILE` does) — before any filesystem work.
    if deployment_config && crate::session_config::Profile::from_str(&config_profile).is_none() {
        eprintln!(
            "publish --deployment-config: unknown --config-profile={config_profile:?} \
             (expected full | tutorial | strict-site)"
        );
        return ExitCode::FAILURE;
    }
    if deployment_config && bare_root {
        eprintln!(
            "publish: --deployment-config is for the SPA projection (sites/{{peer}}/…), \
             not --bare-root (single site at domain root) — drop one."
        );
        return ExitCode::FAILURE;
    }
    // Validate + normalize the hosting prefix at the CLI boundary (reject `..`,
    // leading/trailing `/`, reserved first segment). `--bare-root` IS the root,
    // so a prefix there is a contradiction — fail rather than silently ignore.
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

    // [A] Resolve the source peer + read all its sites AND app sets off the tree.
    let (peer_id, sites, app_sets) =
        match resolve_publish_source(seed, ingest_dir.as_deref(), ingest_apps.as_deref()) {
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
            DeployConfigSpec { profile: config_profile, site: config_site, origin }
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

/// What `--deployment-config` emits — the home posture + site + serving origin
/// for the published SPA. Resolved from the CLI flags in [`run`].
struct DeployConfigSpec {
    /// Posture preset name (`full` / `tutorial` / `strict-site`), validated.
    profile: String,
    /// Home site id; `None` → the demo site, else the first published.
    site: Option<String>,
    /// Serving origin for the published peer; `""` = same-origin.
    origin: String,
}

/// Projection mode: every site under `sites/{peer}/{site}/…` + a peer index
/// (legacy-web `.html`), and — when `emit_bin` — the entity-native `.bin`
/// content data alongside it ([B2]).
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
) -> ExitCode {
    // Clean only the projection roots we own, UNDER the hosting prefix — the
    // `.html` tree (`{out}/{prefix}/sites`) and, when emitting it, the `.bin`
    // content data (`{out}/{prefix}/content` + `{out}/{prefix}/{peer}`). Not the
    // whole output dir, so publishing into a populated dir doesn't nuke
    // unrelated files (e.g. a sibling peer's prefix), while stale artifacts
    // don't linger.
    let base = paths::prefixed_root(out_dir, prefix);
    let mut clean: Vec<std::path::PathBuf> = vec![base.join(SITE_URL_PREFIX)];
    if emit_bin {
        clean.push(base.join("content"));
        clean.push(base.join(peer_id));
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
    let pages = match static_export::export_owned_sites(out_dir, sites, prefix, live_base) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("publish: HTML export failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    // [B2] Entity-native `.bin` content data (the form a live peer ingests).
    if emit_bin {
        if let Err(e) = crate::content_site::publish_fixture::emit_owned_sites(out_dir, sites, prefix) {
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
                out_dir, peer_id, set, &ing.catalog, &ing.bundles, prefix,
            ) {
                Ok(n) => println!("  apps[{set}]: {n} bundle(s) → {}/apps/{}/", peer_id, set),
                Err(e) => {
                    eprintln!("publish: app-set '{set}' (.bin) export failed: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
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
/// `site_mode` posture is derived from the profile preset so the file is
/// self-consistent with the consumer's [`crate::deployment_config`] semantics.
/// The output is round-trip-verified by `DeploymentConfig::parse` in tests.
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
    // Posture from the (already-validated) profile preset — keeps the emitted
    // `site_mode` consistent with what the profile alone would imply.
    let posture = crate::session_config::Profile::from_str(&spec.profile)
        .expect("profile validated in run()")
        .preset()
        .site_mode;

    let mut origins = serde_json::Map::new();
    origins.insert(peer_id.to_string(), serde_json::Value::String(spec.origin.clone()));

    let doc = serde_json::json!({
        "profile": spec.profile,
        "home_site": { "peer": peer_id, "site": site_id, "loc": "" },
        "origins": origins,
        "site_mode": {
            "enabled": posture.enabled,
            "show_toggle": posture.show_toggle,
            "locked": posture.locked,
        },
    });

    let path = out_dir.join("entity-deployment.json");
    std::fs::write(&path, format!("{}\n", serde_json::to_string_pretty(&doc)?))?;
    Ok(path)
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
/// (the "ids are fucked" symptom + the live→static 404). A fixed seed makes
/// the demo *addressable*. **This is the demo identity, not a real deployment
/// identity** — a real hosting peer loads its own durable keypair (the deferred
/// native peer-load seam below); this just stops the demo's id from drifting.
/// The bytes are arbitrary-but-fixed ("entity-demo-publisher" padded).
const DEMO_PUBLISH_SEED: [u8; 32] = *b"entity-demo-publisher-seed-v1\0\0\0";

/// The publisher keypair for a given 32-byte system seed (stable peer-id for a
/// given seed). `DEMO_PUBLISH_SEED` is the default; `--identity-seed=<hex>`
/// supplies any other system identity so each site/deployment publishes under
/// its own stable peer-id (`publish_seed`).
fn publish_identity_keypair(seed: [u8; 32]) -> entity_crypto::Keypair {
    entity_crypto::Keypair::from_seed(seed)
}

/// Resolve the publish identity seed from the CLI: `--identity-seed=<64-hex>`
/// (the same hex form as the runtime `entity_system_seed`), else the fixed demo
/// publisher seed. `Err` on a malformed hex seed so a typo fails the build
/// rather than silently publishing under the wrong (demo) identity.
fn publish_seed(args: &[String]) -> Result<[u8; 32], String> {
    match args.iter().find_map(|a| a.strip_prefix("--identity-seed=")) {
        Some(hex) => crate::vault_codec::hex_to_seed(hex.trim()).ok_or_else(|| {
            format!(
                "--identity-seed expects 64 hex chars (a 32-byte system seed), got {:?}",
                hex
            )
        }),
        None => Ok(DEMO_PUBLISH_SEED),
    }
}

/// Resolve which peer to publish and read all its sites. **The seam** (see
/// the module docs): today it builds a Direct peer **with the fixed demo
/// publisher identity** ([`publish_identity_keypair`]) and seeds the bundled
/// demo site set, then reads it back off the tree — so publish is a faithful
/// end-to-end exercise of the [A] reader on demo data, under a *stable* peer-id.
/// The publisher identity is the supplied `seed` (`--identity-seed=<hex>`, else
/// the demo seed) — so each deployment can publish under its own stable peer-id.
/// Replace this body with "open a persisted peer dir → read its real sites"
/// when the durable native peer-load path lands; nothing downstream changes,
/// and the publisher identity becomes the loaded peer's own (still stable).
fn resolve_publish_source(
    seed: [u8; 32],
    ingest_dir: Option<&Path>,
    ingest_apps: Option<&Path>,
) -> Result<(String, Vec<read::OwnedSite>, crate::apps::ingest::IngestedSets), String> {
    let peers = Peers::new_direct_with_keypair(publish_identity_keypair(seed));
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
        let (peer_id, sites, app_sets) = resolve_publish_source(DEMO_PUBLISH_SEED, None, None).unwrap();
        assert!(!peer_id.is_empty());

        // The publisher identity is STABLE across runs — the whole point of
        // the fixed demo seed (no more shifting peer-ids / broken permalinks).
        let (peer_id_again, _, _) = resolve_publish_source(DEMO_PUBLISH_SEED, None, None).unwrap();
        assert_eq!(peer_id, peer_id_again, "publish peer-id must be reproducible");

        // A DIFFERENT system seed → a different (but still reproducible) peer-id,
        // so each deployment can publish under its own identity (`--identity-seed`).
        let other_seed = *b"different-publisher-seed-here!!!";
        let (other_pid, _, _) = resolve_publish_source(other_seed, None, None).unwrap();
        assert_ne!(peer_id, other_pid, "a custom seed must yield its own peer-id");
        let (other_pid_again, _, _) = resolve_publish_source(other_seed, None, None).unwrap();
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

    /// `--identity-seed` parsing: absent → demo default; a 64-hex seed parses to
    /// those exact bytes; a malformed seed is a loud error (not a silent
    /// fall-back to the demo identity).
    #[test]
    fn publish_seed_parses_identity_or_defaults() {
        // Absent → the demo publisher seed (the first-push default).
        assert_eq!(publish_seed(&[]).unwrap(), DEMO_PUBLISH_SEED);

        // A round-tripped hex seed parses back to the exact bytes.
        let custom = *b"per-site-publisher-identity-0001";
        let hex = crate::vault_codec::seed_to_hex(&custom);
        let arg = vec![format!("--identity-seed={hex}")];
        assert_eq!(publish_seed(&arg).unwrap(), custom);

        // Malformed (wrong length / non-hex) → Err, so a typo fails the build.
        assert!(publish_seed(&["--identity-seed=deadbeef".to_string()]).is_err());
        assert!(publish_seed(&["--identity-seed=not-hex".to_string()]).is_err());
    }

    /// E2E fixture generator (run by `tests/e2e_worker.rs` Phase 27 via
    /// `cargo test --bin entity-browser emit_deployment_config_fixture -- --ignored`,
    /// NOT the unit suite — hence `#[ignore]`). Publishes the demo set plus a
    /// strict-site, same-origin deployment config into `dist/`, so the served
    /// SPA (a generic `Full` build) can `GET /entity-deployment.json`, apply it,
    /// and boot into the published home over same-origin HTTP-poll. Mirrors the
    /// `emit_e2e_fixture` pattern. Asserts the config landed so a silent publish
    /// failure surfaces here, not as a confusing phase-27 boot.
    #[test]
    #[ignore = "e2e fixture generator; run by the e2e harness via --ignored"]
    fn emit_deployment_config_fixture() {
        // Same as `make publish dist DEPLOY_CONFIG=1`: emits the `.bin` content
        // (sites/{peer}/… + content/…) AND dist/entity-deployment.json pointing
        // a generic SPA at this origin's published demo (origin "" = same-origin).
        let _ = run(&["publish".to_string(), "dist".to_string(), "--deployment-config".to_string()]);
        assert!(
            std::path::Path::new("dist/entity-deployment.json").exists(),
            "deployment config not emitted into dist/ — publish --deployment-config failed"
        );
    }

    /// Cut 2b producer↔consumer: the `--deployment-config` JSON `publish`
    /// emits must parse back through the SPA's [`crate::deployment_config`]
    /// reader with the home/origin/posture intact (round-trip safety — the file
    /// is the contract between the two halves).
    #[test]
    fn deployment_config_emit_round_trips_through_consumer() {
        use crate::deployment_config::DeploymentConfig;
        use crate::session_config::Profile;

        let dir = tempfile::tempdir().unwrap();
        let (peer_id, sites, _games) = resolve_publish_source(DEMO_PUBLISH_SEED, None, None).unwrap();
        let spec = DeployConfigSpec {
            profile: "strict-site".to_string(),
            site: None,            // → demo
            origin: String::new(), // same-origin
        };
        let path = emit_deployment_config(dir.path(), &peer_id, &sites, &spec).unwrap();
        assert_eq!(path.file_name().unwrap(), "entity-deployment.json");

        let json = std::fs::read_to_string(&path).unwrap();
        let cfg = DeploymentConfig::parse(&json).expect("emitted config parses");
        assert!(!cfg.is_empty());
        assert_eq!(cfg.profile, Some(Profile::StrictSite));
        let home = cfg.home_site.clone().expect("home_site present");
        assert_eq!(home.peer_id, peer_id, "home peer = the publish peer");
        assert_eq!(home.id, "demo", "default home = demo site");
        assert_eq!(
            cfg.origins.get(&peer_id).map(String::as_str),
            Some(""),
            "same-origin: empty origin (SPA expands at runtime)"
        );
        // Posture derived from the strict-site preset.
        assert_eq!(cfg.site_mode.show_toggle, Some(false));
        assert_eq!(cfg.site_mode.locked, Some(true));

        // And it applies cleanly over a default (Full) build config — proving a
        // generic bundle would adopt the strict-site posture + this home.
        let applied = cfg.apply_to(crate::session_config::SessionConfig::default());
        assert_eq!(applied.boot_surface, crate::session_config::BootSurface::Site);
        assert_eq!(applied.home_site.peer_id, peer_id);
        assert!(!applied.site_mode.show_toggle);
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
        let (_pid, sites, _games) = resolve_publish_source(DEMO_PUBLISH_SEED, None, None).unwrap();
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
