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

    // ---- Build-time deployment profile (reframe §5) --------------------
    // `ENTITY_PROFILE` bakes the COLD-BOOT default posture into this binary:
    // `full` (today's default) / `tutorial` / `strict-site`. It only seeds the
    // default when no durable session config exists — a persisted config
    // always wins on a warm boot. Validated here so a typo fails the build
    // loudly instead of silently booting Full. Read back via
    // `Profile::build_default()` (`option_env!("ENTITY_PROFILE")`).
    println!("cargo:rerun-if-env-changed=ENTITY_PROFILE");
    let profile = env::var("ENTITY_PROFILE").unwrap_or_else(|_| "full".to_string());
    match profile.as_str() {
        "full" | "tutorial" | "strict-site" => {}
        other => panic!(
            "ENTITY_PROFILE='{other}' is not a known deployment profile \
             (expected: full | tutorial | strict-site)"
        ),
    }
    println!("cargo:rustc-env=ENTITY_PROFILE={profile}");
    println!("cargo:warning=deployment profile: {profile} (cold-boot default posture)");

    // ---- Build-time home site (boot-closure cut 2a) --------
    // `ENTITY_HOME_*` bakes the COLD-BOOT default `home_site` — the startup
    // page a CDN-deployed instance points at — into this binary. It is the
    // build-time DEFAULT / TEST path for a thin-lens remote deployment; the
    // production knob is the per-domain `/entity-deployment.json` fetch (cut
    // 2b). Like `ENTITY_PROFILE`, it only seeds the absent-config case — a
    // persisted session config always wins on a warm boot. Unset (the default
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
