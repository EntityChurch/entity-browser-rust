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
/// - **The site arm — exactly one of `--ingest=<dir>` / `--demo-sites` /
///   `--no-sites`, and silence is refused.** See [`SiteSource`]. `--ingest`
///   sources the tree from a content-team `render/` emit (one site dir, or a
///   parent of site dirs); `--demo-sites` publishes the bundled `demo` +
///   `entity-info` set on purpose; `--no-sites` publishes none, for a feed-only
///   or apps-only domain. Not to be confused with `--demo-identity`, which picks
///   the *keypair* — the two are orthogonal and both are explicit.
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
/// - `--supersede=OLD=NEW` (repeatable, with `--deployment-config`) — declare
///   that peer `OLD` was replaced by `NEW`. A returning profile adopts it on the
///   next warm boot and rewrites its stored references to `OLD`.
///
///   **The home peer's re-key does not need this** — moving `home_site` is one
///   slot changing, which the client infers. Every *other* hosted peer does:
///   `origins` is a map, and a key leaving as another arrives is ambiguous
///   between a re-key and one tenant leaving as another joins, so the client
///   must not guess (`DESIGN-RESILIENCE…` §1.1f). Additive — an existing
///   declaration is never dropped by a later publish, including the home
///   publisher's.
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
    // `--window-target=<entity+ref://…>` — WHAT that window opens at. A window
    // type says which viewer; this says what it is looking at, and until it
    // existed a deployment could boot the Feed window and not name a publisher
    // (`DESIGN-2026-09-12-BROWSING-WITHOUT-PRIVILEGING-A-CONVENTION` §2).
    // No default: a deployment that says nothing gets today's behaviour exactly.
    let config_window_target: String = args
        .iter()
        .find_map(|a| a.strip_prefix("--window-target=").map(str::to_string))
        .unwrap_or_default();
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
    // `--supersede=OLD=NEW` (repeatable) — the succession this domain DECLARES.
    //
    // The consumer cannot infer it for anyone but the home peer: `origins` is a
    // map, and a key leaving as another arrives is ambiguous between a re-key
    // and one tenant leaving as another joins. Guessing writes a supersession
    // against a peer that is alive, so the deployer — the only party who knows —
    // says it (`DESIGN-RESILIENCE…` §1.1f item 1).
    let supersede_raw: Vec<String> =
        args.iter().filter_map(|a| a.strip_prefix("--supersede=").map(str::to_string)).collect();
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
    // The SITE arm — `--ingest=<dir>` / `--demo-sites` / `--no-sites`, exactly
    // one, and **silence is refused**. See [`SiteSource`] for why the default
    // was worse than an unwanted demo site: the clean is wholesale, so a publish
    // that meant `--ingest` and omitted it replaced a domain's real sites.
    //
    // Parsed here with the other flags, and **refused below the `--verify`
    // return**: verifying an already-published tree asks nothing about a source,
    // so requiring an arm for it would refuse a verification for a reason with
    // no bearing on it — the same ordering rule the emit-time guards follow.
    let site_source = parse_site_source(args);
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
    // `--ingest-feed=<dir>` is the third axis's front door: a directory of
    // authored posts (`*.md` with a `+++` TOML block carrying `created_at`),
    // ingested into the tree and then read back out of it like the other two.
    // See `crate::feed_ingest` — the date is required, not taken from the
    // filesystem, because mtime makes a publish irreproducible.
    let ingest_feed: Option<PathBuf> = args
        .iter()
        .find_map(|a| a.strip_prefix("--ingest-feed=").map(PathBuf::from));
    // `--gather=<peer_id>@<published-tree-dir>` (repeatable) — `APP-CONVENTION-
    // FEED` §6's gatherer as a verb: read that author's feed out of a tree they
    // published, and carry it into this publish as a mirror.
    //
    // ⚠ **A DIRECTORY, not an origin** — this tree has no native HTTP client
    // (`feed_gather`'s module doc leads with why). The topology it serves is the
    // real one it sounds like a stand-in for: several publishers share a hosting
    // scope and their trees tell them apart, so a gatherer at that origin
    // gathers a sibling with no network at all.
    let gather_raw: Vec<String> =
        args.iter().filter_map(|a| a.strip_prefix("--gather=").map(str::to_string)).collect();
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

    // The site arm, now that `--verify` has returned. Refused before any
    // filesystem work, like the surface typo below it.
    let site_source = match site_source {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("publish: {msg}");
            return ExitCode::FAILURE;
        }
    };

    // A declared arm contradicting a site-shaped mode is refused HERE, at the
    // flags, rather than falling through to the emptiness check below — that one
    // reports *"no sites found on peer X"*, which reads as a discovery about the
    // tree when the operator said it on the command line.
    //
    // ONE expression of *"this mode projects a site"*, consulted by this guard
    // and by the emptiness check below — two spellings of it is C15's drift.
    let site_shaped = bare_root || html_only;
    if site_source == SiteSource::None && site_shaped {
        eprintln!(
            "publish --no-sites: {} projects a site, and this publish declares none — drop one.",
            if bare_root { "--bare-root" } else { "--html-only" }
        );
        return ExitCode::FAILURE;
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
    // `--window-target` is validated HERE, at the CLI boundary, for
    // `parse_registry_pin`'s reason: an address the consumer will drop is
    // refused where the operator can read the refusal, never emitted for
    // somebody else to ignore in silence (audit F9).
    //
    // **Three separate refusals, because they are three different mistakes.**
    // The third is the one worth having: naming an address whose viewer is not
    // the window type declared beside it is a contradiction the deployment
    // document can express and the consumer can only answer with `Aim::NotMine`
    // — a startup window that opens and shows nothing it was pointed at.
    if !config_window_target.is_empty() {
        if !deployment_config || config_surface != "window" {
            eprintln!(
                "publish --window-target: an address rides in /entity-deployment.json on a \
                 window surface, so it needs --deployment-config --surface=window (got \
                 --surface={config_surface:?}); without both, nothing would carry it"
            );
            return ExitCode::FAILURE;
        }
        match crate::open_target::parse(&config_window_target) {
            None => {
                eprintln!(
                    "publish --window-target={config_window_target:?}: not a readable entity \
                     reference (expected APP-CONVENTION-REFERENCE §3.1, \
                     e.g. entity+ref://<peer-id>/app/feed/index)"
                );
                return ExitCode::FAILURE;
            }
            Some(address) => match crate::open_target::route(&address) {
                crate::open_target::Routing::Viewer(w)
                    if crate::window::canonical_window_type(&config_window_type) == w => {}
                crate::open_target::Routing::Viewer(w) => {
                    eprintln!(
                        "publish --window-target={config_window_target:?}: that address opens \
                         in {w:?}, but --window-type={config_window_type:?}. A window cannot \
                         show another convention's address — pick one"
                    );
                    return ExitCode::FAILURE;
                }
                other => {
                    eprintln!(
                        "publish --window-target={config_window_target:?}: no viewer handles \
                         that address ({other:?})"
                    );
                    return ExitCode::FAILURE;
                }
            },
        }
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
    // Same rule as the pin, and for the same reason: a declaration with no
    // document to ride in does nothing at all, silently.
    if !supersede_raw.is_empty() && !deployment_config {
        eprintln!(
            "publish --supersede: the declaration rides in /entity-deployment.json, so it \
             needs --deployment-config too (without it nothing would carry it)"
        );
        return ExitCode::FAILURE;
    }
    let superseded = match parse_supersessions(&supersede_raw) {
        Ok(m) => m,
        Err(msg) => {
            eprintln!("publish --supersede: {msg}");
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
    // A gather has nowhere to land in either site-shaped mode, and BOTH would
    // accept the flag and write nothing: `--bare-root` manages no `{peer}/`
    // projection at all, and `--html-only` skips the entire `.bin` axis loop. A
    // flag that is silently a no-op is the shape an operator ships a deployment
    // believing it carries something — the same refusal `--registry-pin` and
    // `--supersede` take one guard up.
    if !gather_raw.is_empty() && bare_root {
        eprintln!(
            "publish --gather: a mirror is bound at {}… under the gatherer, and \
             --bare-root manages no {{peer}}/ projection to bind it in — drop one.",
            crate::feed::mirror_prefix()
        );
        return ExitCode::FAILURE;
    }
    if !gather_raw.is_empty() && html_only {
        eprintln!(
            "publish --gather: --html-only skips the .bin projection entirely, so the \
             gathered view would be read and then not written — drop one."
        );
        return ExitCode::FAILURE;
    }
    let gathers = match parse_gathers(&gather_raw) {
        Ok(g) => g,
        Err(msg) => {
            eprintln!("publish --gather: {msg}");
            return ExitCode::FAILURE;
        }
    };

    // [A] Resolve the source peer + read every publish axis off the tree.
    let PublishSource { peer_id, sites, app_sets, feed } = match resolve_publish_source(
        keypair,
        &site_source,
        ingest_apps.as_deref(),
        ingest_feed.as_deref(),
    ) {
        Ok(source) => source,
        Err(e) => {
            // Not `publish --ingest:` — every axis ingests through this call and
            // a feed's parse error wearing the site flag's name sends an author
            // to the wrong directory.
            eprintln!("publish: {e}");
            return ExitCode::FAILURE;
        }
    };
    // **"No sites" stopped meaning "nothing to publish" when the third axis
    // landed.** The two site-shaped modes still require one — `--bare-root`
    // renders a single site AT the domain root and `--html-only` emits the
    // legacy-web projection, which only sites have — but the ordinary `.bin`
    // publish is a projection of the tree, and a tree carrying a feed and no
    // site is a publishable tree. The emptiness question is asked of the axis
    // table so a fourth convention answers it without editing this line.
    //
    // **Reachable from the CLI as of 2026-09-16 (`--no-sites`), and gated.**
    // It was written before the arm existed and said so — *"a branch nobody can
    // reach is not a branch anybody has checked"* — which is what made the arm's
    // absence findable when `<coordination-tree>` went looking for it.
    // [A2] Gather, **before anything is cleaned**. The carried bytes are held in
    // memory from here on, which is what makes `--gather=<author>@<this same
    // out-dir>` sound: the blobs behind that author's tree live in the SHARED
    // `content/` store this publish may be about to remove, so reading them
    // first is the difference between mirroring a sibling and mirroring
    // whatever survived our own clean.
    let mirrors = match resolve_gathers(&peer_id, &gathers) {
        Ok(m) => m,
        Err(msg) => {
            eprintln!("publish --gather: {msg}");
            return ExitCode::FAILURE;
        }
    };

    let carries_nothing =
        crate::publish_axes::axes(&peer_id, &sites, &app_sets, feed.as_ref(), &mirrors)
            .iter()
            .all(|a| a.incoming() == 0);
    if sites.is_empty() && (site_shaped || carries_nothing) {
        if site_shaped {
            eprintln!(
                "publish: no sites found on peer {peer_id} — {} projects a site and there is none.",
                if bare_root { "--bare-root" } else { "--html-only" }
            );
        } else {
            eprintln!("publish: peer {peer_id} carries no sites, apps or posts — nothing to publish.");
        }
        return ExitCode::FAILURE;
    }

    // A deployment config naming a home site that isn't published would point
    // the SPA at a 404 — fail fast rather than emit a broken config.
    //
    // ## The site-free deployment's posture — `B-4`'s two riding questions
    //
    // A domain with no sites has **no home site**, and the surface has to be one
    // that does not need one. Both were open questions when the arm was asked
    // for; they are answered here, at the flags, because the alternative is a
    // document that is syntactically fine and boots a visitor into nothing.
    //
    // - **`home_site` is omitted** — see [`emit_deployment_config`]. Measured
    //   rather than assumed: with no `home_site` the document affirms no peer
    //   ([`crate::deployment_config::DeploymentConfig::affirmed_home`] returns
    //   `""`), `stale_against_declared` filters the empty peer out, an empty
    //   `current` drops nothing, and `decide_home` answers `Unchanged`. So a
    //   site-free domain is inert for every returning profile rather than
    //   quietly clearing anybody's supersession records.
    // - **`--surface=site` is refused** — the overlay's entire content is a
    //   site, and on a fresh peer it resolves its default home to a missing
    //   local one (*"No site manifest at 'demo'"*).
    // - **`--surface=window` is refused unless `--window-target` names what the
    //   window opens at.** The default window type is a Site Browser whose
    //   default aim is the home site, so a window surface with no sites and no
    //   address is a real window with a plausible title and an empty rail — the
    //   shipped bug `open_target` was built to retire. With an address it is
    //   coherent and is the *point* of the arm: a Feed window on a timeline
    //   domain. `chrome` needs nothing and is the answer for everything else.
    // - **`--set-home` is refused** — it moves a home onto the peer being
    //   published, and there is none to move.
    if deployment_config {
        if let Some(id) = &config_site {
            if sites.is_empty() {
                eprintln!(
                    "publish --deployment-config: --config-site={id:?}, but this publish \
                     carries no sites — there is nothing for it to name."
                );
                return ExitCode::FAILURE;
            }
            if !sites.iter().any(|s| &s.site_id == id) {
                eprintln!(
                    "publish --deployment-config: --config-site={id:?} not among published \
                     sites {:?}",
                    sites.iter().map(|s| s.site_id.as_str()).collect::<Vec<_>>()
                );
                return ExitCode::FAILURE;
            }
        }
        if sites.is_empty() {
            if config_set_home {
                eprintln!(
                    "publish --deployment-config: --set-home moves this domain's home site \
                     onto the peer being published, and this publish carries no sites — \
                     there is no home to move."
                );
                return ExitCode::FAILURE;
            }
            match config_surface.as_str() {
                "site" => {
                    eprintln!(
                        "publish --deployment-config: --surface=site is the site overlay and \
                         this publish carries no sites — a visitor would land on a home site \
                         that is not there. Use --surface=chrome, or --surface=window with a \
                         --window-target naming what the window opens at."
                    );
                    return ExitCode::FAILURE;
                }
                "window" if config_window_target.is_empty() => {
                    eprintln!(
                        "publish --deployment-config: --surface=window with no --window-target \
                         opens {config_window_type:?} at this domain's home site, and this \
                         publish carries no sites. Name what the window opens at \
                         (--window-target=entity+ref://<peer-id>/app/feed/index), or use \
                         --surface=chrome.\nNote --surface defaults to window, so this is \
                         also what you get by saying nothing."
                    );
                    return ExitCode::FAILURE;
                }
                _ => {}
            }
        }
    }

    // `--plan`: say what this publish WOULD do to the existing projection, and
    // write nothing. Placed here — after the source is resolved (so the plan is
    // about real sites, not a guess) and before any clean — because the clean is
    // the destructive step and a plan that ran after it would be a report.
    if plan_only {
        return run_plan(
            &out_dir,
            &peer_id,
            &sites,
            &prefix,
            bare_root,
            &app_sets,
            feed.as_ref(),
            &mirrors,
            !html_only,
        );
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
                window_target: config_window_target,
                locked: config_locked,
                site: config_site,
                origin,
                registry_pin,
                set_home: config_set_home,
                superseded,
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
            feed.as_ref(),
            &mirrors,
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
    /// What that window is aimed at — an `APP-CONVENTION-REFERENCE` §3.1 address.
    /// Empty = none declared, which is every document published before this flag.
    ///
    /// **Validated at the CLI boundary**, like `registry_pin` one field along and
    /// for the identical reason: an address the consumer will drop is refused
    /// where the operator can read the refusal, never emitted for somebody else
    /// to silently ignore (audit F9).
    window_target: String,
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
    /// **`--supersede=OLD=NEW`, repeatable — the succession this domain
    /// declares** (`DESIGN-RESILIENCE…` §1.1f item 1).
    ///
    /// Merged into any existing `superseded` map rather than replacing it, the
    /// same way sibling `origins` entries are preserved: several tenants may
    /// each have re-keyed, and a publish that dropped another's declaration
    /// would be the clobber `HomeClaim` exists to prevent, one field along.
    ///
    /// **Stated limit:** this is authority by *who holds the out-dir*, not by
    /// signature — nothing here stops a secondary publish declaring a
    /// succession for a peer it does not own. That is true of every field in
    /// this document, which is domain-managed by construction, so it is not a
    /// new hole; it is written down because succession is the field where the
    /// consequence (traffic redirected to another peer) is worst.
    superseded: std::collections::BTreeMap<String, String>,
}

/// Parse `--supersede=OLD=NEW` occurrences into a succession map.
///
/// Refuses at the CLI boundary rather than emitting something a consumer drops
/// silently — audit F9's rule, the same one `parse_registry_pin` follows. The
/// three refusals are the three shapes `DeploymentConfig::parse` would discard:
/// a missing side, a self-loop, and (added here, because only the emitter can
/// see it) the same peer retired twice to different replacements, which is a
/// typo the map would otherwise resolve by silently keeping the last one.
fn parse_supersessions(
    raw: &[String],
) -> Result<std::collections::BTreeMap<String, String>, String> {
    let mut out: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for entry in raw {
        let (retired, replacement) = entry
            .split_once('=')
            .ok_or_else(|| format!("expected OLD=NEW, got {entry:?}"))?;
        let (retired, replacement) = (retired.trim(), replacement.trim());
        if retired.is_empty() || replacement.is_empty() {
            return Err(format!("both peer ids are required, got {entry:?}"));
        }
        if retired == replacement {
            return Err(format!(
                "a peer cannot supersede itself ({retired}) — if you meant to declare a \
                 re-key, the two ids differ"
            ));
        }
        if let Some(prior) = out.get(retired) {
            return Err(format!(
                "{retired} is declared superseded twice, by {prior} and by {replacement} — \
                 a peer has one successor"
            ));
        }
        out.insert(retired.to_string(), replacement.to_string());
    }
    Ok(out)
}

/// Parse `--gather=<peer_id>@<published-tree-dir>` occurrences.
///
/// Refused at the CLI boundary for `parse_supersessions`' reason, and the third
/// refusal is the one only the emitter can see:
///
/// ⭐ **Two gathers of one author would derive ONE key and the second would
/// silently replace the first.** §6.0.1's address is a function of the subject
/// alone (`FEED-R25`), and two `--gather=alice@…` specs have one subject however
/// different the two trees are — so `publish_mirror` writes both records at
/// `app/feed/mirrors/{coordinate}` and the last one wins, with nothing anywhere
/// saying a view was dropped. That is the property the derivation exists for
/// working exactly as intended, met by a caller asking for something the address
/// space cannot hold.
fn parse_gathers(raw: &[String]) -> Result<Vec<(String, PathBuf)>, String> {
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for entry in raw {
        let (author, dir) = entry
            .split_once('@')
            .ok_or_else(|| format!("expected PEER_ID@DIR, got {entry:?}"))?;
        let (author, dir) = (author.trim(), dir.trim());
        if author.is_empty() || dir.is_empty() {
            return Err(format!("both a peer id and a directory are required, got {entry:?}"));
        }
        if out.iter().any(|(a, _)| a == author) {
            return Err(format!(
                "{author} is gathered twice. A mirror's address is derived from its \
                 subject (§6.0.1), so both views would be written at one key and the \
                 second would silently replace the first — gather each author once."
            ));
        }
        out.push((author.to_string(), PathBuf::from(dir)));
    }
    Ok(out)
}

/// Run every `--gather` against the tree it names, **before the clean**.
///
/// Fails the whole publish on the first refusal rather than carrying a partial
/// set: a mirror is a *statement about what you gathered*, and one that silently
/// omitted an author a publisher asked for would be short in exactly the way
/// §6.1 rule 2 lets a reader assume is the source's fault.
fn resolve_gathers(
    gatherer: &str,
    specs: &[(String, PathBuf)],
) -> Result<Vec<crate::feed_mirror::MirrorPlan>, String> {
    let mut out = Vec::with_capacity(specs.len());
    for (author, dir) in specs {
        // Gathering yourself is not a mirror — it is the feed axis. The record
        // would be a claim about our own timeline bound under our own name, and
        // `--ingest-feed` is what publishes that, verifiably, from the source.
        if author == gatherer {
            return Err(format!(
                "{author} is this publisher — a mirror is a view of somebody ELSE's feed; \
                 --ingest-feed publishes your own"
            ));
        }
        let plan = crate::feed_read::block_on(crate::feed_gather::gather_timeline(
            dir,
            author,
            gatherer,
            crate::feed_gather::DEFAULT_GATHER_LIMIT,
        ))
        .map_err(|e| e.to_string())?;
        eprintln!(
            "publish --gather: {} entry(ies) from {author} ({} attributable) ← {}",
            plan.entry_count(),
            plan.attributable(),
            dir.display()
        );
        out.push(plan);
    }
    Ok(out)
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

/// **What a drop guard is scoped to: THIS OUTPUT DIRECTORY, never the estate.**
///
/// [`projected_site_ids`], [`projected_app_sets`] and [`projected_feed_posts`]
/// all answer *what is already in `{out}`* — so on a build-fresh publisher they
/// all read zero and `--plan` reports **"nothing would be removed"** on a
/// publish that replaces everything an origin is serving. `<coordination-tree>`
/// found this against their own pipeline, which does `rm -rf "$out"; mkdir -p
/// "$out"` before every run (their `B-5`).
///
/// ⭐ **The guards are not wrong; the sentence they produce was.** A comparison
/// against the out dir is exactly right for *"this run is about to clobber what
/// the last run put here"*, which is what they were built for. It is simply not
/// an answer to *"what am I about to replace at the origin"*, and a `0` reads as
/// the second. **`nothing would be removed` and `there is nothing here to
/// compare against` are different facts** (AP40) and only one of them is
/// reassuring.
///
/// Returns `false` when this peer has **no prior projection at all** in this
/// out dir, which is the case where every count below is uninformative.
fn prior_projection_present(out_dir: &Path, peer_id: &str, prefix: &str) -> bool {
    let root = paths::prefixed_root(out_dir, prefix);
    // Any of the three axis roots existing means the last run left something
    // here, so the counts are a real comparison. Checked as *existence*, not as
    // a non-empty read: a publisher that emptied a set deliberately still leaves
    // the directory, and that is a comparison we can make.
    root.join(SITE_URL_PREFIX).join(peer_id).exists() || root.join(peer_id).exists()
}

/// The site ids already projected under `{out}/{prefix}/sites/{peer}/`, sorted.
///
/// **Scoped to the output directory** — see [`prior_projection_present`] for
/// what that does and does not license you to say.
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

/// **Every top-level directory at this base, other than ours, that holds an
/// entity pointer anywhere beneath it** — i.e. somebody else's tree.
///
/// [`projected_peer_ids`] answers a narrower question (*who has a site
/// projection*) and was measured in `REFERENCE-PUBLISHING-PIPELINE` §0.2a to be
/// blind to a carried author, who has no `sites/{peer}/` and never will. It is
/// also blind to a co-hosted publisher who published only a feed, which is
/// latent today and stops being latent the moment `resolve_publish_source` can
/// read a real peer's tree.
///
/// ⚠ **This is a SUPPRESSION input and must never authorize a delete.**
/// Enumerating to decide what *not* to remove is safe in the worst case —
/// orphan blobs accumulate, which `--verify` already lists and §7's origin-wide
/// keep-set is the real answer to. Enumerating to decide what to remove is how a
/// publish destroyed a co-hosted publisher's signature blob (AP52/AP53), and the
/// predicate here cannot tell a carried author's segment from a sibling
/// publisher's — which is exactly why it is only ever allowed to say *"leave it
/// alone"*.
///
/// No name list: `sites/` holds `.html`, `content/` holds extensionless blobs
/// and `builds/` holds a retained shell, so *"contains a `.bin`"* separates a
/// peer's tree from every other thing at a publish root without anyone having to
/// keep a list of what those things are called.
fn foreign_trees(base: &Path, peer_id: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut found: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok().map(|n| (n, e.path())))
        .filter(|(name, path)| name != peer_id && holds_any_bin(path))
        .map(|(name, _)| name)
        .collect();
    found.sort();
    found
}

/// Does this directory hold an entity pointer anywhere beneath it? Early-exits
/// on the first one.
fn holds_any_bin(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            if holds_any_bin(&path) {
                return true;
            }
        } else if path.extension().map(|x| x == "bin").unwrap_or(false) {
            return true;
        }
    }
    false
}

/// How many gathered views are already projected under
/// `{out}/{prefix}/{peer}/app/feed/mirrors/`.
///
/// The fourth axis's counterpart of [`projected_feed_posts`], for the fourth
/// time for the same reason — and the report it feeds is the deciding argument
/// for a mirror being its own row rather than a second subgraph of the feed: a
/// publisher carrying posts and no gather would otherwise read *"0 post(s)
/// REMOVED"* on the run that deletes every view they hold.
fn projected_mirrors(out_dir: &Path, peer_id: &str, prefix: &str) -> usize {
    let root = paths::prefixed_root(out_dir, prefix)
        .join(peer_id)
        .join(crate::feed::mirror_prefix());
    std::fs::read_dir(&root)
        .map(|d| {
            d.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".bin")).count()
        })
        .unwrap_or(0)
}

