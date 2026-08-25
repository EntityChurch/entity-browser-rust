//! Persistence — Tauri backend peer storage on the spec layout
//! (`GUIDE-PERSISTENCE.md` §1):
//! ```text
//! $ENTITY_DATA_DIR or ~/.entity/peers/{name}/
//!   ├── keypair        ← PEM, restored at startup
//!   ├── config.toml    ← storage_backend, label
//!   └── store.db       ← SQLite tree (when storage_backend = "sqlite")
//! ```
//!
//! **Helpers duplicated from `entity-browser-rust/src/persistence.rs`.**
//! `src-tauri/` is excluded from the main workspace so it can't import
//! that module directly. The pure helpers (data_root, sanitize_name,
//! synthesize_name, read_config/write_default_config, etc.) match the
//! frontend-side copies exactly. If they drift, fix in both places. The
//! architecture team has an open question
//! about whether name synthesis should live in `entity-sdk`; if that
//! lands, both copies collapse upstream.
//!
//! Tauri-specific: the legacy migration source is
//! `~/.entity/backend-peers/{peer_id}` (flat PEM + sidecar `.meta` /
//! `.pub` files), distinct from the frontend's legacy `~/.entity/keys/`.

use std::path::{Path, PathBuf};

use entity_crypto::Keypair;

// ---------------------------------------------------------------------------
// Shared pure helpers (mirror src/persistence.rs exactly)
// ---------------------------------------------------------------------------

/// Resolve the configuration root. Honors `ENTITY_DATA_DIR` so tests
/// and ops can redirect without touching `$HOME`.
fn data_root() -> PathBuf {
    if let Ok(env) = std::env::var("ENTITY_DATA_DIR") {
        if !env.is_empty() {
            return PathBuf::from(env);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".entity")
}

fn peers_dir() -> PathBuf {
    let dir = data_root().join("peers");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).ok();
    }
    dir
}

fn legacy_backend_peers_dir() -> PathBuf {
    data_root().join("backend-peers")
}

/// Sanitize a label or peer-id prefix into a filesystem-safe alias.
fn sanitize_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter_map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                Some(c.to_ascii_lowercase())
            } else if c.is_whitespace() {
                Some('-')
            } else {
                None
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(|c: char| c == '-' || c == '_').to_string();
    if trimmed.is_empty() {
        "peer".to_string()
    } else {
        trimmed.chars().take(32).collect()
    }
}

/// Synthesize a unique directory name for a peer.
fn synthesize_name(peer_id: &str, label: Option<&str>) -> String {
    let base = match label {
        Some(s) if !s.is_empty() => sanitize_name(s),
        _ => sanitize_name(&peer_id[..peer_id.len().min(8)]),
    };
    let dir = peers_dir();
    if name_matches_or_free(&dir, &base, peer_id) {
        return base;
    }
    for i in 2.. {
        let candidate = format!("{}-{}", base, i);
        if name_matches_or_free(&dir, &candidate, peer_id) {
            return candidate;
        }
        if i > 100 {
            return format!("{}-{}", base, peer_id);
        }
    }
    unreachable!()
}

fn name_matches_or_free(parent: &Path, name: &str, expected_peer_id: &str) -> bool {
    let dir = parent.join(name);
    if !dir.exists() {
        return true;
    }
    let kp_path = dir.join("keypair");
    match Keypair::load_from_file(&kp_path) {
        Ok(kp) => kp.peer_id().to_string() == expected_peer_id,
        Err(_) => false,
    }
}

/// Locate a peer directory by id, **restricted to identities Tori manages**.
///
/// Every mutating caller (delete, config toggles) goes through this rather than
/// [`find_peer_dir_by_id`]. The shared store holds foreign identities, and a
/// mutation addressed by peer-id alone would happily `remove_dir_all` or
/// rewrite the `config.toml` of another tool's fixture. Not reachable from the
/// UI today — the frontend only ever holds managed ids — but the guard belongs
/// at the lookup, not in each caller's head.
///
/// Checking ownership first also makes this cheap: it skips the keypair parse
/// for every directory that is not ours.
fn find_managed_peer_dir_by_id(peer_id: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(peers_dir()).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || !is_tauri_managed(&read_config(&path)) {
            continue;
        }
        if let Ok(kp) = Keypair::load_from_file(&path.join("keypair")) {
            if kp.peer_id().to_string() == peer_id {
                return Some(path);
            }
        }
    }
    None
}

