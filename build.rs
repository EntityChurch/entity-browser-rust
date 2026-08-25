//! Build script — embeds a directory tree of `*.md` files as a static
//! array of `(rel_path, title, content)` tuples accessible from the
//! application at runtime.
//!
//! This is the dogfood ingest path: the Knowledge Base window is
//! seeded with documentation on app startup. The docs are baked into
//! the binary at compile time, so this works for WASM and Tauri builds
//! without any filesystem access at runtime — important because the
//! browser/mobile target loads once and then goes offline.
//!
//! **Source root** is `KB_DOCS_ROOT`. **Unset = embed nothing** —
//! the Knowledge Base is an occasional opt-in POC feature, so the
//! default build is fast, lean, and doesn't flood the worker's OPFS.
//! Opt in explicitly: `KB_DOCS_ROOT=..` for the whole workspace
//! parent, `KB_DOCS_ROOT=docs` for just this crate's docs, or any
//! absolute path. Relative roots resolve against `CARGO_MANIFEST_DIR`.
//!
//! **Filters** (all opt-in; unset = no filter):
//! - `KB_DOCS_MAX_BYTES` — skip any single file larger than N bytes
//!   (the workspace has a handful of multi-MB raw-transcript dumps).
//! - `KB_DOCS_MAX_AGE_DAYS` — skip files whose mtime is older than N
//!   days. The corpus is dominated by old historical material, so
//!   "recent only" (e.g. `KB_DOCS_MAX_AGE_DAYS=14`) is the cheapest
//!   way to cut it down to what's actively worth reviewing.
//!
//! Each file is keyed by its **POSIX path relative to the root** with
//! the `.md` extension stripped. That key is used verbatim as the
//! article's tree sub-path, so the knowledge base mirrors the on-disk
//! directory structure. Keys are collision-free by construction (a
//! real filesystem can't have two files at the same path).
//!
//! The generated file is written to `$OUT_DIR/embedded_docs.rs` and
//! is `include!`'d from `src/views/knowledge_base/ingest.rs`.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Directory names never descended into during the walk. These hold
/// build artifacts / vendored trees whose markdown is noise (and, in
/// `target/`, would be enormous).
const SKIP_DIRS: &[&str] = &["target", "node_modules", "dist", ".git", ".cargo"];

/// Per-file include filters, all opt-in via env (unset = no filter).
struct Filters {
    /// `KB_DOCS_MAX_BYTES` — skip files larger than this.
    max_bytes: Option<u64>,
    /// `KB_DOCS_MAX_AGE_DAYS` — skip files whose mtime is older than
    /// this cutoff. The whole entity-systems corpus is dominated by
    /// old historical dumps; "recent only" is the cheapest big cut.
    min_mtime: Option<SystemTime>,
}