/// How many feed posts are already projected under
/// `{out}/{prefix}/{peer}/app/feed/entries/`.
///
/// **Scoped to the output directory** — see [`prior_projection_present`].
///
/// The third axis's counterpart of [`projected_site_ids`] and
/// [`projected_app_sets`], and it exists for the third time for the same reason
/// — the clean removes `{base}/{peer}/` wholesale, so a publish invoked without
/// `--ingest-feed` **deletes the whole archive**, which for a feed is the one
/// thing that cannot be re-derived from anywhere else in the output tree.
///
/// Counted, not named: an entry's key is the hex of its own content hash, so
/// listing them would print 32 hashes where *"14 post(s)"* is the fact an
/// operator can act on.
fn projected_feed_posts(out_dir: &Path, peer_id: &str, prefix: &str) -> usize {
    let root = paths::prefixed_root(out_dir, prefix)
        .join(peer_id)
        .join(crate::feed::entry_prefix());
    std::fs::read_dir(&root)
        .map(|d| {
            d.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".bin")).count()
        })
        .unwrap_or(0)
}

/// The app sets already projected under `{out}/{prefix}/{peer}/apps/`, each
/// with the number of bundle pointers under it.
///
/// **Scoped to the output directory** — see [`prior_projection_present`].
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

// **What `--verify` last counted as an orphan** — a test-only record, because
// the orphan report is `eprintln!` and an exit code cannot carry it.
//
// It exists for a reason a comment alone cannot hold: **exit 0 is not the whole
// report.** A mirror publish verified clean while describing every carried
// `system/signature` as *"dead weight the sync will keep re-uploading"* — i.e.
// `FEED-R2`'s whole authorship instrument — and no assertion about the exit
// code could ever have seen it.
//
// A thread-local rather than a return value: `run_verify` prints throughout and
// returning a report would be a refactor of a 300-line function for one
// assertion. **`#[cfg(test)]` on both halves**, so nothing in a shipped binary
// carries it; a caller that genuinely wants the count should be given a real
// return type instead of reaching for this.
#[cfg(test)]
thread_local! {
    static LAST_ORPHAN_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
}

#[cfg(test)]
fn record_orphan_count(n: usize) {
    LAST_ORPHAN_COUNT.with(|c| c.set(n));
}