/// Locate a peer directory by id across the **whole** shared store, managed or
/// not. Used only by the legacy migration, whose question is "does this
/// identity already exist on disk anywhere" — re-migrating a peer that is
/// already present under another tool's name would duplicate it.
fn find_peer_dir_by_id(peer_id: &str) -> Option<PathBuf> {
    let dir = peers_dir();
    let entries = std::fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let kp_path = path.join("keypair");
        if let Ok(kp) = Keypair::load_from_file(&kp_path) {
            if kp.peer_id().to_string() == peer_id {
                return Some(path);
            }
        }
    }
    None
}

fn write_default_config(dir: &Path, label: Option<&str>) {
    let body = format!(
        "# entity-browser backend peer (Tauri-managed)\n\
         # Spec: GUIDE-PERSISTENCE.md §1\n\
         storage_backend = \"sqlite\"\n\
         managed_by = \"tauri\"\n\
         {}\n",
        label.map(|l| format!("label = \"{}\"", l.replace('"', "\\\""))).unwrap_or_default(),
    );
    let _ = std::fs::write(dir.join("config.toml"), body);
}

struct PeerConfigFile {
    storage_backend: String,
    label: Option<String>,
    /// Who owns this identity. `~/.entity/peers/` is the **machine-wide**
    /// identity store (`GUIDE-PERSISTENCE.md` §1) shared by every entity-core
    /// tool on the box — the CLI, the conformance harness, sibling repos'
    /// fixtures. Tori stamps `managed_by = "tauri"` on the ones it creates
    /// ([`write_default_config`]), and [`is_tauri_managed`] is the only thing
    /// that decides adoption. **Absent means not ours**, which is the
    /// fail-closed direction: adopting a foreign identity means holding its
    /// secret key and, on start, creating a `store.db` inside another tool's
    /// directory under a key we did not issue.
    managed_by: Option<String>,
    /// Serve `system/signaling` — act as a §6.5 rendezvous for peers that
    /// reach this listener. **Absent means false**, which is both the
    /// fail-closed direction and what every peer written before this key
    /// existed meant, so an old `config.toml` needs no migration.
    signaling_node: bool,
    /// Ask the router to forward this peer's listening port from the internet
    /// (PCP / NAT-PMP). **Absent means false**, same fail-closed reading as
    /// `signaling_node` and for a stronger reason: this one changes who can
    /// reach the listener from *this LAN* to *anyone*.
    port_mapping: bool,
    /// Serve the SPA over HTTP so another device on this network can load it
    /// (`app_server.rs`). **Absent means false**, like its two neighbours.
    ///
    /// Unlike them this one takes effect **live** — it is an independent TCP
    /// listener, not something mounted on `PeerBuilder` or bound to the port
    /// this run happened to get — so the flag is persistence only and the
    /// toggle never restarts the peer.
    app_server: bool,
}

impl Default for PeerConfigFile {
    fn default() -> Self {
        Self {
            storage_backend: "sqlite".into(),
            label: None,
            managed_by: None,
            signaling_node: false,
            port_mapping: false,
            app_server: false,
        }
    }
}

fn read_config(dir: &Path) -> PeerConfigFile {
    let body = match std::fs::read_to_string(dir.join("config.toml")) {
        Ok(s) => s,
        Err(_) => return PeerConfigFile::default(),
    };
    let table = match body.parse::<toml::Table>() {
        Ok(t) => t,
        Err(e) => {
            log::warn!("config.toml at {:?}: parse failed ({}); using defaults", dir, e);
            return PeerConfigFile::default();
        }
    };
    PeerConfigFile {
        storage_backend: table
            .get("storage_backend")
            .and_then(|v| v.as_str())
            .unwrap_or("sqlite")
            .to_string(),
        label: table
            .get("label")
            .and_then(|v| v.as_str())
            .map(String::from)
            .filter(|s| !s.is_empty()),
        managed_by: table
            .get("managed_by")
            .and_then(|v| v.as_str())
            .map(String::from)
            .filter(|s| !s.is_empty()),
        signaling_node: table
            .get("signaling_node")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        port_mapping: table
            .get("port_mapping")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        app_server: table.get("app_server").and_then(|v| v.as_bool()).unwrap_or(false),
    }
}