impl Filters {
    /// True if `entry`'s file should be embedded under these filters.
    fn accepts(&self, entry: &fs::DirEntry) -> bool {
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => return false,
        };
        if let Some(cap) = self.max_bytes {
            if meta.len() > cap {
                return false;
            }
        }
        if let Some(cutoff) = self.min_mtime {
            match meta.modified() {
                Ok(m) if m >= cutoff => {}
                _ => return false,
            }
        }
        true
    }
}

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
    let out_path = Path::new(&out_dir).join("embedded_docs.rs");

    // Resolve the docs root from KB_DOCS_ROOT (absolute, or relative
    // to the manifest dir). **Unset = embed nothing.** The Knowledge
    // Base is an occasional opt-in POC feature — defaulting to the
    // whole workspace made every build slow, bloated the wasm, and
    // flooded the worker's OPFS on load (which broke reload
    // persistence). Opt in explicitly when you actually want docs on
    // a device, e.g. `KB_DOCS_ROOT=.. KB_DOCS_MAX_AGE_DAYS=14`.
    let docs_root: Option<PathBuf> = match env::var("KB_DOCS_ROOT") {
        Ok(v) if !v.trim().is_empty() => {
            let p = PathBuf::from(v.trim());
            Some(if p.is_absolute() {
                p
            } else {
                Path::new(&manifest_dir).join(p)
            })
        }
        _ => None,
    };

    let max_bytes: Option<u64> = env::var("KB_DOCS_MAX_BYTES")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok());

    // Skip files whose mtime is older than N days ago. mtime is the
    // practical "added/changed recently" proxy (std has no portable
    // birthtime), matching `find -mtime`.
    let min_mtime: Option<SystemTime> = env::var("KB_DOCS_MAX_AGE_DAYS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .and_then(|days| {
            SystemTime::now().checked_sub(Duration::from_secs(days.saturating_mul(86_400)))
        });

    let filters = Filters {
        max_bytes,
        min_mtime,
    };

    // Re-run when the corpus or the knobs change.
    //
    // NOTE: the age filter is wall-clock relative. Cargo only re-runs
    // this script on file/env changes, not merely because time passed
    // — so a much-later rebuild with no doc/env change can reuse a
    // stale window. In practice docs in active dirs change (bumping
    // mtime → rerun-if-changed) and we rebuild often; `touch build.rs`
    // forces re-evaluation if ever needed.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=KB_DOCS_ROOT");
    println!("cargo:rerun-if-env-changed=KB_DOCS_MAX_BYTES");
    println!("cargo:rerun-if-env-changed=KB_DOCS_MAX_AGE_DAYS");

    // ---- Build-time startup surface (reframe §5) -----------------------
    // `ENTITY_STARTUP_SURFACE` bakes the COLD-BOOT default surface into this
    // binary: `chrome` (today's default) / `site` / `window`, with
    // `ENTITY_STARTUP_WINDOW_TYPE` naming the window type when surface=window.
    // This replaced the old opaque `ENTITY_PROFILE` presets — only the surface
    // AXIS is baked; the granular posture (`site_mode`, `peer_creation_enabled`)
    // is a per-domain `/entity-deployment.json` concern (a locked kiosk lives
    // there, not the build). It only seeds the default when no durable session
    // config exists — a persisted config always wins on a warm boot. Validated
    // here so a typo fails the build loudly. Read back via `boot_default()`
    // (`option_env!("ENTITY_STARTUP_SURFACE")`).
    println!("cargo:rerun-if-env-changed=ENTITY_STARTUP_SURFACE");
    println!("cargo:rerun-if-env-changed=ENTITY_STARTUP_WINDOW_TYPE");
    let surface = env::var("ENTITY_STARTUP_SURFACE").unwrap_or_else(|_| "chrome".to_string());
    match surface.as_str() {
        "chrome" | "site" | "window" => {}
        other => panic!(
            "ENTITY_STARTUP_SURFACE='{other}' is not a known startup surface \
             (expected: chrome | site | window)"
        ),
    }
    let startup_window_type = env::var("ENTITY_STARTUP_WINDOW_TYPE").unwrap_or_default();
    println!("cargo:rustc-env=ENTITY_STARTUP_SURFACE={surface}");
    println!("cargo:rustc-env=ENTITY_STARTUP_WINDOW_TYPE={startup_window_type}");
    println!("cargo:warning=startup surface: {surface} (cold-boot default surface)");

    // ---- Build-time home site (boot-closure cut 2a) --------
    // `ENTITY_HOME_*` bakes the COLD-BOOT default `home_site` — the startup
    // page a CDN-deployed instance points at — into this binary. It is the
    // build-time DEFAULT / TEST path for a thin-lens remote deployment; the
    // production knob is the per-domain `/entity-deployment.json` fetch (cut
    // 2b). Like `ENTITY_STARTUP_SURFACE`, it only seeds the absent-config case —
    // a persisted session config always wins on a warm boot. Unset (the default
    // build) ⇒ all empty ⇒ the bundled local demo, byte-identical to before.
    //   * ENTITY_HOME_PEER   — the hosting peer-id ("" = local/system peer)
    //   * ENTITY_HOME_SITE   — the site id (default "demo")
    //   * ENTITY_HOME_LOC    — the landing page within the site ("" = root)
    //   * ENTITY_HOME_ORIGIN — http(s) origin where that peer's published
    //                          artifacts live (seeds the site-origin registry)
    // Always emitted (empty when unset) so `option_env!` is deterministic;
    // the Rust side treats empty as absent. Read back in `session_config`.
    for var in [
        "ENTITY_HOME_PEER",
        "ENTITY_HOME_SITE",
        "ENTITY_HOME_LOC",
        "ENTITY_HOME_ORIGIN",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    let home_peer = env::var("ENTITY_HOME_PEER").unwrap_or_default();
    let home_site = env::var("ENTITY_HOME_SITE").unwrap_or_default();
    let home_loc = env::var("ENTITY_HOME_LOC").unwrap_or_default();
    let home_origin = env::var("ENTITY_HOME_ORIGIN").unwrap_or_default();
    // Validate the origin scheme if set — a typo here means the remote home
    // silently never resolves, so fail the build loudly instead (mirrors the
    // profile validation).
    let has_http_scheme =
        home_origin.starts_with("http://") || home_origin.starts_with("https://");
    if !home_origin.is_empty() && !has_http_scheme {
        panic!(
            "ENTITY_HOME_ORIGIN='{home_origin}' must be an http(s) origin \
             (e.g. https://labs.example) — got no recognized scheme"
        );
    }
    // A remote home needs both a peer and an origin to resolve; warn (don't
    // fail) on a half-config so the misconfiguration is visible at build time.
    if home_peer.is_empty() != home_origin.is_empty() {
        println!(
            "cargo:warning=ENTITY_HOME_PEER and ENTITY_HOME_ORIGIN should be set together \
             for a remote home (peer='{home_peer}', origin='{home_origin}')"
        );
    }
    println!("cargo:rustc-env=ENTITY_HOME_PEER={home_peer}");
    println!("cargo:rustc-env=ENTITY_HOME_SITE={home_site}");
    println!("cargo:rustc-env=ENTITY_HOME_LOC={home_loc}");
    println!("cargo:rustc-env=ENTITY_HOME_ORIGIN={home_origin}");
    if !(home_peer.is_empty()
        && home_site.is_empty()
        && home_loc.is_empty()
        && home_origin.is_empty())
    {
        println!(
            "cargo:warning=home site: peer='{home_peer}' site='{home_site}' \
             loc='{home_loc}' origin='{home_origin}' (cold-boot default home)"
        );
    }

    // `ENTITY_WEBRTC_*` bakes the §6.5 signaling node this build rendezvouses
    // through (`session_config::webrtc_provisioning_default`). Its reason for
    // existing is the pairing problem: machine A serves the SPA to the phone
    // and the laptop, so **the URL they already have to type can BE the
    // pairing** — bake A's own node here and a browser that merely loads the
    // app is provisioned, with nothing to retype, no QR, and no camera (which
    // an insecure origin does not have anyway).
    //
    // **These were read by `option_env!` with no `rerun-if-env-changed`**, so
    // changing the node silently reused the cached wasm — the "a green build
    // can be a cached build" trap, on the one knob whose staleness is
    // invisible (a stale node id fails as "nobody is at the rendezvous").
    for var in [
        "ENTITY_WEBRTC_NODE_PEER",
        "ENTITY_WEBRTC_NODE_ADDR",
        "ENTITY_WEBRTC_ICE",
    ] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    let node_peer = env::var("ENTITY_WEBRTC_NODE_PEER").unwrap_or_default();
    let node_addr = env::var("ENTITY_WEBRTC_NODE_ADDR").unwrap_or_default();
    // A HALF config is fatal here, where the home-site half-config above only
    // warns, and the difference is how the failure presents. A wrong home page
    // is visible the moment you look at it; `resolve_webrtc_provisioning`
    // requires BOTH halves and yields `None` for one, so a half config installs
    // no establisher at all and reads as "nobody is at the rendezvous" [AP22] —
    // the exact silent shape this whole knob exists to remove. Fail closed and
    // loudly, at the only moment a person is watching.
    if node_peer.is_empty() != node_addr.is_empty() {
        panic!(
            "ENTITY_WEBRTC_NODE_PEER and ENTITY_WEBRTC_NODE_ADDR must be set \
             TOGETHER — both are required to provision a signaling node, and \
             one alone provisions nothing while looking configured \
             (peer='{node_peer}', addr='{node_addr}')"
        );
    }
    // Empty is legal here — the half-config case is already refused above, so an
    // empty addr at this point means "no node was asked for at all".
    let addr_scheme_ok = node_addr.is_empty()
        || node_addr.starts_with("ws://")
        || node_addr.starts_with("wss://");
    if !addr_scheme_ok {
        panic!(
            "ENTITY_WEBRTC_NODE_ADDR='{node_addr}' must be a ws:// or wss:// \
             address — a node is dialed, not fetched, and a wrong scheme fails \
             as an unreachable rendezvous rather than as a bad value"
        );
    }
    if !node_peer.is_empty() {
        println!(
            "cargo:warning=signaling node baked in: {node_peer} at {node_addr} \
             — every browser served this build rendezvouses through it with no pairing step"
        );
    }

    let mut entries: Vec<(String, String, String)> = Vec::new();
    match &docs_root {
        Some(root) if root.is_dir() => {
            // NOTE: the age filter is wall-clock relative. Cargo only
            // re-runs this script on file/env changes, not because
            // time passed — a much-later rebuild with no doc/env
            // change can reuse a stale window. Active dirs change
            // (bumping mtime → rerun-if-changed) and we rebuild often;
            // `touch build.rs` forces re-evaluation if ever needed.
            println!("cargo:rerun-if-changed={}", root.display());
            visit_dir(root, root, &filters, &mut entries);
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            // Keys are filesystem-relative paths, so duplicates are
            // impossible on a sane tree. Defensive dedup rather than a
            // panic — a 1000+ file ingest shouldn't be brittle.
            entries.dedup_by(|a, b| a.0 == b.0);
            let total_bytes: usize =
                entries.iter().map(|(k, t, c)| k.len() + t.len() + c.len()).sum();
            println!(
                "cargo:warning=knowledge-base: embedding {} docs ({:.1} MiB) from {}",
                entries.len(),
                total_bytes as f64 / (1024.0 * 1024.0),
                root.display(),
            );
        }
        _ => {
            println!(
                "cargo:warning=knowledge-base: KB_DOCS_ROOT unset — embedding 0 docs \
                 (opt in with e.g. KB_DOCS_ROOT=.. KB_DOCS_MAX_AGE_DAYS=14)"
            );
        }
    }

    let mut src = String::new();
    src.push_str(
        "// Auto-generated by build.rs — DO NOT EDIT.\n\
         // Embeds the configured docs root's **/*.md as\n\
         // (rel_path, title, content) tuples. rel_path is the POSIX\n\
         // path relative to the root, sans the .md extension.\n\n",
    );
    src.push_str("pub static EMBEDDED_DOCS: &[(&str, &str, &str)] = &[\n");
    for (key, title, content) in &entries {
        src.push_str(&format!(
            "    ({}, {}, {}),\n",
            rust_string_literal(key),
            rust_string_literal(title),
            rust_string_literal(content),
        ));
    }
    src.push_str("];\n");

    fs::write(&out_path, src).expect("write embedded_docs.rs");

    // ---- i18n locale catalogs (P4) -------------------------------------
    // Parse the JSON locale files under I18N_LOCALES_ROOT and codegen Rust
    // `&'static` literals into $OUT_DIR/embedded_locales.rs (include!'d by
    // src/i18n.rs). Unlike the KB corpus (heavy → default embed nothing),
    // locale catalogs are tiny core-feature data, so the root DEFAULTS to the
    // crate-local `locales/` — a normal build ships the languages. `en` is
    // always the compiled-in base; these layer over it with en fallback.
    generate_embedded_locales(&manifest_dir, &out_dir);
}