#[cfg(not(test))]
fn record_orphan_count(_n: usize) {}

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
    record_orphan_count(orphans.len());

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
    // Entries this walk reached through a mirror record, and whose author is
    // therefore NOT the publisher. Populated by the `app/feed/mirror` arm and
    // read by the `app/feed/entry` arm immediately after — the queue is LIFO, so
    // a record's rows pop before anything else it enqueued.
    let mut carried_author: std::collections::BTreeMap<String, String> = Default::default();
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
        // **A site asset DECLARES its blob, exactly as a trie node declares
        // its children — so it gets the same structural treatment.**
        //
        // Since content-site §4's pointer `[MUST]`, an asset above EMBED §3's
        // 16 KiB ceiling carries `payload: {tag: "pointer", hash}` and its
        // bytes live in the content store as a `system/content/blob` plus
        // chunks. The window scan above *does* find them — a 33-byte hash is a
        // 33-byte hash wherever it sits — which is why this tree verified
        // green the day the pointer arm landed. **And that green was the F8
        // failure again, measured rather than reasoned:** delete the chunk and
        // the closure count drops 11 → 10, `missing` stays 0, and verify
        // reports *"every pointer resolves and every body hashes to its
        // address"* about a site whose figure nobody can resolve. Presence
        // filtering cannot see absence; only a declaration can.
        //
        // The consequence is the one this whole verb exists to prevent — a
        // published tree that is broken and says it is clean — and it is worse
        // for an asset than for an interior node, because the page still
        // renders and only the image is gone.
        if entity.entity_type == crate::content_site::format::SITE_ASSET_TYPE {
            let asset = crate::content_site::format::SiteAsset::from_entity(&entity);
            if let Some(blob) = asset.pointer() {
                if fetcher.content(&blob).is_ok() {
                    queue.push(blob);
                } else if missing_hashes.insert(blob.to_hex()) {
                    eprintln!(
                        "publish --verify: BROKEN asset closure — blob {} is DECLARED by site \
                         asset {} but not projected. The page renders and the image does not; a \
                         consumer has no other way to reach those bytes.",
                        blob.to_hex(),
                        h.to_hex()
                    );
                }
            }
            continue;
        }

        // **An app's asset-bundle index declares one blob per file**, and gets
        // the arm in the commit that introduces it — F8's rule, which the site
        // asset and the feed entry both paid for after the fact. A missing file
        // here is the worst of the three: the app launches, its index resolves,
        // and the first key it asks for comes back `unavailable` from inside a
        // running program, nowhere near the publish that dropped it.
        if entity.entity_type == crate::apps::assets::INDEX_TYPE {
            match crate::apps::assets::AssetIndex::from_entity(&entity) {
                Ok(index) => {
                    for blob in index.blobs() {
                        if fetcher.content(&blob).is_ok() {
                            queue.push(blob);
                        } else if missing_hashes.insert(blob.to_hex()) {
                            eprintln!(
                                "publish --verify: BROKEN app asset closure — blob {} is DECLARED \
                                 by asset index {} but not projected. The app will ask for it \
                                 and be told it is unavailable.",
                                blob.to_hex(),
                                h.to_hex()
                            );
                        }
                    }
                }
                Err(why) => eprintln!(
                    "publish --verify: asset index {} did not decode ({why}) — its closure could \
                     not be checked",
                    h.to_hex()
                ),
            }
            continue;
        }

        // **An `app/feed/entry` declares its body's blob the same way, and this
        // arm landed with the third publish axis rather than after it.** F8's
        // lesson is that a heuristic scan filters on presence and therefore
        // cannot see absence, so every DECLARING type owes an arm here — and a
        // feed entry over EMBED §3's 16 KiB ceiling carries exactly the same
        // `payload: {tag: "pointer", hash}` a site asset does.
        //
        // Worse than the asset case, by the amount that a post is more than a
        // figure: an asset's absence drops an image out of a page that still
        // reads, and an entry's absence is **the post itself**, rendering as an
        // empty body with nothing anywhere saying why.
        // ⭐ **`APP-CONVENTION-FEED` §6's mirror record — F8's rule reaching the
        // fourth declaring type, and the one `REFERENCE-PUBLISHING-PIPELINE`
        // §0.2a said an arm could not fix.**
        //
        // That paragraph is **corrected by this arm, and the correction is worth
        // the space.** It reasoned: the sweep is rooted at `{base}/{peer_id}/`,
        // a mirror's carried bodies are bound under each *author's* segment, so
        // the sweep never visits them and the question is *which peers' subtrees
        // a verify covers* — a scope redesign rather than an arm. Two facts,
        // both measured here rather than reasoned, make it an arm after all:
        //
        // 1. **The blobs are not foreign at all.** `write_entity` puts every
        //    body into the SHARED `content/{aa}/{bb}/{hex}` store whoever it
        //    belongs to; only the `.bin` *pointer* is peer-scoped. So the
        //    closure fetcher already reaches a carried entry's bytes, by hash,
        //    with no widening of anything.
        // 2. **A mirror record DECLARES what it carries** — `entries` is
        //    `FEED-R28`-pinned, so each row is a `(peer, hash)` coordinate. That
        //    is the same structural handle a trie node's children and a site
        //    asset's blob give, and it is what *"a heuristic scan filters on
        //    presence, so it cannot see absence"* has needed every time.
        //
        // ⇒ the general rule holds without an exception: **every declaring type
        // owes an arm, in the commit that introduces it.** What is different
        // here is only that the declaration names a *pair*, so the arm checks
        // two things — the bytes, and the foreign key a consumer resolves them
        // by. Either alone passes a tree that does not serve: bytes with no
        // pointer are unreachable by `read_mirror`'s `entry_key` fetch, and a
        // pointer with no bytes is the two-hop's second leg missing.
        // **The HEAD declares a page RANGE, so it owes an arm of its own.**
        // §6.0a moved `entries` off the head onto key-addressed pages, and the
        // absence that move created is a head naming a page nothing serves —
        // which no scan can see, for the reason this whole section exists.
        if entity.entity_type == crate::feed::FEED_MIRROR_TYPE {
            match crate::feed::FeedMirror::from_entity(&entity, peer_id) {
                Ok(record) => {
                    let subject =
                        crate::feed::MirrorSubject::from_reference(&record.subject);
                    for page in record.oldest..=record.current {
                        let key = subject.page_key(page);
                        let pointer = base.join(peer_id).join(format!("{key}.bin"));
                        if !pointer.exists() && missing_hashes.insert(key.clone()) {
                            eprintln!(
                                "publish --verify: BROKEN mirror — head {} names pages {}..={} \
                                 and page {page} is not projected at {key}. A reader reads DOWN \
                                 from `current`, so the view stops at a hole it cannot explain.",
                                h.to_hex(),
                                record.oldest,
                                record.current
                            );
                        }
                    }
                }
                Err(why) => eprintln!(
                    "publish --verify: mirror head {} did not decode ({why}) — the pages it \
                     names could not be checked",
                    h.to_hex()
                ),
            }
            continue;
        }

        if entity.entity_type == crate::feed::FEED_MIRROR_PAGE_TYPE {
            // The page number is recoverable from the key we read it at, and
            // `MirrorPage::from_entity` checks the body agrees — but this sweep
            // walks by hash and does not carry the key, so the page is decoded
            // against its own declared number. The key/body agreement is gated
            // where a reader learns the key: `read_mirror`.
            match crate::feed::MirrorPage::from_entity(&entity, None) {
                Ok(record) => {
                    for reference in &record.entries {
                        let crate::entity_ref::EntityRef::Pinned { peer: author, hash, .. } =
                            reference
                        else {
                            // `from_entity` already refuses a non-pin in
                            // `entries` (`FEED-R28`), so this is unreachable —
                            // and `continue` rather than a report, because
                            // inventing a message for a state the decoder
                            // excludes is a sentence nobody can ever act on.
                            continue;
                        };
                        // Leg 1 — the bytes, in the shared store.
                        if fetcher.content(hash).is_ok() {
                            queue.push(*hash);
                            // Whoever pops this next needs to know it is not
                            // ours, or `FEED-R1` refuses it and the entry arm
                            // reports a decode failure about a healthy entry.
                            carried_author.insert(hash.to_hex(), author.clone());
                        } else if missing_hashes.insert(hash.to_hex()) {
                            eprintln!(
                                "publish --verify: BROKEN mirror closure — entry {} is DECLARED \
                                 by mirror {} and its body is not projected. The gathered view \
                                 names a post nobody can fetch.",
                                hash.to_hex(),
                                h.to_hex()
                            );
                        }
                        // Leg 2 — the key a consumer resolves it BY, under the
                        // author, which is the half outside this sweep's root.
                        let pointer = base
                            .join(author)
                            .join(format!("{}.bin", crate::feed::entry_key(hash)));
                        if !pointer.exists()
                            && missing_hashes.insert(format!("{}@{author}", hash.to_hex()))
                        {
                            eprintln!(
                                "publish --verify: BROKEN mirror closure — entry {} is DECLARED \
                                 by mirror {} and has no pointer at {}/{}. A consumer reaches a \
                                 carried entry by KEY under its author, so these bytes are \
                                 unreachable however present they are.",
                                hash.to_hex(),
                                h.to_hex(),
                                author,
                                crate::feed::entry_key(hash)
                            );
                        }
                        // **The author's detached signature, if they published
                        // one.** Followed rather than declared, and the two are
                        // different for a reason: `FEED-R4` makes a missing
                        // signature an ordinary fact, so the record says nothing
                        // about whether one exists and there is nothing for
                        // structure to check. What *is* wrong is leaving it out
                        // of the walk — measured, before this existed: every
                        // carried signature was reported as an **orphan**, i.e.
                        // `--verify` calling `FEED-R2`'s entire authorship
                        // instrument *"dead weight the sync will keep
                        // re-uploading"*. Enqueuing it both silences that and
                        // puts its bytes through the same integrity check
                        // everything else here gets.
                        let sig_pointer = base
                            .join(author)
                            .join(format!("{}.bin", crate::feed::signature_key(author, hash)));
                        if let Ok(raw) = std::fs::read(&sig_pointer) {
                            if let Ok(sig_hash) = super::http_poll::crack_pointer(&raw) {
                                queue.push(sig_hash);
                            }
                        }
                    }
                }
                // Reported, never skipped — same rule as an undecodable feed
                // entry one arm down.
                Err(why) => eprintln!(
                    "publish --verify: mirror record {} did not decode ({why}) — the entries \
                     it carries could not be checked",
                    h.to_hex()
                ),
            }
            continue;
        }

        if entity.entity_type == crate::feed::FEED_ENTRY_TYPE {
            // **Whose entry is this?** Ours by default, and a carried one
            // belongs to the author the mirror record named. `FEED-R1` refuses
            // an entry whose `author` is not the namespace it was read in, so
            // passing `peer_id` for a gathered entry would report a decode
            // failure about an entry that is exactly right.
            let author = carried_author.get(&h.to_hex()).map(String::as_str).unwrap_or(peer_id);
            match crate::feed::FeedEntry::from_entity(&entity, author) {
                Ok(feed_entry) => {
                    for blob in crate::feed_tree::body_blob_hashes(&feed_entry.body) {
                        if fetcher.content(&blob).is_ok() {
                            queue.push(blob);
                        } else if missing_hashes.insert(blob.to_hex()) {
                            eprintln!(
                                "publish --verify: BROKEN feed closure — blob {} is DECLARED by \
                                 feed entry {} but not projected. The post appears in the index \
                                 and its body is empty.",
                                blob.to_hex(),
                                h.to_hex()
                            );
                        }
                    }
                }
                // An entry we cannot decode is reported, never skipped: a
                // publisher's own tree carrying an undecodable entry under a
                // signed root is a fault in the publish, and the whole point of
                // this verb is that a broken tree does not say it is clean.
                Err(why) => eprintln!(
                    "publish --verify: feed entry {} did not decode ({why:?}) — its body's \
                     closure could not be checked",
                    h.to_hex()
                ),
            }
            continue;
        }

        // A `system/content/blob` declares its chunks. Same rule one level
        // down: without this, a projected blob whose chunks were dropped is a
        // pointer to a chunk list nobody can follow, and the window scan
        // reports it as clean.
        if entity.entity_type == entity_types::TYPE_CONTENT_BLOB {
            match crate::content_site::asset_store::chunk_hashes_of(&entity) {
                Ok(chunks) => {
                    for c in chunks {
                        if fetcher.content(&c).is_ok() {
                            queue.push(c);
                        } else if missing_hashes.insert(c.to_hex()) {
                            eprintln!(
                                "publish --verify: BROKEN asset closure — chunk {} is DECLARED \
                                 by blob {} but not projected. The blob resolves and reassembly \
                                 stops here.",
                                c.to_hex(),
                                h.to_hex()
                            );
                        }
                    }
                }
                Err(why) => {
                    // Counted as missing, not merely logged: a blob we cannot
                    // decode is a chunk list we cannot follow, which is the
                    // same consumer outcome as chunks that are not there.
                    if missing_hashes.insert(h.to_hex()) {
                        eprintln!(
                            "publish --verify: BROKEN asset closure — blob {} does not decode: \
                             {why}",
                            h.to_hex()
                        );
                    }
                }
            }
            continue;
        }

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
#[allow(clippy::too_many_arguments)] // one parameter per publish axis, plus the modes
fn run_plan(
    out_dir: &Path,
    peer_id: &str,
    sites: &[read::OwnedSite],
    prefix: &str,
    bare_root: bool,
    app_sets: &crate::apps::ingest::IngestedSets,
    feed: Option<&crate::feed_tree::OwnedFeed>,
    mirrors: &[crate::feed_mirror::MirrorPlan],
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

    // ⭐ **Say what the counts are scoped to BEFORE printing any of them.**
    // Every "present" below is read from this output directory, so a pipeline
    // that recreates `{out}` on each run (`rm -rf "$out"; mkdir -p "$out"` —
    // meta's, and the shape that earned this) makes all three axes read zero and
    // the plan report "0 REMOVED" for a publish that replaces an entire estate.
    // `nothing would be removed` and `there is nothing here to compare against`
    // are different facts and only one of them is reassuring, so the empty case
    // gets its own sentence rather than a reassuring zero (AP40).
    if !prior_projection_present(out_dir, peer_id, prefix) {
        eprintln!(
            "publish --plan: this output directory holds NO prior projection for this peer, so \
             every count below is 0 — that is a fact about {}, NOT about what an origin is \
             currently serving. If your pipeline recreates the output directory on each run, \
             --plan cannot tell you what a deploy would replace; compare against the origin.",
            out_dir.display()
        );
    }
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

    // The feed axis. Same rule, third time: the clean covers
    // `{peer}/app/feed/**`, and a publish carrying fewer posts than the tree
    // already holds is an archive being truncated. `run_plan`'s own doc comment
    // demands this term — *"if a future emitter adds a third subgraph under the
    // peer prefix, it owes a term here in the same commit"* — and this is that
    // commit.
    let feed_removed: usize = if emit_bin {
        let present = projected_feed_posts(out_dir, peer_id, prefix);
        let incoming = feed.map_or(0, |f| f.entries.len());
        if present > 0 || incoming > 0 {
            eprintln!(
                "publish --plan: {present} post(s) present, {incoming} incoming — {} REMOVED",
                present.saturating_sub(incoming)
            );
        }
        present.saturating_sub(incoming)
    } else {
        0
    };

    // The mirror axis. Fourth time, same rule — and this term is the reason a
    // mirror is its own row: folded into the feed's, a publisher republishing
    // posts without re-gathering would be told *"0 post(s) REMOVED"* on the run
    // that deletes every gathered view they hold.
    //
    // ⚠ **The units are VIEWS, not the entries inside them.** The carried
    // bodies are not removed by the clean at all (they are under their authors,
    // which `{base}/{peer}/` is not), so counting entries here would name a loss
    // that does not happen and hide the one that does: it is the *record* — the
    // only thing that says a view exists and how to find it — that goes.
    let mirrors_removed: usize = if emit_bin {
        let present = projected_mirrors(out_dir, peer_id, prefix);
        let incoming = mirrors.len();
        if present > 0 || incoming > 0 {
            eprintln!(
                "publish --plan: {present} gathered view(s) present, {incoming} incoming — \
                 {} REMOVED",
                present.saturating_sub(incoming)
            );
        }
        present.saturating_sub(incoming)
    } else {
        0
    };

    if removed.is_empty() && app_removed.is_empty() && feed_removed == 0 && mirrors_removed == 0
    {
        // ⭐ **The verdict, not just the counts.** A bare *"nothing would be
        // removed"* is the sentence `<coordination-tree>`'s `B-5` is about: with
        // no prior projection to compare against it is true of this directory
        // and says nothing about the deploy, which is the question the operator
        // is actually asking. Two outcomes, two sentences (AP40) — and the exit
        // code stays `SUCCESS` for both, because *we cannot tell* is not a
        // refusal and a pipeline that gates on this must not start failing.
        if prior_projection_present(out_dir, peer_id, prefix) {
            eprintln!("publish --plan: nothing would be removed.");
        } else {
            eprintln!(
                "publish --plan: nothing would be removed FROM THIS DIRECTORY — which held no \
                 prior projection for this peer, so that is not a statement about what a deploy \
                 would replace."
            );
        }
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
        if feed_removed > 0 {
            eprintln!(
                "publish --plan: {feed_removed} feed post(s) would be REMOVED. The clean covers \
                 {peer_id}/app/feed/** too — re-run with --ingest-feed=<dir> to carry them."
            );
        }
        if mirrors_removed > 0 {
            eprintln!(
                "publish --plan: {mirrors_removed} gathered view(s) would be REMOVED. The \
                 clean covers {peer_id}/{} too — re-run with --gather=<peer_id>@<dir> to \
                 carry them. (The entries themselves stay: they are bound under their own \
                 authors, which this clean does not reach — what goes is the record that \
                 says the view exists.)",
                crate::feed::mirror_prefix()
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
    feed: Option<&crate::feed_tree::OwnedFeed>,
    mirrors: &[crate::feed_mirror::MirrorPlan],
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
        //
        // **And `siblings` is not the whole question** — it reads
        // `{base}/sites/`, so it cannot see a peer whose tree is here without a
        // site projection. `APP-CONVENTION-FEED` §6's carried bodies are exactly
        // that: bound at `{base}/{author}/app/feed/entries/…` with their blobs
        // in this shared store, and with no `sites/{author}/` now or ever. Left
        // to `siblings` alone, a republish that does not re-gather would delete
        // the blobs and leave the pointers — **remove-then-un-name, which is
        // this repo's own rule run backwards**, and a tree that no longer
        // resolves what it still names.
        //
        // So the question the content clean asks is the broader one, and it is
        // asked in the direction that is safe to be wrong in: *is anybody else's
        // tree here at all?* A false positive costs orphans; a false negative
        // costs somebody else's bytes.
        let foreign = foreign_trees(&base, peer_id);
        if siblings.is_empty() && foreign.is_empty() {
            clean.push(base.join("content"));
        } else if siblings.is_empty() {
            println!(
                "  foreign tree(s) at this base ({}) — leaving the shared content store \
                 alone, because their bodies are in it and nothing here can tell a \
                 gathered author's segment from a co-hosted publisher's",
                foreign.join(", ")
            );
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
        // **Every axis, through the one list** (`crate::publish_axes`). This
        // used to be `emit_owned_sites(sites)` followed by an inline
        // `for set in app_sets` loop — the hardcoded two-convention enumeration
        // `AGENTS.md` records as the defect, where a third L5 convention cost an
        // edit in five places and a fourth would again. Each row reads its own
        // subgraph out of the tree and records into **this** projector, which is
        // what `a_second_axis_signed_by_its_own_projector_un_names_the_first`
        // measured is the only composable shape: one projection, N axes.
        //
        // An axis with nothing to publish is not an error — most publishers use
        // one convention — so a `None` report line is silence, not a skip that
        // needs explaining.
        for axis in crate::publish_axes::axes(peer_id, sites, app_sets, feed, mirrors) {
            match axis.project(out_dir, peer_id, prefix, &mut root) {
                Ok(Some(line)) => println!("  {line}"),
                Ok(None) => {}
                Err(e) => {
                    eprintln!("publish: {} (.bin) export failed: {e}", axis.name());
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
/// peer-id (content lives under `sites/{peer}/…`) **and the whole `home_site`
/// key is absent when this publish carries no sites**; `origins[peer]` is the
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
    //
    // **`None` when this publish carries no sites, and the key is then ABSENT
    // from the document rather than present-and-empty.** A `home_site` naming
    // the empty site id is dropped by `DeploymentConfig::parse` anyway, so the
    // two arms agree at the consumer — what differs is that one of them is a
    // field on the wire asserting a home that does not exist, for every reader
    // and every diff. The refusals in `run` mean a site-free document is always
    // `surface=chrome`, or a `window` with its own address.
    let site_id: Option<String> = match &spec.site {
        Some(id) => Some(id.clone()),
        None => pick_bare_site(sites, None).map(|s| s.site_id.clone()),
    };

    let mut origins = serde_json::Map::new();
    origins.insert(peer_id.to_string(), serde_json::Value::String(spec.origin.clone()));

    let mut doc = serde_json::json!({
        "surface": spec.surface,
        "origins": origins,
    });
    let obj = doc.as_object_mut().expect("json! built an object");
    if let Some(site_id) = &site_id {
        obj.insert(
            "home_site".into(),
            serde_json::json!({ "peer": peer_id, "site": site_id, "loc": "" }),
        );
    }
    // The §7.4 preload: the registry this deployment seeds. Emitted only when
    // asked for — a build that ships no pin is the fail-closed default, and an
    // empty object here would read as "a pin that resolves nothing".
    if let Some(pin) = &spec.registry_pin {
        obj.insert(
            "name_registry_pin".into(),
            serde_json::json!({ "peer_id": pin.peer_id, "origin": pin.origin }),
        );
    }
    // Window surface carries its window type, and its address if one was named.
    if spec.surface == "window" {
        obj.insert("window_type".into(), serde_json::Value::String(spec.window_type.clone()));
        // Absent rather than empty when nothing was declared: *"named no
        // target"* and *"named the empty target"* stay apart on the wire, the
        // same choice `superseded` makes about an empty declared set.
        if !spec.window_target.is_empty() {
            obj.insert(
                "window_target".into(),
                serde_json::Value::String(spec.window_target.clone()),
            );
        }
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

    // The successions this document will carry: whatever it already declared,
    // plus whatever this publish declares. **Additive on every arm**, including
    // the home-publish arms that rewrite the domain's own fields — a re-key is
    // a historical fact about a peer, and a home publish has no more business
    // dropping a tenant's succession than dropping their `origins` entry. The
    // only way one leaves is an operator editing the document.
    let mut superseded: serde_json::Map<String, serde_json::Value> = existing
        .as_ref()
        .and_then(|o| o.get("superseded"))
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let mut declared_now: Vec<(String, String)> = Vec::new();
    for (retired, replacement) in &spec.superseded {
        let prior = superseded.get(retired).and_then(|v| v.as_str()).map(str::to_string);
        if prior.as_deref() != Some(replacement.as_str()) {
            declared_now.push((retired.clone(), replacement.clone()));
        }
        superseded.insert(retired.clone(), serde_json::Value::String(replacement.clone()));
    }
    // Applied to `merged` below rather than to `obj` here, because `obj` is the
    // FRESH document and the `Defers` arm never uses it — a secondary publish
    // returns the prior document, so an insert here would be silently dropped
    // on exactly the arm a re-keying tenant publishes under.
    let mut merged = match (&claim, existing) {
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

    if superseded.is_empty() {
        merged.remove("superseded");
    } else {
        merged.insert("superseded".into(), serde_json::Value::Object(superseded));
    }

    // D13: say what was done to a document this publish did not author. Silence
    // here is how the clobber went unnoticed for as long as it did.
    for (retired, replacement) in &declared_now {
        println!(
            "  deployment config: DECLARED {retired} superseded by {replacement} — returning \
             profiles will adopt this and rewrite their stored references to {retired}"
        );
    }
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

/// Everything a publish reads out of the tree — one member per axis in
/// [`crate::publish_axes::axes`].
///
/// A struct rather than a tuple since the third axis landed: a fourth would
/// make it a five-tuple, and the members are not interchangeable.
pub(crate) struct PublishSource {
    pub peer_id: String,
    pub sites: Vec<read::OwnedSite>,
    pub app_sets: crate::apps::ingest::IngestedSets,
    /// `None` when this peer has authored no posts — which is *not* an empty
    /// feed. See [`crate::feed_tree`]'s module doc: publishing an empty head on
    /// behalf of every site publisher who never used the convention would be
    /// making a claim for them.
    pub feed: Option<crate::feed_tree::OwnedFeed>,
}

/// Where a publish's **sites** come from — the first content axis, and the only
/// one that ever had a default.
///
/// ## ⭐ THE AXIS THAT INVENTED CONTENT, AND THE ARGUMENT AGAINST IT IS OUR OWN
///
/// [`resolve_publish_source`]'s own comment on `--ingest-feed` reads: *"a publish
/// that invented an empty feed would claim every site publisher has one."* That
/// is exactly right, and until 2026-09-16 the sites axis did the thing it
/// refuses — absent `--ingest` it seeded `demo` + `entity-info` and claimed
/// every publisher had them. **The asymmetry was not a decision anybody made:**
/// when the seeding was written, every publish had sites.
///
/// **The consequence is larger than two unwanted sites, because the clean is
/// wholesale.** `run_projection` removes `{base}/{peer}` and rebuilds it, so a
/// publish that forgot `--ingest` did not merely *add* the demo set — it
/// replaced a domain's real sites with it, under that domain's own identity, and
/// exited `0`. **Silence is refused now** ([`parse_site_source`]): a publish
/// states its site arm or does not run.
///
/// The three arms are 1:1 with `<coordination-tree>`'s `estate.conf` axis
/// vocabulary (`papers` / `builtin` / `none`), which is where the ask came from
/// (`TRACKER-<coordination-tree>.md` `B-4`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SiteSource {
    /// `--ingest=<dir>` — a content-team `render/` emit, disk → tree.
    Ingest(PathBuf),
    /// `--demo-sites` — the bundled `demo` + `entity-info` set, **on purpose**.
    /// This is what silence used to mean; it is a flag now so that a publish
    /// carrying it says so in its own command line.
    Demo,
    /// `--no-sites` — this publish carries no sites, deliberately. A feed-only
    /// or apps-only domain; the other two axes decide whether it is empty.
    None,
}

/// Parse the site arm out of the command line. **Exactly one of three, and
/// silence is an error** — pure so the whole matrix is gated by `make test`
/// rather than only through a process invocation.
///
/// **Refusing silence is the point of the change, not a side effect of it.**
/// *"You forgot `--ingest`"* and *"this domain has no sites"* produced the same
/// command line, so no guard anywhere — ours or the caller's — could tell them
/// apart. They are different command lines now.
fn parse_site_source(args: &[String]) -> Result<SiteSource, String> {
    let ingest: Option<PathBuf> =
        args.iter().find_map(|a| a.strip_prefix("--ingest=").map(PathBuf::from));
    let demo = args.iter().any(|a| a == "--demo-sites");
    let none = args.iter().any(|a| a == "--no-sites");
    match (ingest, demo, none) {
        (Some(dir), false, false) => Ok(SiteSource::Ingest(dir)),
        (None, true, false) => Ok(SiteSource::Demo),
        (None, false, true) => Ok(SiteSource::None),
        (None, false, false) => Err(
            "this publish does not say where its SITES come from. Pass exactly one of:\n  \
             --ingest=<dir>   publish a render/ emit (the content-team pipeline)\n  \
             --demo-sites     publish the bundled demo + entity-info set, on purpose\n  \
             --no-sites       publish no sites — a feed-only or apps-only domain\n\
             Silence used to mean --demo-sites. It is refused because the clean is \
             wholesale: a publish that meant --ingest and omitted it replaced a domain's \
             real sites with the demo set, under that domain's own identity, and exited 0."
                .to_string(),
        ),
        (i, d, n) => {
            let mut passed: Vec<&str> = Vec::new();
            if i.is_some() {
                passed.push("--ingest");
            }
            if d {
                passed.push("--demo-sites");
            }
            if n {
                passed.push("--no-sites");
            }
            Err(format!(
                "the site arm is exactly one of --ingest=<dir> / --demo-sites / --no-sites, \
                 and this command passes {}. They are three different publishes — pick one.",
                passed.join(" and ")
            ))
        }
    }
}

/// Resolve which peer to publish and read every axis off its tree. **The seam**
/// (see the module docs): today it builds a Direct peer under the supplied
/// **publisher `keypair`** (durable by default; `--identity-seed` / `--demo-identity`
/// override) and seeds the bundled demo site set, then reads it back off the
/// tree — so publish is a faithful end-to-end exercise of the [A] reader on
/// demo data, under a *stable* peer-id.
/// Replace this body with "open a persisted peer dir → read its real sites"
/// when the durable native peer-load path lands; nothing downstream changes,
/// and the publisher identity becomes the loaded peer's own (still stable).
///
/// **The pipeline is `ingest(disk) → tree → read_all(tree) → project`**, and
/// that is the whole publishing model: an input is translated into the tree, and
/// the tree is what gets projected. Every `--ingest*` flag is one front door
/// onto the same tree, never a parallel source.
///
/// ## ⭐ THE PEER IS FRESH, AND THAT IS WHY `A-36`'s PEER-ROOT SCOPE IS FREE HERE
/// AND EXPENSIVE ELSEWHERE — read this before building the durable-load path above
///
/// Arch ruled (2026-09-15) that a publish MUST commit over a scope containing
/// both an entry and its `FEED-R2` signature, and named the peer root as the
/// scope that does. `entity-workbench-go` built it and measured the cost on
/// their architecture: **4 committed keys → 386, 7 emitted entities → 400 across
/// 379 paths**, with an operator's folder path, another peer's LAN address and
/// an unshared document's body landing in the upload directory. Filed as their
/// `A-38`. On the static road there is no grant, so **the published prefix *is*
/// the disclosure control**, and the ruled fix moves exactly that control.
///
/// **We measured the same publish shape and it is 19 committed keys, 20 emitted
/// paths, 23 content blobs** — one site of 7 pages plus a 4-post feed, every
/// key something an axis projected. Not luck, and not a better ruling:
/// [`Peers::new_direct_with_keypair`] builds a **fresh in-memory peer** whose
/// only writes are the `ingest*` calls below, and `RootProjector::record`
/// inserts only what an emitter hands it. ⇒ ***a publish scope here is a binding
/// set we CONSTRUCTED, not a subtree we POINTED AT*** — the same sentence
/// ("publish over the peer root") is a projection policy in one architecture and
/// a directory traversal in the other.
///
/// ⛔ **So `A-38` is a property of the verb this doc comment tells you to build,
/// not of their implementation.** The line above — *"replace this body with 'open
/// a persisted peer dir → read its real sites'"* — is `F3`, and on the day it
/// lands we inherit their finding exactly: a durable tree holds keys, app state
/// and other peers' cached bytes, and projecting its root would publish them.
/// `RootProjector`'s own doc already refuses that (*"the root commits to the
/// bytes we projected, not to the tree we read from… never over the source
/// peer's whole tree, which holds keys and app state a publish must not commit
/// to"*), and what is missing is the thing `AGENTS.md` has named for weeks: **a
/// publication policy — which subgraphs are public.** A grep for `publishable`
/// still finds a comment. **`F3` is not "read a real store instead of a fresh
/// one"; it is that plus the policy, and their measurement is the price tag.**
fn resolve_publish_source(
    keypair: entity_crypto::Keypair,
    sites: &SiteSource,
    ingest_apps: Option<&Path>,
    ingest_feed: Option<&Path>,
) -> Result<PublishSource, String> {
    let peers = Peers::new_direct_with_keypair(keypair);
    let peer_id = peers.primary_peer_id().to_string();
    match sites {
        // Real content: ingest a `render/` emit (disk→tree), then read it
        // back through the same path the demo source uses.
        SiteSource::Ingest(dir) => {
            let ids = super::ingest::ingest_path(&peers, &peer_id, dir)?;
            eprintln!(
                "publish --ingest: ingested {} site(s) from {} — {:?}",
                ids.len(),
                dir.display(),
                ids
            );
        }
        // The bundled demo site set (the SSG / demo generator), **asked for**.
        SiteSource::Demo => {
            seed_demo_site_set(&peers, &peer_id);
            eprintln!("publish --demo-sites: seeded the bundled demo site set");
        }
        // The arm that did not exist. Nothing is seeded and nothing is read;
        // the other two axes decide whether this publish carries anything.
        SiteSource::None => eprintln!("publish --no-sites: this publish carries no sites"),
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
    // The third axis. Opt-in like the apps one and for the same reason: without
    // a source there is nothing to read, and a publish that invented an empty
    // feed would claim every site publisher has one.
    if let Some(dir) = ingest_feed {
        let got = crate::feed_ingest::ingest_path(&peers, &peer_id, dir)?;
        eprintln!(
            "publish --ingest-feed: ingested {} post(s) from {}",
            got.posts,
            dir.display()
        );
        // An authored key the entry could not carry. Printed AFTER the count so
        // the happy line is not buried, and printed at all because until
        // 2026-09-16 this loss was silent here while `entity-core-papers`'
        // authoring gate warned about it — see `feed_ingest`'s `params` loop.
        for note in &got.notes {
            eprintln!("publish --ingest-feed: dropped {note}");
        }
    }
    let sites = read::read_all_sites(&peers, &peer_id);
    let app_sets = crate::apps::read::read_all_app_sets(&peers, &peer_id);
    let feed = crate::feed_tree::read_owned_feed(&peers, &peer_id);
    Ok(PublishSource { peer_id, sites, app_sets, feed })
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
    // The sites index lands at `{prefix}/sites/` (just `sites/` at the root) —
    // and a publish carrying no sites does not emit one, so pointing a reader
    // at it would be a 404 in the closing line of a successful publish.
    if sites.is_empty() {
        println!(
            "  view: this publish carries no sites, so there is no /{SITE_URL_PREFIX}/ page \
             — its content is read by a consumer, not browsed as HTML"
        );
        return;
    }
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
    use crate::content_site::http_poll;

    /// The publish source resolver seeds + reads both demo sites whole off
    /// the tree (the [A] read), with the second site's cross-link intact.
    #[test]
    fn publish_source_reads_the_seeded_demo_set() {
        let demo_kp = || entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED);
        let PublishSource { peer_id, sites, app_sets, .. } =
            resolve_publish_source(demo_kp(), &SiteSource::Demo, None, None).unwrap();
        assert!(!peer_id.is_empty());

        // The publisher identity is STABLE across runs for a given seed — the
        // whole point (no more shifting peer-ids / broken permalinks).
        let PublishSource { peer_id: peer_id_again, .. } =
            resolve_publish_source(demo_kp(), &SiteSource::Demo, None, None).unwrap();
        assert_eq!(peer_id, peer_id_again, "publish peer-id must be reproducible");

        // A DIFFERENT system seed → a different (but still reproducible) peer-id,
        // so each deployment can publish under its own identity (`--identity-seed`).
        let other_kp = || entity_crypto::Keypair::from_seed(*b"different-publisher-seed-here!!!");
        let PublishSource { peer_id: other_pid, .. } =
            resolve_publish_source(other_kp(), &SiteSource::Demo, None, None).unwrap();
        assert_ne!(peer_id, other_pid, "a custom seed must yield its own peer-id");
        let PublishSource { peer_id: other_pid_again, .. } =
            resolve_publish_source(other_kp(), &SiteSource::Demo, None, None).unwrap();
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
            content: Vec::new(),
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
            content: Vec::new(),
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
            let mut args = vec![
                "publish".to_string(),
                "--demo-sites".into(),
                out.clone(),
                "--demo-identity".into(),
            ];
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
        let PublishSource { peer_id: pid, sites: _sites, app_sets: _apps, .. } = 
            resolve_publish_source(demo_kp, &SiteSource::Demo, None, None).unwrap();
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

    // -----------------------------------------------------------------------
    // The third axis — `APP-CONVENTION-FEED` entering the projection.
    // -----------------------------------------------------------------------

    /// Write `n` authored posts into a fresh directory and hand back its path.
    fn posts_dir(root: &Path, n: usize) -> std::path::PathBuf {
        let dir = root.join("posts");
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..n {
            std::fs::write(
                dir.join(format!("post-{i}.md")),
                format!(
                    "+++\ncreated_at = 2026-09-{:02}T09:00:00Z\ntitle = \"Post {i}\"\n+++\nbody {i}\n",
                    i + 1
                ),
            )
            .unwrap();
        }
        dir
    }

    /// Resolve a peer-relative key out of a published tree **through the signed
    /// root** — the manifest, the root signature and the trie walk, not a file
    /// read. `Err("Absent")` is what an un-named key answers.
    fn resolve_signed(out_dir: &Path, peer_id: &str, key: &str) -> Result<(), String> {
        let pin = crate::content_site::signed_fetch::PinnedPublisher::from_peer_id("", peer_id)
            .expect("a canonical peer-id carries its key");
        let session = crate::content_site::signed_fetch::SignedSession::new(pin);
        let src = crate::feed_publish::tests::Origin(out_dir.to_path_buf());
        let k = key.to_string();
        crate::feed_read::block_on(async move {
            session.resolve(&src, &k).await.map(|_| ()).map_err(|e| format!("{e:?}"))
        })
    }

    /// ⭐ **THE THIRD AXIS, AND THE POSITIVE HALF OF THE CONSTRAINT THAT SHAPED
    /// IT.** `feed_publish`'s
    /// `a_second_axis_signed_by_its_own_projector_un_names_the_first` measured
    /// that two `RootProjector`s over one directory **do not compose** — the
    /// second `finish` signs a root naming only its own axis, and the first
    /// silently stops resolving. That is the whole argument against a standalone
    /// `feed OUT_DIR` verb, and it is a statement about what does *not* work.
    ///
    /// This is the statement about what does: **one publish, one projector,
    /// three axes, and every one of them still named.** The site manifest and
    /// the feed head are asserted through the same signed root in the same
    /// tree — which is the property `crate::publish_axes` exists to make
    /// structural rather than remembered.
    ///
    /// Falsified — republishing the feed under its own `RootProjector` after
    /// the shared root has been signed (the standalone-verb shape, spliced into
    /// `run_projection`) reds the **site** assertion with `Err("Absent")`,
    /// i.e. the consumer reporting *the publisher withdrew this site* about a
    /// site that is right there on disk.
    #[test]
    fn one_publish_carries_every_axis_and_names_all_of_them_in_one_signed_root() {
        let tmp = std::env::temp_dir().join(format!("entity-publish-feed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.join("out");
        let posts = posts_dir(&tmp, 3);

        let code = run(&[
            "publish".into(),
            "--demo-sites".into(),
            out.to_string_lossy().to_string(),
            "--demo-identity".into(),
            format!("--ingest-feed={}", posts.display()),
        ]);
        assert_eq!(code, ExitCode::SUCCESS);

        let pid = entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED).peer_id().to_string();
        // The feed arrived…
        assert_eq!(
            resolve_signed(&out, &pid, crate::feed::index_head_key()),
            Ok(()),
            "the §4.2 head resolves through the signed root"
        );
        assert_eq!(
            resolve_signed(&out, &pid, &crate::feed::index_page_key(0)),
            Ok(()),
            "and so does page 0"
        );
        // …and did NOT take the site's name with it. This is the assertion the
        // whole design turns on.
        assert_eq!(
            resolve_signed(&out, &pid, &format!("sites/{}/manifest", crate::views::content_site::DEMO_SITE_ID)),
            Ok(()),
            "the site is still named by the root the feed publish signed — if this is \
             `Absent`, the axes have stopped composing and a standalone verb has crept back in"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// ⭐⭐ **A READER CAN SEE WHAT A PUBLISHER OFFERS WITHOUT BEING TOLD — one
    /// enumeration of one signed root names both conventions.**
    ///
    /// The operator's model of this system, stated 2026-09-13: *"you don't guess
    /// sites, you don't guess the feed, you don't guess an app — you go to the
    /// peer and see what they have, you look at their manifest, you walk their
    /// tree, and then you open whatever they offer."*
    ///
    /// **That is available today and nothing in the product does it.**
    /// `SignedSession::enumerate` walks the trie under a prefix, is bounded on
    /// both axes a large-or-hostile origin can grow, and reports whether it
    /// finished — and its only callers are its own unit tests. Meanwhile the two
    /// surfaces that need the answer guess: the Registry Browser resolves a name
    /// and opens `sites` because that is what it has always opened, and
    /// `views::games::app_source` picks the first foreign origin it holds
    /// (AP54).
    ///
    /// So this gate measures the **premise**, not a feature: with sites and a
    /// feed published in one run under one root, an enumeration of that root
    /// names keys from **both** conventions. If it ever reds, the no-guessing
    /// model has stopped being implementable and the surfaces that guess have
    /// acquired an excuse.
    ///
    /// **The empty prefix is the point.** Enumerating `sites/` is a reader who
    /// already decided what they were looking for; the model above is a reader
    /// who has not, and the difference is exactly one argument — falsified that
    /// way, and the failure prints the site keys under the words *sites only*.
    #[test]
    fn one_enumeration_of_a_publishers_root_names_every_convention_they_carry() {
        let tmp =
            std::env::temp_dir().join(format!("entity-publish-enumerate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.join("out");
        let posts = posts_dir(&tmp, 2);

        let code = run(&[
            "publish".into(),
            "--demo-sites".into(),
            out.to_string_lossy().to_string(),
            "--demo-identity".into(),
            format!("--ingest-feed={}", posts.display()),
        ]);
        assert_eq!(code, ExitCode::SUCCESS);

        let pid = entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED).peer_id().to_string();
        let pin = crate::content_site::signed_fetch::PinnedPublisher::from_peer_id("", &pid)
            .expect("a canonical peer-id carries its key");
        let session = crate::content_site::signed_fetch::SignedSession::new(pin);
        let src = crate::feed_gather::DirOrigin(out.clone());
        // Bounded, and the bound REPORTS — a truncated enumeration would let a
        // reader conclude a publisher carries less than they do, which is the
        // same wrong answer as guessing with extra steps.
        let found = crate::feed_read::block_on(async {
            session
                .enumerate_bounded(
                    &src,
                    "",
                    crate::content_site::signed_fetch::DEFAULT_ENUMERATION_BUDGET,
                )
                .await
        })
        .expect("a tree this publish just wrote enumerates");
        assert!(
            found.complete,
            "the whole premise is that a reader learns what is there; an incomplete \
             enumeration of {} keys cannot support it — and `complete` is the field \
             that must be read before `keys`, because a short list cannot be told \
             from a small publisher",
            found.keys.len()
        );

        let sites: Vec<&String> = found.keys.iter().filter(|k| k.starts_with("sites/")).collect();
        let feed: Vec<&String> =
            found.keys.iter().filter(|k| k.starts_with("app/feed/")).collect();
        assert!(
            !sites.is_empty(),
            "no site key in an enumeration of a publisher who published sites: {:?}",
            found.keys
        );
        assert!(
            !feed.is_empty(),
            "no feed key in an enumeration of a publisher who published a feed — a reader \
             asking this publisher what they offer would be told *sites only*, which is the \
             guess the model exists to replace: {:?}",
            found.keys
        );
        // §4.2's pinned head specifically, not merely "something under app/feed":
        // the head is what a reader needs to START, so a tree whose entries are
        // named and whose index is not is a feed nobody can open.
        assert!(
            found.keys.iter().any(|k| k == crate::feed::index_head_key()),
            "the enumeration names feed keys but not the §4.2 index head, so a reader who \
             found this feed still has no way in: {feed:?}"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // -----------------------------------------------------------------------
    // The fourth axis — `APP-CONVENTION-FEED` §6's gatherer as a verb.
    // -----------------------------------------------------------------------

    /// A second publisher's seed, so a gather has somebody to gather.
    const AUTHOR_SEED: [u8; 32] = *b"the-author-this-gatherer-mirrors";

    fn author_seed_arg() -> String {
        format!("--identity-seed={}", crate::vault_codec::seed_to_hex(&AUTHOR_SEED))
    }

    fn author_peer_id() -> String {
        entity_crypto::Keypair::from_seed(AUTHOR_SEED).peer_id().to_string()
    }

    /// Publish `n` posts under [`AUTHOR_SEED`] into `out`, and hand back the id.
    ///
    /// `long_post` adds one over EMBED §3's 16 KiB inline ceiling, which
    /// `feed_ingest` lowers to the **pointer** arm — so the entry declares a
    /// blob of its own. That is the only shape in which a carried entry has a
    /// closure *below* it, and therefore the only one that can measure whether
    /// the walk decoded it with the right author.
    fn publish_an_author(tmp: &Path, out: &Path, n: usize, long_post: bool) -> String {
        let posts = posts_dir(tmp, n);
        if long_post {
            std::fs::write(
                posts.join("post-long.md"),
                format!(
                    "+++\ncreated_at = 2026-09-30T09:00:00Z\ntitle = \"Long\"\n+++\n{}\n",
                    "x".repeat(crate::embed::INLINE_PAYLOAD_MAX + 1)
                ),
            )
            .unwrap();
        }
        assert_eq!(
            run(&[
                "publish".into(),
                "--demo-sites".into(),
                out.to_string_lossy().to_string(),
                author_seed_arg(),
                format!("--ingest-feed={}", posts.display()),
            ]),
            ExitCode::SUCCESS,
            "the author's own publish failed, so nothing below measures a gather"
        );
        author_peer_id()
    }

    /// ⭐⭐ **THE GATHER LOOP, END TO END AND THROUGH THE PRODUCT'S OWN
    /// CONSUMER.** A publishes a feed. B gathers it and publishes a mirror in
    /// the same run as B's own site. A third party then reads B's origin —
    /// **record through B's signed root, carried bodies by key and hash** — and
    /// gets A's posts back, attributed to A.
    ///
    /// The read goes through [`crate::feed_fetch::OriginMirrorSource`], which is
    /// the production type, for the reason `ROUTING-2026-09-12-c` §3 stated
    /// before this existed: ***a map keyed by `(peer, key)` verifies nothing***.
    /// Such a double answers both legs identically and would leave the whole
    /// asymmetry this gate is about — one leg signed-root-anchored, one leg
    /// pin-and-hash-anchored — untouched by every assertion.
    #[test]
    fn a_gathered_view_is_published_and_a_stranger_reads_it_back_attributed_to_its_author() {
        let tmp =
            std::env::temp_dir().join(format!("entity-publish-gather-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let author_out = tmp.join("author");
        let author = publish_an_author(&tmp, &author_out, 3, false);

        // B publishes its own site AND the gathered view, in one run, one
        // projector, one root.
        let out = tmp.join("gatherer");
        assert_eq!(
            run(&[
                "publish".into(),
                "--demo-sites".into(),
                out.to_string_lossy().to_string(),
                "--demo-identity".into(),
                format!("--gather={author}@{}", author_out.display()),
            ]),
            ExitCode::SUCCESS
        );
        let gatherer = entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED).peer_id().to_string();
        assert_ne!(gatherer, author, "the fixture's two publishers collapsed into one");

        // B's own site is still named — the axes composed (the third axis's own
        // gate one screen up, now with a fourth in the run).
        assert_eq!(
            resolve_signed(
                &out,
                &gatherer,
                &format!("sites/{}/manifest", crate::views::content_site::DEMO_SITE_ID)
            ),
            Ok(()),
            "the mirror publish un-named B's site"
        );

        // …and the read, as a stranger performs it.
        let subject = crate::feed::MirrorSubject::timeline(&author);
        let src = crate::feed_fetch::OriginMirrorSource::new(
            "",
            &gatherer,
            std::rc::Rc::new(crate::feed_gather::DirOrigin(out.clone())),
        )
        .expect("the gatherer's peer id carries its key");
        let rows = crate::feed_read::block_on(crate::feed_mirror::read_mirror(
            &src, &gatherer, &subject, 100,
        ))
        .expect("the mirror reads back off the published origin");

        assert_eq!(rows.len(), 3, "the gathered view came back short");
        for row in &rows {
            assert_eq!(row.entry.author, author, "attribution followed the carrier");
            assert_eq!(
                row.attribution,
                crate::feed_read::Attribution::Signed,
                "an entry that travelled through a stranger lost its authorship — the \
                 author's detached signature did not survive the republish"
            );
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// ⛔ **A republish that does not re-gather leaves NO DANGLING POINTER.**
    ///
    /// The clean removes `{base}/{peer}/` and — when it believes it is alone at
    /// this base — the shared `content/` store. A carried author has no
    /// `sites/{author}/`, so `projected_peer_ids` cannot see them
    /// (`REFERENCE-PUBLISHING-PIPELINE` §0.2a measured exactly this): left to
    /// that check alone, the second publish deletes the blobs behind every
    /// carried entry and leaves the pointers naming them. **Remove-then-un-name
    /// — this repo's own rule run backwards** — and a tree that no longer
    /// resolves what it still names.
    ///
    /// The mirror RECORD going is correct and is asserted: it is under the
    /// gatherer, the clean is wholesale, and `--plan` says so in the units an
    /// operator can act on. What must not go is the evidence.
    ///
    /// Falsified: restore `if siblings.is_empty()` as the content clean's only
    /// condition and this reds with the blob missing under its own pointer.
    #[test]
    fn a_republish_that_does_not_re_gather_leaves_no_dangling_pointer() {
        let tmp =
            std::env::temp_dir().join(format!("entity-publish-regather-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let author_out = tmp.join("author");
        let author = publish_an_author(&tmp, &author_out, 2, false);
        let out = tmp.join("gatherer");
        let publish = |extra: &[String]| {
            let mut args = vec![
                "publish".to_string(),
                "--demo-sites".to_string(),
                out.to_string_lossy().to_string(),
                "--demo-identity".into(),
            ];
            args.extend(extra.iter().cloned());
            run(&args)
        };
        assert_eq!(
            publish(&[format!("--gather={author}@{}", author_out.display())]),
            ExitCode::SUCCESS
        );

        // Every carried pointer, and the blob each one names.
        let carried: Vec<std::path::PathBuf> = {
            let mut v = Vec::new();
            collect_files(&out.join(&author), "bin", &mut v);
            v
        };
        assert!(!carried.is_empty(), "nothing was carried, so this gate measures nothing");
        let blob_of = |p: &std::path::PathBuf| -> std::path::PathBuf {
            let h = http_poll::crack_pointer(&std::fs::read(p).unwrap()).unwrap();
            out.join(http_poll::content_url("", &h).trim_start_matches('/'))
        };
        assert!(carried.iter().all(|p| blob_of(p).exists()));

        // The republish, with no `--gather` — the ordinary case, a content fix.
        assert_eq!(publish(&[]), ExitCode::SUCCESS);

        for p in &carried {
            assert!(
                p.exists(),
                "a carried pointer was deleted by a clean that does not reach {}",
                p.display()
            );
            assert!(
                blob_of(p).exists(),
                "the pointer at {} survived and the bytes it names did not — the tree now \
                 serves a name with nothing behind it",
                p.display()
            );
        }

        // And the record itself IS gone, which is the axis behaving like every
        // other axis: carried on every publish or deleted.
        assert_eq!(
            projected_mirrors(&out, &entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED).peer_id().to_string(), ""),
            0,
            "the mirror record survived a publish that did not carry it — then the clean \
             is not wholesale and the plan's term is a lie"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// ⛔ **`--verify` fails when a mirror's declared evidence is missing — and
    /// BOTH legs are falsified separately.**
    ///
    /// `REFERENCE-PUBLISHING-PIPELINE` §0.2a said an arm could not close this,
    /// because the sweep is rooted at the publisher's subtree. The correction is
    /// in `verify_signed_root`'s mirror arm and this is the measurement behind
    /// it: a mirror record *declares* `(peer, hash)` pairs, the bytes are in the
    /// shared content store (not foreign at all), and the pointer is the one
    /// genuinely foreign half — so the arm checks both and neither alone is
    /// enough.
    ///
    /// Leg 1 gone = bytes a consumer cannot fetch. Leg 2 gone = bytes present
    /// and unreachable, because a carried entry is reached by KEY under its
    /// author. **Either one alone passes a tree that does not serve**, which is
    /// why the two arms are asserted apart rather than as "verify goes red".
    #[test]
    fn verify_follows_a_mirror_and_fails_on_either_half_of_what_it_declares() {
        let tmp =
            std::env::temp_dir().join(format!("entity-publish-vmirror-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let author_out = tmp.join("author");
        let author = publish_an_author(&tmp, &author_out, 2, true);
        let out = tmp.join("gatherer");
        assert_eq!(
            run(&[
                "publish".into(),
                "--demo-sites".into(),
                out.to_string_lossy().to_string(),
                "--demo-identity".into(),
                format!("--gather={author}@{}", author_out.display()),
            ]),
            ExitCode::SUCCESS
        );
        let verify = || {
            run(&[
                "publish".into(),
                "--demo-sites".into(),
                out.to_string_lossy().to_string(),
                "--demo-identity".into(),
                "--verify".into(),
            ])
        };
        assert_eq!(verify(), ExitCode::SUCCESS, "a complete mirror must verify clean");

        // ⛔ **And it verifies clean WITHOUT calling the authors' signatures
        // rubbish.** Measured by hand before this assertion existed: every
        // carried `system/signature` came back as an *orphan* — a blob no
        // pointer references, which `run_verify` describes as *"dead weight the
        // sync will keep re-uploading"*. It is `FEED-R2`'s entire authorship
        // instrument, referenced by a pointer under the author that this
        // sweep's own root does not cover. **Exit code 0 is not the whole
        // report**, and a gate that only reads the code would have shipped a
        // verb that tells an operator to delete the evidence.
        assert_eq!(
            LAST_ORPHAN_COUNT.with(|c| c.get()),
            0,
            "a carried mirror left orphans — if these are the authors' signatures, \
             --verify is describing FEED-R2's evidence as dead weight"
        );

        // One carried ENTRY pointer, and the blob behind it.
        let entry_pointer = {
            let mut v = Vec::new();
            collect_files(&out.join(&author).join(crate::feed::entry_prefix()), "bin", &mut v);
            v.sort();
            v.into_iter().next().expect("the gather carried an entry")
        };
        let entry_blob = {
            let h = http_poll::crack_pointer(&std::fs::read(&entry_pointer).unwrap())
                .unwrap();
            out.join(http_poll::content_url("", &h).trim_start_matches('/'))
        };

        // Leg 1 — the bytes.
        let saved = std::fs::read(&entry_blob).unwrap();
        std::fs::remove_file(&entry_blob).unwrap();
        assert_eq!(
            verify(),
            ExitCode::from(VERIFY_DEFECT_EXIT),
            "a mirror naming a post whose body is not projected verified CLEAN"
        );
        std::fs::write(&entry_blob, &saved).unwrap();
        assert_eq!(verify(), ExitCode::SUCCESS, "the restore did not restore");

        // Leg 2 — the key a consumer reaches it BY, which is the half outside
        // the sweep's own root.
        let saved_ptr = std::fs::read(&entry_pointer).unwrap();
        std::fs::remove_file(&entry_pointer).unwrap();
        assert_eq!(
            verify(),
            ExitCode::from(VERIFY_DEFECT_EXIT),
            "a mirror whose carried entry has no pointer under its author verified CLEAN \
             — the bytes are there and nothing can ask for them"
        );
        std::fs::write(&entry_pointer, &saved_ptr).unwrap();
        assert_eq!(verify(), ExitCode::SUCCESS);

        // Leg 3 — **the closure BELOW a carried entry**, which is what says the
        // walk decoded it as its author's rather than as ours.
        //
        // `FEED-R1` refuses an entry whose `author` is not the namespace it was
        // read in, so a walk passing the *publisher's* id for a gathered entry
        // gets a decode failure — which is only an `eprintln!`, so the exit code
        // never moves and nothing here would notice. What it silently skips is
        // this: the body's blob. One post in this fixture is over EMBED §3's
        // inline ceiling and therefore carries a pointer, so dropping its blob
        // MUST red — and does not, if `carried_author` is not consulted.
        let long_blob = {
            let mut found = None;
            let mut pointers = Vec::new();
            collect_files(&out.join(&author).join(crate::feed::entry_prefix()), "bin", &mut pointers);
            for p in pointers {
                let h = http_poll::crack_pointer(&std::fs::read(&p).unwrap()).unwrap();
                let blob =
                    out.join(http_poll::content_url("", &h).trim_start_matches('/'));
                let entity =
                    http_poll::verify_and_decode(&std::fs::read(&blob).unwrap(), &h).unwrap();
                let entry = crate::feed::FeedEntry::from_entity(&entity, &author).unwrap();
                if let Some(b) = crate::feed_tree::body_blob_hashes(&entry.body).first() {
                    found = Some(out.join(http_poll::content_url("", b).trim_start_matches('/')));
                    break;
                }
            }
            found.expect(
                "no carried entry took the pointer arm — the fixture's long post did not \
                 land, so this leg measures nothing",
            )
        };
        std::fs::remove_file(&long_blob).unwrap();
        assert_eq!(
            verify(),
            ExitCode::from(VERIFY_DEFECT_EXIT),
            "a carried post whose BODY is not projected verified CLEAN — the walk reached \
             the entry and could not read it, which is what happens when a gathered entry \
             is decoded under the publisher's own namespace instead of its author's"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The CLI refusals, each of which exists because the alternative is silent.
    #[test]
    fn a_gather_the_address_space_cannot_hold_is_refused_at_the_boundary() {
        // §6.0.1 derives one address per subject, so two gathers of one author
        // would write two views at one key and the second would win in silence.
        let dup = vec!["alice@/a".to_string(), "alice@/b".to_string()];
        let err = parse_gathers(&dup).unwrap_err();
        assert!(err.contains("gathered twice"), "{err}");
        // …and two DIFFERENT authors are fine, which is what makes the above a
        // statement about the address and not about repetition.
        assert!(parse_gathers(&["alice@/a".into(), "bob@/b".into()]).is_ok());

        // Shape refusals.
        assert!(parse_gathers(&["alice".to_string()]).is_err(), "no directory");
        assert!(parse_gathers(&["@/a".to_string()]).is_err(), "no author");
        assert!(parse_gathers(&["alice@".to_string()]).is_err(), "empty directory");

        // Gathering yourself is the feed axis, not a mirror — and it is caught
        // where the gatherer id is known rather than at parse time.
        let err = resolve_gathers("QmMe", &[("QmMe".into(), PathBuf::from("/a"))]).unwrap_err();
        assert!(err.contains("--ingest-feed"), "{err}");
    }

    /// **A prefixed publish puts every axis INSIDE the prefix**, and the feed is
    /// the one that can get this wrong: `emit_owned_sites` and `emit_app_set`
    /// both take `prefix` and join it themselves, while `publish_feed` writes
    /// `{dir}/{peer}/…` directly — so the axis has to hand it the prefixed base.
    /// Get it backwards and the whole archive lands at the origin root, outside
    /// the hosting scope its own signed root is served from, where no consumer
    /// resolving `{origin}/{prefix}/…` will ever look.
    #[test]
    fn a_prefixed_publish_puts_every_axis_inside_the_prefix() {
        let tmp =
            std::env::temp_dir().join(format!("entity-publish-feed-pfx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.join("out");
        let posts = posts_dir(&tmp, 2);

        assert_eq!(
            run(&[
                "publish".into(),
                "--demo-sites".into(),
                out.to_string_lossy().to_string(),
                "--demo-identity".into(),
                "--prefix=scope".into(),
                format!("--ingest-feed={}", posts.display()),
            ]),
            ExitCode::SUCCESS
        );

        let pid = entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED).peer_id().to_string();
        let head = format!("{}.bin", crate::feed::index_head_key());
        assert!(
            out.join("scope").join(&pid).join(&head).exists(),
            "the feed head is under the hosting prefix"
        );
        assert!(
            !out.join(&pid).join(&head).exists(),
            "and NOT at the origin root, where the prefix's consumers cannot reach it"
        );
        // The control: the site axis lands under the prefix too, so this is a
        // statement about the prefix and not about the feed being special.
        assert!(out.join("scope").join(&pid).join("sites").is_dir());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// ⭐ **A plan against a freshly-created output directory says so, instead of
    /// reporting a reassuring zero — `<coordination-tree>`'s `B-5`.**
    ///
    /// All three drop guards count what is already in `{out}`. Their pipeline
    /// does `rm -rf "$out"; mkdir -p "$out"` before every run, so all three read
    /// zero and `--plan` reported *"nothing would be removed"* on a publish that
    /// replaces an entire estate. The guards are right about what they measure;
    /// the sentence was wrong about what it meant. **`nothing would be removed`
    /// and `there is nothing here to compare against` are different facts**, and
    /// a publisher deciding whether a deploy is safe needs the second one said
    /// out loud.
    ///
    /// Both arms, because a predicate asserted from one side passes for an
    /// implementation that always answers that way — the `F-5` lesson.
    #[test]
    fn a_plan_against_a_fresh_output_directory_does_not_report_a_reassuring_zero() {
        let tmp = std::env::temp_dir()
            .join(format!("entity-publish-plan-fresh-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.join("out");
        let pid = entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED).peer_id().to_string();

        // ARM 1: what meta's pipeline hands us — the directory exists and holds
        // nothing. Every count is 0 and none of them means "safe".
        std::fs::create_dir_all(&out).unwrap();
        assert!(
            !prior_projection_present(&out, &pid, ""),
            "a recreated output directory has nothing to compare against, and the \
             plan must not present its zeros as 'nothing would be removed'"
        );
        assert_eq!(projected_site_ids(&out, &pid, "").len(), 0, "…and the counts really are 0");
        assert_eq!(projected_feed_posts(&out, &pid, ""), 0);

        // ARM 2: after a real publish the comparison has a subject, so the note
        // must NOT fire — otherwise it prints on every run and stops being read.
        let args = vec![
            "publish".to_string(),
            "--demo-sites".to_string(),
            out.to_string_lossy().to_string(),
            "--demo-identity".into(),
        ];
        assert_eq!(run(&args), ExitCode::SUCCESS);
        assert!(
            prior_projection_present(&out, &pid, ""),
            "a populated out dir IS a real comparison — the note must stay quiet here"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // -----------------------------------------------------------------------
    // `B-4` — the site arm. Three ways to say where sites come from, and
    // silence is not one of them.
    // -----------------------------------------------------------------------

    /// **The decision, at the level it lives.** Pure, so every combination is
    /// gated by `make test` rather than only through a process invocation.
    ///
    /// The last two rows are the ones a later edit breaks: `--ingest-apps` and
    /// `--ingest-feed` are *other axes*, and a `starts_with("--ingest")` here
    /// would silently read a feed publish as a site publish.
    #[test]
    fn the_site_arm_is_exactly_one_of_three_and_silence_is_an_error() {
        let arm = |v: &[&str]| {
            let mut args = vec!["publish".to_string(), "out".to_string()];
            args.extend(v.iter().map(|s| s.to_string()));
            parse_site_source(&args)
        };

        assert_eq!(arm(&["--ingest=/render"]), Ok(SiteSource::Ingest(PathBuf::from("/render"))));
        assert_eq!(arm(&["--demo-sites"]), Ok(SiteSource::Demo));
        assert_eq!(arm(&["--no-sites"]), Ok(SiteSource::None));

        // Silence. This is the whole change: it used to mean `--demo-sites`,
        // which is the same command line as forgetting `--ingest`.
        let err = arm(&[]).unwrap_err();
        for arm_name in ["--ingest=", "--demo-sites", "--no-sites"] {
            assert!(err.contains(arm_name), "the refusal must name every arm; got: {err}");
        }

        // Every pair and the triple. Each refusal names *what was passed*, so an
        // operator reads their own two flags back rather than a rule to go find.
        for combo in [
            vec!["--ingest=/r", "--demo-sites"],
            vec!["--ingest=/r", "--no-sites"],
            vec!["--demo-sites", "--no-sites"],
            vec!["--ingest=/r", "--demo-sites", "--no-sites"],
        ] {
            let err = arm(&combo).unwrap_err();
            for flag in &combo {
                let name = flag.split('=').next().unwrap();
                assert!(err.contains(name), "{err}\n…should name {name}");
            }
        }

        assert!(arm(&["--ingest-feed=/posts"]).is_err(), "the feed axis is not the site arm");
        assert!(arm(&["--ingest-apps=/dist"]).is_err(), "the apps axis is not the site arm");
        assert_eq!(arm(&["--ingest-apps=/dist", "--no-sites"]), Ok(SiteSource::None));
    }

    /// **A publish that says nothing about sites writes nothing at all.**
    ///
    /// The `--verify` row is the ordering half: verifying an already-published
    /// tree asks nothing about a source, so requiring an arm for it would refuse
    /// a verification for a reason with no bearing on it.
    #[test]
    fn a_publish_that_names_no_site_arm_refuses_before_it_writes() {
        let tmp =
            std::env::temp_dir().join(format!("entity-publish-nosilence-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.join("out");
        let out_s = out.to_string_lossy().to_string();

        assert_eq!(
            run(&["publish".to_string(), out_s.clone(), "--demo-identity".into()]),
            ExitCode::FAILURE,
            "silence is refused"
        );
        assert!(!out.exists(), "a refused publish creates nothing — not even the out dir");

        // The arm is refused BELOW the `--verify` return, so a verification of a
        // tree that does not exist fails on the tree, not on a missing flag.
        // Both are FAILURE here; what distinguishes them is that the publish
        // below now succeeds against the same flags plus an arm.
        assert_eq!(
            run(&[
                "publish".to_string(),
                out_s.clone(),
                "--demo-identity".into(),
                "--demo-sites".into(),
            ]),
            ExitCode::SUCCESS,
            "…and the same command WITH an arm publishes"
        );
        assert_eq!(
            run(&["publish".to_string(), out_s, "--demo-identity".into(), "--verify".into()]),
            ExitCode::SUCCESS,
            "--verify needs no site arm: it reads a tree, it does not resolve a source"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// **`B-4`'s shape: publish a feed and nothing else.** A real domain whose
    /// whole purpose is a timeline — declarable in `<coordination-tree>`'s
    /// `estate.conf` since 2026-09-15, and refused by their `publish.sh` until
    /// this arm existed because the alternative was two demo sites nobody asked
    /// for.
    ///
    /// The `sites/` assertion is the gate: *the feed published* is equally true
    /// of a publish that carried the demo set along beside it.
    #[test]
    fn a_feed_only_publish_carries_a_feed_and_no_sites() {
        let tmp =
            std::env::temp_dir().join(format!("entity-publish-feedonly-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.join("out");
        let posts = posts_dir(&tmp, 4);
        let pid = entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED).peer_id().to_string();

        assert_eq!(
            run(&[
                "publish".to_string(),
                out.to_string_lossy().to_string(),
                "--demo-identity".into(),
                "--no-sites".into(),
                format!("--ingest-feed={}", posts.display()),
            ]),
            ExitCode::SUCCESS
        );

        assert_eq!(projected_feed_posts(&out, &pid, ""), 4, "the feed is what this domain serves");
        assert!(
            projected_site_ids(&out, &pid, "").is_empty(),
            "…and NOTHING invented a site beside it"
        );
        assert!(
            !out.join(SITE_URL_PREFIX).exists(),
            "no sites/ projection at all — not an empty one"
        );
        // Reachable through the signed root, not merely present on disk.
        assert!(
            resolve_signed(&out, &pid, crate::feed::index_head_key()).is_ok(),
            "the feed index resolves through this publish's own signed root"
        );

        // The two site-shaped modes project a site, so the arm contradicts them
        // — refused at the flags, where the operator can read their own word
        // back, rather than as "no sites found on peer X" further in.
        for mode in ["--bare-root", "--html-only"] {
            assert_eq!(
                run(&[
                    "publish".to_string(),
                    out.to_string_lossy().to_string(),
                    "--demo-identity".into(),
                    "--no-sites".into(),
                    mode.to_string(),
                ]),
                ExitCode::FAILURE,
                "{mode} projects a site and --no-sites declares none"
            );
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// **`B-4`'s two riding questions, answered where a visitor feels them.**
    ///
    /// A site-free domain has no home site and cannot boot into one. The
    /// document omits `home_site` entirely; `--surface=site` and a bare
    /// `--surface=window` are refused; a `window` that names its own address is
    /// the coherent case and is the point of the whole arm.
    ///
    /// **`--surface` defaults to `window`**, so the refusal is also what a
    /// caller gets for saying nothing — which is why the message names both
    /// ways forward rather than only the flag that was wrong.
    #[test]
    fn a_site_free_deployment_has_no_home_site_and_a_surface_that_needs_none() {
        let tmp =
            std::env::temp_dir().join(format!("entity-publish-nohome-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let posts = posts_dir(&tmp, 2);
        let feed_flag = format!("--ingest-feed={}", posts.display());
        let call = |out: &Path, extra: &[&str]| {
            let mut args = vec![
                "publish".to_string(),
                out.to_string_lossy().to_string(),
                "--demo-identity".into(),
                "--no-sites".into(),
                feed_flag.clone(),
                "--deployment-config".into(),
            ];
            args.extend(extra.iter().map(|s| s.to_string()));
            run(&args)
        };

        // The answer: chrome, and no home_site key at all.
        let chrome = tmp.join("chrome");
        assert_eq!(call(&chrome, &["--surface=chrome"]), ExitCode::SUCCESS);
        let doc: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(chrome.join("entity-deployment.json")).unwrap(),
        )
        .unwrap();
        assert!(
            doc.get("home_site").is_none(),
            "a domain with no sites has no home site — the key is ABSENT, not empty: {doc}"
        );
        assert_eq!(doc["surface"], "chrome");
        // …and the consumer agrees it affirms nobody, which is what keeps a
        // site-free domain from clearing a returning profile's records.
        let parsed = crate::deployment_config::DeploymentConfig::parse(&doc.to_string()).unwrap();
        assert_eq!(parsed.affirmed_home(), "", "no home_site affirms no peer");
        assert!(
            crate::peer_supersession::stale_against_declared(
                &[("OLD".to_string(), "NEW".to_string())].into_iter().collect(),
                parsed.affirmed_home(),
                &parsed.superseded,
            )
            .is_empty(),
            "an empty `current` drops nothing — a feed-only domain is inert for a \
             returning profile, it does not quietly wipe its supersession records"
        );

        // A window that names what it opens at is coherent with no sites, and is
        // the reason the arm is worth having.
        let aimed = tmp.join("aimed");
        let address = crate::open_target::feed("2KPUBLISHEREXAMPLE").to_uri().unwrap();
        assert_eq!(
            call(
                &aimed,
                &["--surface=window", "--window-type=Feed", &format!("--window-target={address}")]
            ),
            ExitCode::SUCCESS
        );
        let aimed_doc: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(aimed.join("entity-deployment.json")).unwrap(),
        )
        .unwrap();
        assert!(aimed_doc.get("home_site").is_none());
        assert_eq!(aimed_doc["window_target"], serde_json::Value::String(address));

        // Three refusals, each its own mistake.
        let bad = tmp.join("bad");
        assert_eq!(
            call(&bad, &["--surface=site"]),
            ExitCode::FAILURE,
            "the overlay's entire content is a site"
        );
        assert_eq!(
            call(&bad, &["--surface=window"]),
            ExitCode::FAILURE,
            "a Site Browser aimed at a home site that does not exist is an empty rail"
        );
        assert_eq!(
            call(&bad, &["--surface=chrome", "--set-home"]),
            ExitCode::FAILURE,
            "--set-home moves a home onto this peer and there is none to move"
        );
        assert_eq!(
            call(&bad, &["--surface=chrome", "--config-site=demo"]),
            ExitCode::FAILURE,
            "--config-site names a site this publish does not carry"
        );
        assert!(
            !bad.join("entity-deployment.json").exists(),
            "every one of those refused BEFORE emitting a document"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// **`run_plan` owes every axis a term, and this is the feed's.** Its own
    /// doc comment states the rule — *"if a future emitter adds a third subgraph
    /// under the peer prefix, it owes a term here in the same commit"* — and the
    /// reason is the apps incident: the clean removes `{peer}/` wholesale, so a
    /// republish that forgets `--ingest-feed` does not leave the archive alone,
    /// it **deletes** it. For a feed that is the one thing in the output tree
    /// that cannot be re-derived from anything else in it.
    ///
    /// The last step is what makes it a regression test rather than a
    /// tautology: it proves the removal the plan predicts is real.
    #[test]
    fn a_plan_that_omits_the_feed_reports_the_posts_it_would_delete() {
        let tmp =
            std::env::temp_dir().join(format!("entity-publish-plan-feed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.join("out");
        let posts = posts_dir(&tmp, 4);
        let with_feed = format!("--ingest-feed={}", posts.display());
        let call = |extra: &[&str]| {
            let mut args =
                vec![
                    "publish".to_string(),
                    "--demo-sites".into(),
                    out.to_string_lossy().to_string(),
                    "--demo-identity".into(),
                ];
            args.extend(extra.iter().map(|s| s.to_string()));
            run(&args)
        };

        // 1. Publish WITH the feed.
        assert_eq!(call(&[&with_feed]), ExitCode::SUCCESS);
        let pid = entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED).peer_id().to_string();
        assert_eq!(projected_feed_posts(&out, &pid, ""), 4, "precondition: four posts projected");

        // 2. Plan the same publish WITHOUT it — the destructive exit code, not
        //    "nothing would be removed".
        assert_eq!(
            call(&["--plan"]),
            ExitCode::from(PLAN_DESTRUCTIVE_EXIT),
            "a plan that omits the feed must not report a safe publish"
        );
        // …and the plan wrote nothing.
        assert_eq!(projected_feed_posts(&out, &pid, ""), 4);

        // 3. Run it, and confirm the deletion the plan predicted is real.
        assert_eq!(call(&[]), ExitCode::SUCCESS);
        assert_eq!(
            projected_feed_posts(&out, &pid, ""),
            0,
            "a publish without --ingest-feed really does delete the archive — if this ever \
             becomes 4, the clean stopped covering app/feed/** and the plan above is now \
             a false alarm"
        );

        // Re-planning with the feed back is quiet again: the destructive code is
        // about the omission, not about feeds existing.
        assert_eq!(call(&["--plan", &with_feed]), ExitCode::SUCCESS);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// **F8's hole, one convention over, closed in the commit that opened
    /// it.** `--verify`'s heuristic window scan filters on *presence*, so it
    /// cannot see *absence* — that is why an `app/site-asset` and a
    /// `system/content/blob` are DECODED rather than scanned. A feed entry over
    /// EMBED §3's ceiling carries the identical `payload: {tag: "pointer"}`, so
    /// it owed the same arm, and the third publish axis is what made it
    /// reachable.
    ///
    /// Worse than the asset case by the amount that a post is more than a
    /// figure: an asset's absence drops an image out of a page that still reads;
    /// an entry's absence is **the post**, which appears in the index and
    /// renders empty.
    ///
    /// Both levels asserted separately, for the asset gate's reason: deleting
    /// the chunk and deleting the blob take different arms, and a gate that
    /// removed only one would pass for an implementation that wired the other.
    #[test]
    fn a_pointer_bodys_closure_is_declared_so_verify_fails_when_the_post_loses_its_bytes() {
        let tmp = std::env::temp_dir()
            .join(format!("entity-publish-feedclosure-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let posts = tmp.join("posts");
        let out = tmp.join("out");
        std::fs::create_dir_all(&posts).unwrap();

        // Over the ceiling and non-uniform, so the chunker really splits it.
        let long: String = (0..(crate::embed::INLINE_PAYLOAD_MAX as u32 + 4_000))
            .map(|i| char::from(b'a' + ((i.wrapping_mul(2_654_435_761) >> 11) % 26) as u8))
            .collect();
        std::fs::write(
            posts.join("long.md"),
            format!("+++\ncreated_at = 2026-09-10T09:00:00Z\ntitle = \"Long\"\n+++\n{long}"),
        )
        .unwrap();

        // The same staging the ingest performs, so we know exactly which blob
        // and chunks the publish will emit — found by construction rather than
        // by scanning the output for "the big one", which the demo sites the
        // publish also carries would make ambiguous.
        let scratch: std::sync::Arc<dyn entity_store::ContentStore> =
            std::sync::Arc::new(entity_store::MemoryContentStore::new());
        let staged = crate::content_site::asset_store::stage(
            "text/markdown",
            long.as_bytes().to_vec(),
            &scratch,
        )
        .unwrap();
        let blob_hash = staged.asset.pointer().expect("an oversized body chunks");
        let chunk_hash = crate::content_site::asset_store::chunk_hashes_of(&staged.content[0])
            .unwrap()
            .first()
            .copied()
            .expect("the blob names at least one chunk");

        let out_s = out.to_string_lossy().to_string();
        let call = |extra: &[&str]| {
            let mut args = vec![
                "publish".to_string(),
                "--demo-sites".into(),
                out_s.clone(),
                "--demo-identity".into(),
            ];
            args.extend(extra.iter().map(|s| s.to_string()));
            run(&args)
        };
        assert_eq!(
            call(&[&format!("--ingest-feed={}", posts.to_string_lossy())]),
            ExitCode::SUCCESS
        );
        assert_eq!(call(&["--verify"]), ExitCode::SUCCESS, "the published tree is clean");

        let at = |h: &entity_hash::Hash| {
            let hex = h.to_hex();
            out.join("content").join(&hex[0..2]).join(&hex[2..4]).join(&hex)
        };

        // Arm 1 — a chunk. The blob still resolves, so the chain is intact for
        // one hop and stops at reassembly.
        let chunk = at(&chunk_hash);
        let chunk_bytes = std::fs::read(&chunk).expect("the chunk was projected");
        std::fs::remove_file(&chunk).unwrap();
        assert_eq!(
            call(&["--verify"]),
            ExitCode::from(VERIFY_DEFECT_EXIT),
            "a chunk the blob DECLARES is not projected — the post cannot be reassembled"
        );
        std::fs::write(&chunk, &chunk_bytes).unwrap();
        assert_eq!(call(&["--verify"]), ExitCode::SUCCESS, "restored tree is clean again");

        // Arm 2 — the blob itself, reached from the ENTRY's body payload. This
        // is the arm that did not exist before this commit.
        let blob = at(&blob_hash);
        let blob_bytes = std::fs::read(&blob).expect("the blob was projected");
        std::fs::remove_file(&blob).unwrap();
        assert_eq!(
            call(&["--verify"]),
            ExitCode::from(VERIFY_DEFECT_EXIT),
            "the blob a feed entry DECLARES is not projected — verify must fail on the ENTRY \
             arm, not only on the blob-declares-chunks one"
        );
        std::fs::write(&blob, &blob_bytes).unwrap();
        assert_eq!(call(&["--verify"]), ExitCode::SUCCESS);

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
            let mut args = vec![
                "publish".to_string(),
                "--demo-sites".into(),
                out.clone(),
                "--demo-identity".into(),
            ];
            args.extend(extra.iter().map(|s| s.to_string()));
            run(&args)
        };

        let demo_kp = entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED);
        let PublishSource { peer_id: pid, sites: _sites, app_sets: _apps, .. } = 
            resolve_publish_source(demo_kp, &SiteSource::Demo, None, None).unwrap();

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
        let PublishSource { peer_id: pid, sites: _sites, app_sets: _apps, .. } = 
            resolve_publish_source(demo_kp, &SiteSource::Demo, None, None).unwrap();
        // Seed an apps projection by hand — the shape a previous publish left.
        let bundles = tmp.join(&pid).join("apps").join("games").join("bundles");
        std::fs::create_dir_all(&bundles).unwrap();
        std::fs::write(bundles.join("war.bin"), b"x").unwrap();
        assert_eq!(projected_app_sets(&tmp, &pid, ""), vec![("games".to_string(), 1)]);

        let out = tmp.to_string_lossy().to_string();
        let args = vec![
            "publish".to_string(),
            "--demo-sites".to_string(),
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
            let mut args = vec![
                "publish".to_string(),
                "--demo-sites".into(),
                out.clone(),
                "--demo-identity".into(),
            ];
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
                "--demo-sites".to_string(),
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
                "--demo-sites".to_string(),
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

    /// **A declared succession survives a later publish by the OTHER peer.**
    ///
    /// The whole point of `--supersede` is that a returning profile reads it on
    /// a warm boot, so the day it stops being in the document is the day the
    /// mechanism stops working — and the document is rewritten by every publish.
    /// The home-publish arm rebuilds the domain's own fields from scratch, which
    /// is exactly how `origins` siblings were being dropped before the merge; a
    /// succession dropped the same way would be *silent*, because nothing 404s
    /// and nothing renders wrong. It just never reaches anyone.
    ///
    /// Falsified by making the merge non-additive (build `superseded` from
    /// `spec` alone): the second assertion reds with an empty map.
    #[test]
    fn a_declared_succession_survives_a_later_publish_by_another_peer() {
        let tmp = std::env::temp_dir()
            .join(format!("entity-publish-supersede-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.to_string_lossy().to_string();
        let read_doc = || -> serde_json::Value {
            serde_json::from_str(
                &std::fs::read_to_string(tmp.join("entity-deployment.json")).unwrap(),
            )
            .unwrap()
        };

        let a = "d1".repeat(32);
        let b = "d2".repeat(32);

        // Peer A takes the domain and declares that some earlier identity of a
        // tenant was replaced. The ids are opaque to the emitter, which is the
        // point: only the deployer knows the pair.
        assert_eq!(
            run(&[
                "publish".to_string(),
                "--demo-sites".to_string(),
                out.clone(),
                "--deployment-config".to_string(),
                format!("--identity-seed={a}"),
                "--prefix=alpha".to_string(),
                "--supersede=2KTenantOld=2KTenantNew".to_string(),
            ]),
            ExitCode::SUCCESS
        );
        assert_eq!(read_doc()["superseded"]["2KTenantOld"].as_str(), Some("2KTenantNew"));

        // A secondary publish contributes its origin and must not disturb it…
        assert_eq!(
            run(&[
                "publish".to_string(),
                "--demo-sites".to_string(),
                out.clone(),
                "--deployment-config".to_string(),
                format!("--identity-seed={b}"),
                "--prefix=beta".to_string(),
            ]),
            ExitCode::SUCCESS
        );
        assert_eq!(
            read_doc()["superseded"]["2KTenantOld"].as_str(),
            Some("2KTenantNew"),
            "a secondary publish dropped a succession the domain had declared"
        );

        // …and neither must the home publisher republishing its own fields,
        // which is the arm that rebuilds the document from scratch.
        assert_eq!(
            run(&[
                "publish".to_string(),
                "--demo-sites".to_string(),
                out.clone(),
                "--deployment-config".to_string(),
                format!("--identity-seed={a}"),
                "--prefix=alpha".to_string(),
            ]),
            ExitCode::SUCCESS
        );
        assert_eq!(
            read_doc()["superseded"]["2KTenantOld"].as_str(),
            Some("2KTenantNew"),
            "the home publish rebuilt the document and dropped the succession — the same \
             clobber `origins` had, one field along and silent"
        );

        // And a document that declares nothing carries no empty map, so a
        // consumer cannot read "declared nothing" as "declared an empty set".
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        assert_eq!(
            run(&[
                "publish".to_string(),
                "--demo-sites".to_string(),
                out.clone(),
                "--deployment-config".to_string(),
                format!("--identity-seed={a}"),
            ]),
            ExitCode::SUCCESS
        );
        assert!(read_doc().get("superseded").is_none());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The `--supersede` refusals, at the CLI boundary where an operator can
    /// read them — audit F9's rule. The last one is the emitter's own: a typo
    /// that retires one peer twice would otherwise be resolved by silently
    /// keeping whichever came last.
    #[test]
    fn a_malformed_supersession_is_refused_where_the_operator_can_see_it() {
        let ok = parse_supersessions(&["2KA=2KB".to_string()]).unwrap();
        assert_eq!(ok.get("2KA").map(String::as_str), Some("2KB"));

        for bad in ["2KA", "2KA=", "=2KB", "2KA=2KA"] {
            assert!(
                parse_supersessions(&[bad.to_string()]).is_err(),
                "{bad:?} should be refused"
            );
        }
        assert!(
            parse_supersessions(&["2KA=2KB".to_string(), "2KA=2KC".to_string()]).is_err(),
            "one peer has one successor"
        );
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
                "--demo-sites".to_string(),
                out.clone(),
                "--deployment-config".to_string(),
                format!("--identity-seed={seed}"),
            ])
        };
        let verify = |seed: &str| {
            run(&[
                "publish".to_string(),
                "--demo-sites".to_string(),
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
                "--demo-sites".to_string(),
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
            let mut args = vec![
                "publish".to_string(),
                "--demo-sites".into(),
                out.clone(),
                "--demo-identity".into(),
            ];
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
            let mut args = vec![
                "publish".to_string(),
                "--demo-sites".into(),
                out.clone(),
                "--demo-identity".into(),
            ];
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
            let mut args = vec![
                "publish".to_string(),
                "--demo-sites".into(),
                out.clone(),
                "--demo-identity".into(),
            ];
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

    /// **A pointer asset's blob closure is DECLARED, so `--verify` must fail
    /// when it is not projected — and before this gate it did not.**
    ///
    /// Measured by hand on a real `make site` tree the day the pointer arm
    /// landed: delete the asset's chunk and verify printed *"every pointer
    /// resolves and every body hashes to its address"*, exit 0, with the
    /// closure count silently dropping 11 → 10. That is Audit F8's failure a
    /// second time, in a new shape — the 33-byte window scan *does* find a
    /// `payload.hash`, but it filters on presence, so **it cannot see
    /// absence**. The comment F8 left in this very function says what to do
    /// about it: *"A heuristic scan cannot close it… Structure can."* An
    /// `app/site-asset` declares its blob and a `system/content/blob`
    /// declares its chunks, so both get decoded rather than scanned.
    ///
    /// Worse here than for an interior trie node, which is why it needs its
    /// own gate rather than riding the existing one: a missing trie node
    /// makes the whole subtree unreadable and is loud, while a missing asset
    /// closure renders the page perfectly and drops only the image.
    ///
    /// **Both levels are asserted separately.** Deleting the chunk and
    /// deleting the blob take different arms, and a gate that only removed
    /// one would pass for an implementation that wired the other.
    #[test]
    fn a_pointer_assets_blob_closure_is_declared_so_verify_fails_when_it_is_missing() {
        let tmp = std::env::temp_dir()
            .join(format!("entity-publish-assetclosure-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let src = tmp.join("src");
        let out = tmp.join("out");
        std::fs::create_dir_all(src.join("pages")).unwrap();
        std::fs::create_dir_all(src.join("assets/figures")).unwrap();
        std::fs::write(
            src.join("site.manifest.json"),
            br#"{"site_id":"ac","title":"Asset Closure","nav":[]}"#,
        )
        .unwrap();
        std::fs::write(
            src.join("pages/index.md"),
            b"# Home\n\n::embed[Fig]{ref=assets/figures/big.png}\n",
        )
        .unwrap();
        // Over the ceiling, and non-uniform: a constant buffer would make
        // every chunker agree by accident.
        let big: Vec<u8> = (0..(crate::content_site::format::INLINE_PAYLOAD_MAX as u32 + 4_000))
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 11) as u8)
            .collect();
        std::fs::write(src.join("assets/figures/big.png"), &big).unwrap();

        let out_s = out.to_string_lossy().to_string();
        let call = |extra: &[&str]| {
            let mut args = vec!["publish".to_string(), out_s.clone(), "--demo-identity".into()];
            args.extend(extra.iter().map(|s| s.to_string()));
            run(&args)
        };
        assert_eq!(
            call(&[&format!("--ingest={}", src.to_string_lossy())]),
            ExitCode::SUCCESS
        );
        assert_eq!(call(&["--verify"]), ExitCode::SUCCESS, "the published tree is clean");

        // Find the closure members by DECODED TYPE, never by size or sort
        // position — the sizes overlap once the chunker splits and hash-keyed
        // filenames have no stable order (the lesson
        // `verify_proves_a_published_tree_and_catches_a_tampered_body` already
        // paid for one test over).
        let mut blobs = Vec::new();
        collect_files(&out.join("content"), "", &mut blobs);
        let typed = |want: &str| -> std::path::PathBuf {
            blobs
                .iter()
                .find(|p| {
                    std::fs::read(p)
                        .ok()
                        .and_then(|b| ciborium::from_reader::<ciborium::Value, _>(&b[..]).ok())
                        .and_then(|v| {
                            use entity_ecf::ValueExt;
                            v.get("type").and_then(|t| t.as_str()).map(|t| t == want)
                        })
                        .unwrap_or(false)
                })
                .unwrap_or_else(|| panic!("the publish emitted a {want}"))
                .clone()
        };
        let blob = typed(entity_types::TYPE_CONTENT_BLOB);
        let chunk = typed(entity_types::TYPE_CONTENT_CHUNK);

        // Arm 1 — the chunk. The blob still resolves, so the pointer chain is
        // intact for one hop and stops at reassembly.
        let chunk_bytes = std::fs::read(&chunk).unwrap();
        std::fs::remove_file(&chunk).unwrap();
        assert_eq!(
            call(&["--verify"]),
            ExitCode::from(VERIFY_DEFECT_EXIT),
            "a chunk the blob DECLARES is not projected — the image cannot be reassembled and \
             verify must say so"
        );
        std::fs::write(&chunk, &chunk_bytes).unwrap();
        assert_eq!(call(&["--verify"]), ExitCode::SUCCESS, "restored tree is clean again");

        // Arm 2 — the blob itself, reached from the asset entity's payload.
        let blob_bytes = std::fs::read(&blob).unwrap();
        std::fs::remove_file(&blob).unwrap();
        assert_eq!(
            call(&["--verify"]),
            ExitCode::from(VERIFY_DEFECT_EXIT),
            "the blob a site asset DECLARES is not projected — verify must fail on the asset arm, \
             not only on the chunk one"
        );
        std::fs::write(&blob, &blob_bytes).unwrap();
        assert_eq!(call(&["--verify"]), ExitCode::SUCCESS);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// **An app's asset-bundle index declares its files, so a missing file is a
    /// broken tree** — the same structural arm the site asset and the feed entry
    /// have, landed with the type rather than after it. Through the real CLI:
    /// `--ingest-apps` a `dist/` whose app declares a bundle, verify clean,
    /// delete the one file blob, verify must fail, restore, clean again.
    #[test]
    fn an_app_asset_bundle_declares_its_files_so_verify_fails_when_one_is_missing() {
        let tmp = std::env::temp_dir()
            .join(format!("entity-publish-appassets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let dist = tmp.join("dist");
        let out = tmp.join("out");
        std::fs::create_dir_all(dist.join("vm.assets/guest")).unwrap();
        std::fs::write(
            dist.join("index.json"),
            br#"[{"id":"vm","name":"VM","type":"tool","x-assets":["guest"]}]"#,
        )
        .unwrap();
        std::fs::write(dist.join("vm.html"), b"<html><body>vm</body></html>").unwrap();
        std::fs::write(dist.join("vm.assets/guest/kernel"), b"the only file in the bundle").unwrap();

        let out_s = out.to_string_lossy().to_string();
        let call = |extra: &[&str]| {
            let mut args = vec![
                "publish".to_string(),
                "--demo-sites".into(),
                out_s.clone(),
                "--demo-identity".into(),
            ];
            args.extend(extra.iter().map(|s| s.to_string()));
            run(&args)
        };
        assert_eq!(
            call(&[&format!("--ingest-apps={}", dist.to_string_lossy())]),
            ExitCode::SUCCESS
        );
        assert_eq!(call(&["--verify"]), ExitCode::SUCCESS, "the published tree is clean");

        // The one `system/content/blob` in the tree is the bundle file's: the
        // catalog and app bundle are plain entities, not chunked content.
        let mut files = Vec::new();
        collect_files(&out.join("content"), "", &mut files);
        let blob = files
            .iter()
            .find(|p| {
                std::fs::read(p)
                    .ok()
                    .and_then(|b| ciborium::from_reader::<ciborium::Value, _>(&b[..]).ok())
                    .and_then(|v| {
                        use entity_ecf::ValueExt;
                        v.get("type").and_then(|t| t.as_str()).map(|t| t == entity_types::TYPE_CONTENT_BLOB)
                    })
                    .unwrap_or(false)
            })
            .expect("the publish emitted the bundle file's blob")
            .clone();

        let bytes = std::fs::read(&blob).unwrap();
        std::fs::remove_file(&blob).unwrap();
        assert_eq!(
            call(&["--verify"]),
            ExitCode::from(VERIFY_DEFECT_EXIT),
            "a file the asset index DECLARES is not projected — the app would be told it is \
             unavailable, and verify must say so first"
        );
        std::fs::write(&blob, &bytes).unwrap();
        assert_eq!(call(&["--verify"]), ExitCode::SUCCESS, "restored tree is clean again");
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
            let mut args = vec![
                "publish".to_string(),
                "--demo-sites".into(),
                out.clone(),
                "--demo-identity".into(),
            ];
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
            run(&["publish".to_string(), "--demo-sites".into(), out.clone(), "--demo-identity".into()]),
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
            content: Vec::new(),
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
        let PublishSource { peer_id: _pid, sites, app_sets: _apps, .. } = 
            resolve_publish_source(demo_kp, &SiteSource::Demo, None, None).unwrap();
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
            "--demo-sites".to_string(),
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
            "--demo-sites".to_string(),
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

    // ── The gathered-feed fixture — §6's browser leg ──────────────────────
    //
    // `a_gathered_view_is_published_and_a_stranger_reads_it_back_attributed_to_\
    // its_author` above proves the loop natively, through the production
    // `OriginMirrorSource`. What it cannot prove is that a **browser** shows it:
    // the rig had never registered a gatherer origin or published a mirror to
    // one, so *"a mirror reaches a person"* rested on native gates plus the
    // type being the production one. This emits the tree that closes it.
    //
    // **The author is published into a THROWAWAY directory and never served.**
    // That is the scenario and it is also the anti-vacuity guard: with no origin
    // for A, the published leg cannot contribute, so anything the browser shows
    // came through the mirror. A rig that served both would go green with the
    // mirror leg entirely unwired.

    /// The author a browser cannot reach directly. Distinct from every other
    /// fixture seed here, so a scenario that silently published the wrong
    /// identity cannot pass as a gather.
    const GATHERED_AUTHOR_SEED: [u8; 32] = *b"the-author-a-browser-cant-reach\0";

    /// The gatherer whose origin the browser DOES have. Likewise distinct.
    const GATHERER_SEED: [u8; 32] = *b"the-gatherer-a-browser-reads-th0";

    /// Publish A's feed to a scratch dir, then B's site + a mirror of A + a
    /// deployment document into `ENTITY_GATHER_OUT`.
    ///
    /// `--surface=chrome` so the e2e can open the Feed window from the palette;
    /// the default `window` surface would boot a maximized Site Browser over the
    /// thing under test. `--set-home` for every fixture emitter's reason: this
    /// one DEFINES the domain its scenario boots against, and the document
    /// merges rather than clobbers.
    #[test]
    #[ignore = "e2e fixture generator; run by the e2e harness via --ignored"]
    fn emit_gathered_feed_fixture() {
        let out = std::env::var("ENTITY_GATHER_OUT").unwrap_or_else(|_| "dist".to_string());
        let tmp = std::env::temp_dir()
            .join(format!("entity-gather-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // A's tree. Local to this process and thrown away — the browser never
        // fetches from it, and that is the point.
        let author_out = tmp.join("author");
        let posts = posts_dir(&tmp, 3);
        let author_hex = crate::vault_codec::seed_to_hex(&GATHERED_AUTHOR_SEED);
        assert_eq!(
            run(&[
                "publish".into(),
                "--demo-sites".into(),
                author_out.to_string_lossy().to_string(),
                format!("--identity-seed={author_hex}"),
                format!("--ingest-feed={}", posts.display()),
            ]),
            ExitCode::SUCCESS,
            "the author's own publish failed, so there is nothing to gather"
        );
        let author = entity_crypto::Keypair::from_seed(GATHERED_AUTHOR_SEED).peer_id().to_string();

        // B's tree — site, mirror and the document that tells a browser where B
        // is. One run, one projector, one root (`publish_axes`).
        let gatherer_hex = crate::vault_codec::seed_to_hex(&GATHERER_SEED);
        assert_eq!(
            run(&[
                "publish".into(),
                "--demo-sites".into(),
                out.clone(),
                format!("--identity-seed={gatherer_hex}"),
                format!("--gather={author}@{}", author_out.display()),
                "--deployment-config".into(),
                "--set-home".into(),
                "--surface=chrome".into(),
            ]),
            ExitCode::SUCCESS,
            "the gatherer's publish failed"
        );

        // Hand the two ids to the harness through the artifact, never through a
        // seed the test re-derives: the browser's belief comes from these bytes,
        // so the test's notion of who is who must come from them too. The
        // gatherer is already in the document's `origins`; the author is not
        // (deliberately — see above), so it is written beside it.
        std::fs::write(
            std::path::Path::new(&out).join("gathered-feed-fixture.json"),
            format!("{{\n  \"author\": \"{author}\",\n  \"gatherer\": \"{}\"\n}}\n",
                entity_crypto::Keypair::from_seed(GATHERER_SEED).peer_id()),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ── The published-feed fixture — the feed's ORDINARY leg ──────────────
    //
    // The gathered fixture above is the extraordinary case: an author a browser
    // cannot reach, read through somebody else. The ordinary one — *this
    // publisher serves their own feed at their own origin, and a reader follows
    // them* — had no browser rig at all, so `Leg::Published` was gated only
    // natively while the leg a real deployment uses had never been executed in a
    // browser once.
    //
    // **One peer, sites AND a feed, one publish, one signed root.** That is the
    // thing worth having a rig for beyond the leg itself: it is the shape
    // `examples/entity-demo/` documents, and the assertion that two application
    // conventions coexist under one identity is otherwise only a sentence.

    /// The publisher whose own origin serves their own feed. Distinct from every
    /// other seed here, so a scenario that published the wrong identity cannot
    /// pass as this one.
    const PUBLISHED_FEED_SEED: [u8; 32] = *b"the-author-who-serves-their-own\0";

    /// How many posts the fixture authors. Handed to the harness in the
    /// artifact rather than duplicated as a literal in the e2e file — a count
    /// written down twice is a count that disagrees with itself the first time
    /// somebody adds a post.
    ///
    /// ⭐ **The number is chosen to CROSS A PAGE BOUNDARY and stay under the
    /// reader's limit.** `feed_publish::DEFAULT_PAGE_SIZE` is 32 and
    /// `feed_fetch::LIMIT` is 50, so 34 entries publish as two `app/feed/index`
    /// pages and a conformant walk must fetch **both** — the head names the
    /// newest page, and the oldest post is on the other one. Every other feed
    /// fixture in this tree is a single page, which means *"a reader that
    /// fetched the head and the first page and stopped"* had never been
    /// falsifiable anywhere. `FEED-R12` forbids assuming a page size; a
    /// one-page population cannot tell an implementation that walks from one
    /// that got lucky.
    const PUBLISHED_FEED_POSTS: usize = 34;

    /// Publish one peer's site set **and** their feed into `ENTITY_PUBLISHED_FEED_OUT`,
    /// with a deployment document that registers their origin.
    ///
    /// `--surface=chrome` so the e2e can open the Feed window from the palette.
    /// `--set-home` for every fixture emitter's reason: this one defines the
    /// domain its scenario boots against, and the document merges rather than
    /// clobbers.
    ///
    /// **The last post is deliberately over EMBED §3's 16 KiB inline ceiling**,
    /// so the archive this browser reads back contains a body on the *pointer*
    /// arm. That arm is gated natively at the chunker and at the publish
    /// closure; what nothing covered is a real consumer resolving a tree that
    /// contains one. It costs one post and it is the row a synthetic
    /// all-inline fixture can never contain — which is the same population
    /// argument that let the legacy-asset regression ship.
    #[test]
    #[ignore = "e2e fixture generator; run by the e2e harness via --ignored"]
    fn emit_published_feed_fixture() {
        let out = std::env::var("ENTITY_PUBLISHED_FEED_OUT").unwrap_or_else(|_| "dist".to_string());
        let tmp = std::env::temp_dir()
            .join(format!("entity-published-feed-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Authored here rather than through `posts_dir`, which spells its dates
        // `2026-09-{i+1}` and therefore cannot count past 30 without emitting a
        // date TOML refuses. Titles are zero-padded so `Post 00` is nobody
        // else's prefix — the harness asserts the oldest by name.
        let posts = tmp.join("posts");
        std::fs::create_dir_all(&posts).unwrap();
        for i in 0..PUBLISHED_FEED_POSTS - 1 {
            std::fs::write(
                posts.join(format!("post-{i:02}.md")),
                format!(
                    "+++\ncreated_at = 2026-09-{:02}T{:02}:00:00Z\ntitle = \"Post {i:02}\"\n+++\n\
                     body {i}\n",
                    1 + i / 10,
                    i % 10
                ),
            )
            .unwrap();
        }
        // The oversized one, and the newest. `stage` refuses an inline payload
        // above the ceiling, so this body has exactly one conformant encoding
        // and the publish must carry its blob as well as the entry.
        let long = "This post is long on purpose. ".repeat(700);
        assert!(long.len() > 16_384, "the oversized post is not oversized: {}", long.len());
        std::fs::write(
            posts.join("post-long.md"),
            format!(
                "+++\ncreated_at = 2026-09-20T09:00:00Z\ntitle = \"Post Long\"\n+++\n{long}\n"
            ),
        )
        .unwrap();

        let hex = crate::vault_codec::seed_to_hex(&PUBLISHED_FEED_SEED);
        assert_eq!(
            run(&[
                "publish".into(),
                "--demo-sites".into(),
                out.clone(),
                format!("--identity-seed={hex}"),
                format!("--ingest-feed={}", posts.display()),
                "--deployment-config".into(),
                "--set-home".into(),
                "--surface=chrome".into(),
            ]),
            ExitCode::SUCCESS,
            "the publisher's own publish failed, so there is no feed to read"
        );

        // The id comes out of the ARTIFACT, never re-derived from the seed in the
        // harness: the browser's belief comes from these bytes, so the test's
        // notion of who it is reading must come from the same place.
        std::fs::write(
            std::path::Path::new(&out).join("published-feed-fixture.json"),
            format!(
                "{{\n  \"author\": \"{}\",\n  \"posts\": {PUBLISHED_FEED_POSTS}\n}}\n",
                entity_crypto::Keypair::from_seed(PUBLISHED_FEED_SEED).peer_id()
            ),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
    }

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
            "--demo-sites".to_string(),
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
            "--demo-sites".to_string(),
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
        let PublishSource { peer_id, sites, app_sets: _games, .. } =
            resolve_publish_source(
                entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED),
                &SiteSource::Demo,
                None,
                None,
            )
                .unwrap();
        let spec = DeployConfigSpec {
            surface: "site".to_string(),
            window_type: String::new(),
            window_target: String::new(),
            locked: true,
            site: None,            // → demo
            origin: String::new(), // same-origin
            registry_pin: None,
            set_home: false,
            superseded: Default::default(),
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
        let PublishSource { peer_id, sites, app_sets: _apps, .. } =
            resolve_publish_source(
                entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED),
                &SiteSource::Demo,
                None,
                None,
            )
                .unwrap();
        let spec = DeployConfigSpec {
            surface: "site".to_string(),
            window_type: String::new(),
            window_target: String::new(),
            locked: false,
            site: None,
            origin: String::new(),
            registry_pin: Some(pin.clone()),
            set_home: false,
            superseded: Default::default(),
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
            window_target: String::new(),
            locked: false,
            site: None,
            origin: String::new(),
            registry_pin: None,
            set_home: false,
            superseded: Default::default(),
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
        let PublishSource { peer_id, sites, app_sets: _games, .. } =
            resolve_publish_source(
                entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED),
                &SiteSource::Demo,
                None,
                None,
            )
                .unwrap();
        let spec = DeployConfigSpec {
            surface: "window".to_string(),
            window_type: "Site Browser".to_string(),
            window_target: String::new(),
            locked: false,
            site: None,
            origin: String::new(),
            registry_pin: None,
            set_home: false,
            superseded: Default::default(),
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

    /// ⭐ **A deployment can say WHICH feed, end to end** — the one-field gap
    /// `DESIGN-2026-09-12-BROWSING-WITHOUT-PRIVILEGING-A-CONVENTION` §2 measured:
    /// before this, `surface: window` + `window_type: Feed` could name the viewer
    /// and had no way to name a publisher, so a domain declaring *"open my feed"*
    /// booted an empty picker.
    ///
    /// Asserted through the whole chain — emitter → JSON → `parse` → `apply_to` →
    /// `BootSurface` — because each hop is where the address could be dropped, and
    /// a test of the emitter alone would pass with the parser ignoring the key.
    #[test]
    fn a_deployment_can_name_which_feed_its_startup_window_opens_at() {
        use crate::deployment_config::DeploymentConfig;

        let dir = tempfile::tempdir().unwrap();
        let PublishSource { peer_id, sites, .. } = resolve_publish_source(
            entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED),
            &SiteSource::Demo,
            None,
            None,
        )
        .unwrap();
        let address = crate::open_target::feed("2KPUBLISHEREXAMPLE").to_uri().unwrap();
        let spec = DeployConfigSpec {
            surface: "window".to_string(),
            window_type: "Feed".to_string(),
            window_target: address.clone(),
            locked: false,
            site: None,
            origin: String::new(),
            registry_pin: None,
            set_home: false,
            superseded: Default::default(),
        };
        let path = emit_deployment_config(dir.path(), &peer_id, &sites, &spec).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let cfg = DeploymentConfig::parse(&raw).unwrap();
        assert_eq!(cfg.window_target.as_deref(), Some(address.as_str()));

        let applied = cfg.apply_to(crate::session_config::SessionConfig::default());
        let crate::session_config::BootSurface::Window { window_type, target, .. } =
            &applied.boot_surface
        else {
            panic!("expected a window surface, got {:?}", applied.boot_surface)
        };
        assert_eq!(window_type, "Feed");
        assert_eq!(
            target.as_ref().map(|t| t.peer()),
            Some("2KPUBLISHEREXAMPLE"),
            "the startup window must know whose feed it opens at"
        );
        // And the routing agrees with the declared window type, which is what the
        // CLI refuses a contradiction of.
        assert_eq!(
            crate::open_target::route(target.as_ref().unwrap()),
            crate::open_target::Routing::Viewer("Feed")
        );
    }

    /// **Declaring nothing emits no key** — the same choice `superseded` makes,
    /// and for the same reason: *"named no target"* and *"named the empty
    /// target"* are different facts, and a document that cannot tell them apart
    /// makes the absent case unexpressible.
    #[test]
    fn a_deployment_that_names_no_target_carries_no_target_key() {
        use crate::deployment_config::DeploymentConfig;

        let dir = tempfile::tempdir().unwrap();
        let PublishSource { peer_id, sites, .. } = resolve_publish_source(
            entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED),
            &SiteSource::Demo,
            None,
            None,
        )
        .unwrap();
        let spec = DeployConfigSpec {
            surface: "window".to_string(),
            window_type: "Site Browser".to_string(),
            window_target: String::new(),
            locked: false,
            site: None,
            origin: String::new(),
            registry_pin: None,
            set_home: false,
            superseded: Default::default(),
        };
        let path = emit_deployment_config(dir.path(), &peer_id, &sites, &spec).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("window_target"), "{raw}");
        let cfg = DeploymentConfig::parse(&raw).unwrap();
        assert_eq!(cfg.window_target, None);
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
        let PublishSource { peer_id: _pid, sites, app_sets: _games, .. } =
            resolve_publish_source(
                entity_crypto::Keypair::from_seed(DEMO_PUBLISH_SEED),
                &SiteSource::Demo,
                None,
                None,
            )
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