/// Flip the persisted "serve rendezvous" setting for one peer, preserving the
/// rest of its `config.toml`.
///
/// Returns whether the write landed. A peer whose directory cannot be found is
/// `false` rather than a panic — the caller surfaces that as a failed toggle.
///
/// **Rewrites the file from the parsed table** rather than appending a line, so
/// flipping twice cannot leave two `signaling_node` keys with the first one
/// winning (toml takes the first, which would make the toggle appear to stop
/// working after one use).
pub fn set_signaling_node(peer_id: &str, enabled: bool) -> bool {
    set_peer_flag(peer_id, "signaling_node", enabled)
}

/// Flip the persisted "ask the router to forward my port" setting.
/// See [`set_peer_flag`] for the round-trip rule both toggles depend on.
pub fn set_port_mapping(peer_id: &str, enabled: bool) -> bool {
    set_peer_flag(peer_id, "port_mapping", enabled)
}

/// Flip the persisted "serve the SPA over HTTP" setting.
///
/// Persistence only — the server itself starts and stops live, so unlike its
/// two neighbours this write is not followed by a peer restart.
pub fn set_app_server(peer_id: &str, enabled: bool) -> bool {
    set_peer_flag(peer_id, "app_server", enabled)
}

/// Flip one boolean in a peer's `config.toml`, preserving the rest.
///
/// Parameterized rather than copied per setting: the second toggle would
/// otherwise be a second copy of the round-trip rule below, and a copy is where
/// one of them quietly starts appending instead.
fn set_peer_flag(peer_id: &str, key: &str, enabled: bool) -> bool {
    let Some(dir) = find_managed_peer_dir_by_id(peer_id) else {
        log::warn!("set_peer_flag({key}): no managed peer dir for {}", peer_id);
        return false;
    };
    let path = dir.join("config.toml");
    let mut table = std::fs::read_to_string(&path)
        .ok()
        .and_then(|b| b.parse::<toml::Table>().ok())
        .unwrap_or_default();
    table.insert(key.into(), toml::Value::Boolean(enabled));
    match std::fs::write(&path, table.to_string()) {
        Ok(()) => {
            log::info!(
                "Backend peer {} {} = {}",
                &peer_id[..12.min(peer_id.len())],
                key,
                enabled
            );
            true
        }
        Err(e) => {
            log::warn!("set_peer_flag({key}): write {:?} failed: {}", path, e);
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Tauri-specific high-level API
// ---------------------------------------------------------------------------

/// One backend peer record loaded from disk. The `sqlite_path` is the
/// concrete `peers/{name}/store.db` location to pass to
/// `PeerBuilder::sqlite()` when the peer is started.
pub struct LoadedBackendPeer {
    pub peer_id: String,
    pub keypair: Keypair,
    pub label: Option<String>,
    pub sqlite_path: Option<PathBuf>,
    /// Persisted "serve §6.5 rendezvous" setting. Consulted at start, where
    /// `signaling_node::resolve_enabled` lets the environment override it.
    pub signaling_node: bool,
    /// Persisted "ask the router to forward my port" setting. Consulted at
    /// start, where `port_mapping::resolve_enabled` lets the environment
    /// override it.
    pub port_mapping: bool,
    /// Persisted "serve the SPA over HTTP" setting (`app_server.rs`).
    pub app_server: bool,
}

pub fn save_peer(keypair: &Keypair, label: Option<&str>) -> Option<PathBuf> {
    let peer_id = keypair.peer_id().to_string();
    let name = synthesize_name(&peer_id, label);
    let dir = peers_dir().join(&name);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::error!("Failed to create peer dir for {}: {}", peer_id, e);
        return None;
    }
    if let Err(e) = keypair.save_to_file(&dir.join("keypair")) {
        log::error!("Failed to save keypair for {}: {}", peer_id, e);
        return None;
    }
    write_default_config(&dir, label);
    log::info!("Backend peer {} saved to {:?}", &peer_id[..12.min(peer_id.len())], dir);
    Some(dir.join("store.db"))
}

/// The `managed_by` value Tori stamps on identities it creates.
const MANAGED_BY_TAURI: &str = "tauri";

/// Does this directory hold an identity **Tori manages**, as opposed to one
/// that merely lives on this machine?
///
/// The distinction is the whole point of this predicate and it is not
/// cosmetic. `~/.entity/peers/` is shared: on a developer box it accumulates
/// conformance-harness identities, CLI peers and sibling-repo fixtures, and
/// every one of them is a valid keypair in the spec layout. Adopting them all
/// (which is what this module did until 2026-08-21) means Tori holds ~1500
/// foreign secret keys in memory and offers a Start button that would create a
/// `store.db` inside another tool's fixture directory and run a peer under a
/// key Tori never issued.
///
/// **`config.toml` presence is NOT the discriminator** — measured on a real
/// box, 317 of 1514 directories carried one (the CLI writes them too) while
/// exactly 3 carried `managed_by = "tauri"`, and those 3 were precisely the
/// peers the app already knew about.
fn is_tauri_managed(cfg: &PeerConfigFile) -> bool {
    cfg.managed_by.as_deref() == Some(MANAGED_BY_TAURI)
}

/// An identity present in the machine-wide store that Tori does **not**
/// manage. Carries no keypair, deliberately: the point of the split is that
/// these are listed, never adopted, so the secret never enters our process.
pub struct UnmanagedIdentity {
    /// The directory name under `peers/` — what the owning tool called it,
    /// and the only human-meaningful label we have for it.
    pub name: String,
    pub peer_id: String,
}

/// Load the backend peers **Tori manages**. Foreign identities sharing the
/// store are skipped — see [`is_tauri_managed`] — and reported as a count so a
/// developer box with a large shared store says so once in the log rather than
/// silently producing a short list.
pub fn load_all_peers() -> Vec<LoadedBackendPeer> {
    migrate_legacy_layout_if_needed();

    let dir = peers_dir();
    let mut peers = Vec::new();
    let mut skipped = 0usize;
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return peers,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // Read the config BEFORE the keypair: an unmanaged identity's secret
        // key must never be loaded into this process at all, so the cheap
        // ownership check has to come first.
        let cfg = read_config(&path);
        if !is_tauri_managed(&cfg) {
            skipped += 1;
            continue;
        }
        let kp_path = path.join("keypair");
        let keypair = match Keypair::load_from_file(&kp_path) {
            Ok(kp) => kp,
            Err(e) => {
                log::warn!("Skipping peer dir {:?} without valid keypair: {}", path, e);
                continue;
            }
        };
        let sqlite_path = if cfg.storage_backend == "sqlite" {
            Some(path.join("store.db"))
        } else {
            None
        };
        peers.push(LoadedBackendPeer {
            peer_id: keypair.peer_id().to_string(),
            keypair,
            label: cfg.label,
            sqlite_path,
            signaling_node: cfg.signaling_node,
            port_mapping: cfg.port_mapping,
            app_server: cfg.app_server,
        });
    }
    if skipped > 0 {
        log::info!(
            "Loaded {} managed backend peer(s) from {:?}; {} other identit{} in this \
             shared store are not managed by Tori and were not adopted",
            peers.len(),
            dir,
            skipped,
            if skipped == 1 { "y" } else { "ies" },
        );
    } else {
        log::info!("Loaded {} backend peer(s) from {:?}", peers.len(), dir);
    }
    peers
}

/// How many identities in the shared store are not ours — **without parsing a
/// single keypair**.
///
/// The cheap counterpart to [`list_unmanaged_identities`], for the one-line
/// startup report. On a developer box the difference is 1500 Ed25519 parses
/// against 1500 small file reads, and the report does not need the ids.
pub fn count_unmanaged_identities() -> usize {
    let Ok(entries) = std::fs::read_dir(peers_dir()) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| {
            let p = e.path();
            p.is_dir() && !is_tauri_managed(&read_config(&p))
        })
        .count()
}

