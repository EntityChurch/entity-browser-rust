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
}

impl Default for PeerConfigFile {
    fn default() -> Self {
        Self {
            storage_backend: "sqlite".into(),
            label: None,
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

pub fn load_all_peers() -> Vec<LoadedBackendPeer> {
    migrate_legacy_layout_if_needed();

    let dir = peers_dir();
    let mut peers = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return peers,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
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
        let cfg = read_config(&path);
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
        });
    }
    log::info!("Loaded {} backend peer(s) from {:?}", peers.len(), dir);
    peers
}

pub fn delete_peer(peer_id: &str) {
    if let Some(dir) = find_peer_dir_by_id(peer_id) {
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