/// A locale message during codegen — mirrors `i18n::Message` but owns `String`s.
enum LocaleMsg {
    Simple(String),
    Plural(Vec<(&'static str, String)>),
}

/// i18n (P4): codegen `$OUT_DIR/embedded_locales.rs` from the JSON locale files
/// under `I18N_LOCALES_ROOT` (default: the crate-local `locales/`; explicit
/// empty ⇒ en-only lean build). Each `*.json` file is one locale (id = file
/// stem); its top-level object maps message keys to either a **string**
/// (`Simple`) or an **object** of CLDR-category → string (`Plural`). A
/// `locales/en.json` is ignored — `en` is the compiled-in base (`EN`), and
/// these are the locales that *layer over* it.
fn generate_embedded_locales(manifest_dir: &str, out_dir: &str) {
    use serde_json::Value;

    println!("cargo:rerun-if-env-changed=I18N_LOCALES_ROOT");

    let root: Option<PathBuf> = match env::var("I18N_LOCALES_ROOT") {
        Ok(v) if v.trim().is_empty() => None, // explicit empty = lean en-only
        Ok(v) => {
            let p = PathBuf::from(v.trim());
            Some(if p.is_absolute() {
                p
            } else {
                Path::new(manifest_dir).join(p)
            })
        }
        Err(_) => Some(Path::new(manifest_dir).join("locales")), // default: bake
    };

    // CLDR plural categories in canonical order → the Rust variant ident. A
    // category outside this set is a typo — fail the build (the selector must
    // be exact). Building the plural list by iterating this array also gives
    // deterministic canonical ordering regardless of JSON key order.
    const CATS: &[(&str, &str)] = &[
        ("zero", "Zero"),
        ("one", "One"),
        ("two", "Two"),
        ("few", "Few"),
        ("many", "Many"),
        ("other", "Other"),
    ];

    let mut locales: Vec<(String, Vec<(String, LocaleMsg)>)> = Vec::new();

    if let Some(root) = &root {
        if root.is_dir() {
            println!("cargo:rerun-if-changed={}", root.display());
            let mut files: Vec<PathBuf> = fs::read_dir(root)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
                .collect();
            files.sort();
            for path in files {
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                if id.is_empty() {
                    continue;
                }
                if id == "en" {
                    println!(
                        "cargo:warning=i18n: skipping {} — `en` is the compiled-in base",
                        path.display()
                    );
                    continue;
                }
                let text = fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("i18n: cannot read {}: {e}", path.display()));
                let json: Value = serde_json::from_str(&text)
                    .unwrap_or_else(|e| panic!("i18n: {} is not valid JSON: {e}", path.display()));
                let obj = json.as_object().unwrap_or_else(|| {
                    panic!(
                        "i18n: {} must be a JSON object of key → message",
                        path.display()
                    )
                });
                let mut entries: Vec<(String, LocaleMsg)> = Vec::new();
                for (key, val) in obj {
                    let msg = match val {
                        Value::String(s) => LocaleMsg::Simple(s.clone()),
                        Value::Object(forms) => {
                            // Reject unknown category names (typos) up front.
                            for cat in forms.keys() {
                                if !CATS.iter().any(|(name, _)| name == cat) {
                                    panic!(
                                        "i18n: {} key '{key}' has unknown plural category \
                                         '{cat}' (expected: zero one two few many other)",
                                        path.display()
                                    );
                                }
                            }
                            let mut plural: Vec<(&'static str, String)> = Vec::new();
                            for (name, variant) in CATS {
                                if let Some(fval) = forms.get(*name) {
                                    let s = fval.as_str().unwrap_or_else(|| {
                                        panic!(
                                            "i18n: {} key '{key}' category '{name}' must be a string",
                                            path.display()
                                        )
                                    });
                                    plural.push((*variant, s.to_string()));
                                }
                            }
                            LocaleMsg::Plural(plural)
                        }
                        _ => panic!(
                            "i18n: {} key '{key}' must be a string or an object of plural forms",
                            path.display()
                        ),
                    };
                    entries.push((key.clone(), msg));
                }
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                locales.push((id, entries));
            }
        }
    }

    if locales.is_empty() {
        println!(
            "cargo:warning=i18n: no overlay locales embedded (en-only) — \
             add JSON to locales/ or set I18N_LOCALES_ROOT"
        );
    } else {
        let ids: Vec<&str> = locales.iter().map(|(id, _)| id.as_str()).collect();
        println!(
            "cargo:warning=i18n: embedding {} overlay locale(s): {}",
            locales.len(),
            ids.join(", ")
        );
    }

    let mut src = String::new();
    src.push_str(
        "// Auto-generated by build.rs — DO NOT EDIT.\n\
         // Overlay locale catalogs parsed from the I18N_LOCALES_ROOT JSON.\n\
         // `en` is the compiled-in base (EN); these layer over it (en fallback).\n\n",
    );
    src.push_str("pub static EMBEDDED_LOCALES: &[EmbeddedLocale] = &[\n");
    for (id, entries) in &locales {
        src.push_str(&format!(
            "    EmbeddedLocale {{\n        id: {},\n        entries: &[\n",
            rust_string_literal(id)
        ));
        for (key, msg) in entries {
            match msg {
                LocaleMsg::Simple(s) => src.push_str(&format!(
                    "            ({}, Message::Simple({})),\n",
                    rust_string_literal(key),
                    rust_string_literal(s),
                )),
                LocaleMsg::Plural(forms) => {
                    src.push_str(&format!(
                        "            ({}, Message::Plural(&[\n",
                        rust_string_literal(key)
                    ));
                    for (variant, s) in forms {
                        src.push_str(&format!(
                            "                (PluralCategory::{}, {}),\n",
                            variant,
                            rust_string_literal(s),
                        ));
                    }
                    src.push_str("            ])),\n");
                }
            }
        }
        src.push_str("        ],\n    },\n");
    }
    src.push_str("];\n");

    let out_path = Path::new(out_dir).join("embedded_locales.rs");
    fs::write(&out_path, src).expect("write embedded_locales.rs");
}

/// Recursively walk `dir`, collecting `*.md` files keyed by their path
/// relative to `root`. Skips the `SKIP_DIRS` and any dot-directory.
fn visit_dir(
    root: &Path,
    dir: &Path,
    filters: &Filters,
    entries: &mut Vec<(String, String, String)>,
) {
    let read = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for entry in read {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path: PathBuf = entry.path();
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            if SKIP_DIRS.contains(&name) || name.starts_with('.') {
                continue;
            }
            visit_dir(root, &path, filters, entries);
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            if !filters.accepts(&entry) {
                continue;
            }
            let key = match rel_key(root, &path) {
                Some(k) if !k.is_empty() => k,
                _ => continue,
            };
            let content = match fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let title = first_h1(&content).unwrap_or(stem);
            entries.push((key, title, content));
        }
    }
}

/// POSIX path of `path` relative to `root`, with the trailing `.md`
/// stripped. Returns None if `path` isn't under `root`.
fn rel_key(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut parts: Vec<String> = rel
        .components()
        .filter_map(|c| c.as_os_str().to_str().map(str::to_string))
        .collect();
    if let Some(last) = parts.last_mut() {
        if let Some(stripped) = last.strip_suffix(".md") {
            *last = stripped.to_string();
        }
    }
    Some(parts.join("/"))
}

/// Return the first Markdown H1 heading text (the line starting with
/// `# `), or None if there isn't one.
fn first_h1(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

/// Format a string as a Rust string literal, escaping it correctly.
/// Uses raw strings when the content contains no `"#` sequences (the
/// common case for documentation), otherwise falls back to a regular
/// escaped literal.
fn rust_string_literal(s: &str) -> String {
    // Find the smallest number of `#`s such that `r#...#"..."#...#` is unambiguous.
    // The check is: the content must NOT contain `"` followed by exactly
    // n hashes. We bump n until we find one that works.
    let mut hashes = 0usize;
    loop {
        let needle: String = std::iter::once('"').chain(std::iter::repeat_n('#', hashes)).collect();
        if !s.contains(&needle) {
            break;
        }
        hashes += 1;
        if hashes > 16 {
            // Pathological — fall back to escaped literal.
            return escaped_literal(s);
        }
    }
    let hash_str: String = "#".repeat(hashes);
    format!("r{0}\"{1}\"{0}", hash_str, s)
}

fn escaped_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