/// Enumerate the identities in the shared store that Tori does not manage.
///
/// **On demand, never at startup.** Listing them is reasonable — they really
/// are peer identities on this computer — but it costs a keypair parse each,
/// and the startup path has no reason to pay it. The keypair is dropped as
/// soon as the peer-id is derived from it.
pub fn list_unmanaged_identities() -> Vec<UnmanagedIdentity> {
    let dir = peers_dir();
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || is_tauri_managed(&read_config(&path)) {
            continue;
        }
        let Ok(kp) = Keypair::load_from_file(&path.join("keypair")) else {
            continue;
        };
        out.push(UnmanagedIdentity {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            peer_id: kp.peer_id().to_string(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn delete_peer(peer_id: &str) {
    if let Some(dir) = find_managed_peer_dir_by_id(peer_id) {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            log::warn!("Failed to remove peer dir {:?}: {}", dir, e);
        } else {
            log::info!("Deleted backend peer at {:?}", dir);
        }
    }
}

/// Idempotent migration of the legacy `backend-peers/{peer_id}` layout
/// into `peers/{name}/{keypair, config.toml}`. Runs once per startup
/// before `load_all_peers`. Skips peers that already exist in the new
/// layout (matched by peer-id).
fn migrate_legacy_layout_if_needed() {
    let legacy = legacy_backend_peers_dir();
    if !legacy.exists() {
        return;
    }
    let entries = match std::fs::read_dir(&legacy) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut migrated = 0usize;
    for entry in entries.flatten() {
        let pem_path = entry.path();
        if let Some(ext) = pem_path.extension() {
            if ext == "pub" || ext == "meta" {
                continue;
            }
        }
        if !pem_path.is_file() {
            continue;
        }
        let keypair = match Keypair::load_from_file(&pem_path) {
            Ok(kp) => kp,
            Err(_) => continue,
        };
        let pid = keypair.peer_id().to_string();
        if find_peer_dir_by_id(&pid).is_some() {
            continue;
        }
        let label = std::fs::read_to_string(legacy.join(format!("{}.meta", pid)))
            .ok()
            .filter(|s| !s.is_empty());
        let name = synthesize_name(&pid, label.as_deref());
        let target_dir = peers_dir().join(&name);
        if let Err(e) = std::fs::create_dir_all(&target_dir) {
            log::warn!("migrate: mkdir {:?} failed: {}", target_dir, e);
            continue;
        }
        if let Err(e) = keypair.save_to_file(&target_dir.join("keypair")) {
            log::warn!("migrate: save keypair for {} failed: {}", pid, e);
            continue;
        }
        write_default_config(&target_dir, label.as_deref());
        migrated += 1;
    }
    if migrated > 0 {
        log::info!("Migrated {} backend peer(s) from legacy layout", migrated);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn save_load_round_trip() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("ENTITY_DATA_DIR", tmp.path());

        let kp = Keypair::generate();
        let pid = kp.peer_id().to_string();
        let saved = save_peer(&kp, Some("Backend One"));
        assert!(saved.is_some(), "save_peer should return sqlite path");

        let loaded = load_all_peers();
        assert_eq!(loaded.len(), 1);
        let p = &loaded[0];
        assert_eq!(p.peer_id, pid);
        assert_eq!(p.label.as_deref(), Some("Backend One"));
        assert!(p.sqlite_path.as_ref().unwrap().ends_with("store.db"));

        std::env::remove_var("ENTITY_DATA_DIR");
    }

    #[test]
    fn delete_removes_peer_directory() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("ENTITY_DATA_DIR", tmp.path());

        let kp = Keypair::generate();
        let pid = kp.peer_id().to_string();
        save_peer(&kp, None);
        assert_eq!(load_all_peers().len(), 1);

        delete_peer(&pid);
        assert_eq!(load_all_peers().len(), 0);

        std::env::remove_var("ENTITY_DATA_DIR");
    }

    #[test]
    fn migrates_legacy_backend_peers_layout() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("ENTITY_DATA_DIR", tmp.path());

        // Seed legacy backend-peers layout.
        let legacy_dir = tmp.path().join("backend-peers");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        let kp = Keypair::generate();
        let pid = kp.peer_id().to_string();
        kp.save_to_file(&legacy_dir.join(&pid)).unwrap();
        std::fs::write(legacy_dir.join(format!("{}.meta", pid)), "Legacy Backend").unwrap();

        // load_all_peers triggers migration.
        let peers = load_all_peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].peer_id, pid);
        assert_eq!(peers[0].label.as_deref(), Some("Legacy Backend"));
        assert!(peers[0].sqlite_path.is_some());

        // Idempotent.
        assert_eq!(load_all_peers().len(), 1);

        std::env::remove_var("ENTITY_DATA_DIR");
    }

    /// Seed a foreign identity the way the shared store really acquires them:
    /// a valid keypair in the spec layout, written by some other tool.
    /// `config` is what that tool left behind — `None` for the conformance
    /// harness (keypair only), or a `config.toml` with no `managed_by`, which
    /// is what the CLI writes.
    fn seed_foreign_identity(root: &Path, name: &str, config: Option<&str>) -> String {
        let dir = root.join("peers").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let kp = Keypair::generate();
        kp.save_to_file(&dir.join("keypair")).unwrap();
        if let Some(body) = config {
            std::fs::write(dir.join("config.toml"), body).unwrap();
        }
        kp.peer_id().to_string()
    }

    /// `~/.entity/peers/` is the machine-wide identity store, not Tori's
    /// private one. Both shapes of foreign identity must be **listed** (they
    /// are genuinely peer identities on this computer) and **never adopted**
    /// — adoption means holding a foreign secret key and offering a Start
    /// button that would create a `store.db` in another tool's directory.
    ///
    /// Note what this test does NOT assert on: the presence of `config.toml`.
    /// That was the tempting discriminator and it is wrong — measured on a
    /// real box, 317 of 1514 foreign directories carried one.
    #[test]
    fn an_identity_this_app_did_not_create_is_listed_but_never_adopted() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("ENTITY_DATA_DIR", tmp.path());

        let ours = Keypair::generate();
        let our_pid = ours.peer_id().to_string();
        save_peer(&ours, Some("Ours"));

        // The conformance harness: keypair, nothing else.
        let bare = seed_foreign_identity(tmp.path(), "vcref-c1002544", None);
        // The CLI: a config.toml in the spec layout, but not ours.
        let cli = seed_foreign_identity(
            tmp.path(),
            "test-rust",
            Some("storage_backend = \"sqlite\"\nlabel = \"CLI peer\"\n"),
        );

        let adopted = load_all_peers();
        assert_eq!(
            adopted.iter().map(|p| p.peer_id.clone()).collect::<Vec<_>>(),
            vec![our_pid.clone()],
            "only the identity Tori created may be adopted",
        );

        let present: Vec<String> =
            list_unmanaged_identities().into_iter().map(|i| i.peer_id).collect();
        assert!(present.contains(&bare), "a bare foreign identity must still be listed");
        assert!(present.contains(&cli), "a CLI-written identity must still be listed");
        assert!(
            !present.contains(&our_pid),
            "our own peer is managed and must not also appear as unmanaged",
        );

        std::env::remove_var("ENTITY_DATA_DIR");
    }

    /// A mutation addressed by peer-id alone would reach into the shared store.
    /// Not reachable from the UI today; the guard sits at the lookup so it does
    /// not depend on that staying true.
    #[test]
    fn a_foreign_identity_cannot_be_deleted_or_reconfigured_by_id() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("ENTITY_DATA_DIR", tmp.path());

        let pid = seed_foreign_identity(tmp.path(), "someone-elses", None);
        let dir = tmp.path().join("peers").join("someone-elses");

        delete_peer(&pid);
        assert!(dir.join("keypair").exists(), "delete must not touch a foreign identity");

        assert!(!set_signaling_node(&pid, true), "a foreign identity has no togglable config");
        assert!(
            !dir.join("config.toml").exists(),
            "and the refused toggle must not have written one",
        );

        std::env::remove_var("ENTITY_DATA_DIR");
    }

    #[test]
    fn save_peer_returns_sqlite_path_under_peer_dir() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("ENTITY_DATA_DIR", tmp.path());

        let kp = Keypair::generate();
        let path = save_peer(&kp, Some("X")).expect("save returns sqlite path");
        assert!(path.ends_with("store.db"));
        assert!(path.starts_with(tmp.path()));

        std::env::remove_var("ENTITY_DATA_DIR");
    }
}
