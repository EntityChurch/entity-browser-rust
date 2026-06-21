//! Persistence — platform-specific peer storage (browser app layer).
//!
//! Native (spec layout per `GUIDE-PERSISTENCE.md` §1):
//! ```text
//! $ENTITY_DATA_DIR or ~/.entity/peers/{name}/
//!   ├── keypair        ← PEM, restored at startup
//!   ├── config.toml    ← storage_backend, label
//!   └── store.db       ← SQLite tree (when storage_backend = "sqlite")
//! ```
//! WASM: this module persists the **identity (keypair)** as localStorage JSON
//! with hex-encoded seeds. The **tree** IS durably persisted in **Worker mode**
//! (the default) — the Worker's OPFS journal at `workers/{peer_id}/`,
//! flush-on-write, owned upstream (`wasm-worker-host`); this module does not
//! touch it. In **Direct mode** (the auto-fallback) the tree is in-memory only.
//! (Stale "no tree persistence on the web — sister-doc C1" note corrected,
//! F-BOOT-7; see the browser-storage-substrate research.)
//!
//! Apps own persistence I/O. The SDK accepts the loaded set via
//! `Peers::load_persisted`; new peers go through `save_peer`
//! (which synthesizes a directory name from the label or peer-id).

use entity_crypto::Keypair;

use entity_sdk::peer_manager::PersistedPeer;

use crate::peer_mode::PeerMode;

/// App-tier wrapper bundling a `PersistedPeer` with the user-facing
/// mode the peer was created in. The SDK type (`PersistedPeer`)
/// doesn't carry a mode because hosting model is an app concern;
/// modes determine which SDK the peer is loaded into at boot.
///
/// Default mode is `PeerMode::Frontend` for legacy persisted entries
/// that pre-date this field.
pub struct PersistedPeerEntry {
    pub persisted: PersistedPeer,
    pub mode: PeerMode,
}

// ---------------------------------------------------------------------------
// Native persistence (filesystem) — spec layout
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use std::path::{Path, PathBuf};

    /// Resolve the configuration root. Honors `ENTITY_DATA_DIR` so
    /// tests and ops can redirect without touching `$HOME`.
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

    fn legacy_keys_dir() -> PathBuf {
        data_root().join("keys")
    }

    /// Sanitize a label or peer-id prefix into a filesystem-safe alias.
    /// Lowercases ASCII alnum/`-`/`_`, drops everything else, caps at 32.
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

    /// Synthesize a unique directory name. `name = sanitize(label).unwrap_or(peer_id_prefix)`,
    /// suffixed with `-N` if a peer dir already exists for a different peer-id.
    fn synthesize_name(peer_id: &str, label: Option<&str>) -> String {
        let base = match label {
            Some(s) if !s.is_empty() => sanitize_name(s),
            _ => sanitize_name(&peer_id[..peer_id.len().min(8)]),
        };
        let dir = peers_dir();
        // Bare base is fine if the dir doesn't exist or already houses this peer.
        if name_matches_or_free(&dir, &base, peer_id) {
            return base;
        }
        for i in 2.. {
            let candidate = format!("{}-{}", base, i);
            if name_matches_or_free(&dir, &candidate, peer_id) {
                return candidate;
            }
            if i > 100 {
                // Defensive cap — extremely unlikely to fire.
                return format!("{}-{}", base, peer_id);
            }
        }
        unreachable!()
    }

    /// True if `peers/{name}/` is either absent or already holds a
    /// keypair belonging to `expected_peer_id` (same peer, OK to reuse).
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

    /// Find an existing peers/{name}/ directory for the given peer-id.
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

    fn write_config(dir: &Path, label: Option<&str>, mode: PeerMode) {
        let body = format!(
            "# entity-browser peer configuration\n\
             # Spec: GUIDE-PERSISTENCE.md §1\n\
             storage_backend = \"sqlite\"\n\
             mode = \"{}\"\n\
             {}\n",
            mode.persist_key(),
            label.map(|l| format!("label = \"{}\"", l.replace('"', "\\\""))).unwrap_or_default(),
        );
        let _ = std::fs::write(dir.join("config.toml"), body);
    }

    /// Parsed `config.toml` — only the fields this app reads today.
    /// Unknown keys are ignored so future spec additions don't break us.
    struct PeerConfigFile {
        storage_backend: String,
        label: Option<String>,
        mode: PeerMode,
    }

    impl Default for PeerConfigFile {
        fn default() -> Self {
            Self {
                storage_backend: "sqlite".into(),
                label: None,
                mode: PeerMode::Frontend,
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
                tracing::warn!(dir = ?dir, error = %e, "config.toml: parse failed; using defaults");
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
            mode: table
                .get("mode")
                .and_then(|v| v.as_str())
                .and_then(PeerMode::from_persist_key)
                .unwrap_or(PeerMode::Frontend),
        }
    }

    /// Default-mode save (Frontend). Kept for callers that don't care
    /// about peer hosting mode (e.g. Tauri backend peer registration).
    pub fn save_peer(peer_id: &str, keypair: &Keypair, label: Option<&str>) {
        save_peer_with_mode(peer_id, keypair, label, PeerMode::Frontend);
    }

    pub fn save_peer_with_mode(
        peer_id_arg: &str,
        keypair: &Keypair,
        label: Option<&str>,
        mode: PeerMode,
    ) {
        let peer_id = keypair.peer_id().to_string();
        // F-BOOT-8: the keypair is authoritative, but the passed id is now a
        // *checked* invariant rather than silently ignored — a mismatch is a
        // caller bug (the WASM twin uses its param, so the contract aligns).
        debug_assert_eq!(
            peer_id_arg, peer_id,
            "save_peer_with_mode: passed peer_id must match the keypair-derived id"
        );
        let name = synthesize_name(&peer_id, label);
        let dir = peers_dir().join(&name);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::error!(peer_id = %peer_id, error = %e, "failed to create peer dir");
            return;
        }
        if let Err(e) = keypair.save_to_file(&dir.join("keypair")) {
            tracing::error!(peer_id = %peer_id, error = %e, "failed to save keypair");
            return;
        }
        write_config(&dir, label, mode);
        tracing::info!(peer_id = %peer_id, name = %name, mode = %mode.persist_key(), "peer saved to {:?}", dir);
    }

    pub fn load_all_peers() -> Vec<PersistedPeer> {
        load_all_peer_entries()
            .into_iter()
            .map(|e| e.persisted)
            .collect()
    }

    pub fn load_all_peer_entries() -> Vec<PersistedPeerEntry> {
        // One-shot migration: copy any `keys/{peer_id}.pem` files into
        // the new spec layout before reading.
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
                    tracing::warn!(path = ?path, error = %e, "skipping peer dir without valid keypair");
                    continue;
                }
            };
            let cfg = read_config(&path);
            let sqlite_path = if cfg.storage_backend == "sqlite" {
                Some(path.join("store.db"))
            } else {
                None
            };
            peers.push(PersistedPeerEntry {
                persisted: PersistedPeer {
                    keypair,
                    label: cfg.label,
                    sqlite_path,
                },
                mode: cfg.mode,
            });
        }
        tracing::info!(count = peers.len(), "loaded persisted peer entries from {:?}", dir);
        peers
    }

    pub fn delete_peer(peer_id: &str) {
        if let Some(dir) = find_peer_dir_by_id(peer_id) {
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                tracing::warn!(peer_id = %peer_id, error = %e, "failed to remove peer dir");
            } else {
                tracing::info!(peer_id = %peer_id, "deleted persisted peer at {:?}", dir);
            }
        } else {
            tracing::debug!(peer_id = %peer_id, "delete_peer: no dir found (already gone?)");
        }
    }

    /// Idempotent migration of the old `keys/{peer_id}.pem(.meta)` layout
    /// into `peers/{name}/{keypair, config.toml}`. Runs once per startup
    /// before `load_all_peers`. Skips peers that already exist in the
    /// new layout (matched by peer-id).
    fn migrate_legacy_layout_if_needed() {
        let legacy = legacy_keys_dir();
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
            // Already migrated? Skip.
            if find_peer_dir_by_id(&pid).is_some() {
                continue;
            }
            // Pull legacy label sidecar.
            let label = std::fs::read_to_string(legacy.join(format!("{}.meta", pid)))
                .ok()
                .filter(|s| !s.is_empty());
            let name = synthesize_name(&pid, label.as_deref());
            let target_dir = peers_dir().join(&name);
            if let Err(e) = std::fs::create_dir_all(&target_dir) {
                tracing::warn!(peer_id = %pid, error = %e, "migrate: mkdir failed");
                continue;
            }
            if let Err(e) = keypair.save_to_file(&target_dir.join("keypair")) {
                tracing::warn!(peer_id = %pid, error = %e, "migrate: save keypair failed");
                continue;
            }
            write_config(&target_dir, label.as_deref(), PeerMode::Frontend);
            migrated += 1;
        }
        if migrated > 0 {
            tracing::info!(count = migrated, "migrated peers from legacy layout");
        }
    }
}

// ---------------------------------------------------------------------------
// WASM persistence (localStorage)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;

    const STORAGE_KEY: &str = "entity_peers";
    /// Newline-separated peer-ids whose `workers/{peer_id}/` OPFS
    /// subdirectory still needs deletion. Populated when a Backend(OPFS)
    /// peer is deleted at runtime (the worker holds sync access handles
    /// so we can't remove the dir immediately); drained at next boot
    /// before any worker grabs OPFS handles. See `opfs_cleanup::run_at_boot`.
    const OPFS_TOMBSTONE_KEY: &str = "entity_opfs_tombstones";
    /// Hex-encoded 32-byte seed of the durable **main-thread system peer**
    /// (the IDB-backed Direct/Tauri primary). Distinct from the
    /// `entity_peers` spawn-list: this is the stable identity of the
    /// always-present main-thread peer, generated + persisted on first
    /// boot so the same peer-id — and therefore the same IndexedDB
    /// database — maps across reloads. The embryo of the persistent system
    /// peer (per the persistent-system-peer + durability-substrate design).
    const SYSTEM_SEED_KEY: &str = "entity_system_seed";

    fn get_storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok()?
    }

    /// The stable seed for the durable main-thread system peer. Reads the
    /// persisted `entity_system_seed`; on first boot (or if unreadable)
    /// generates a fresh seed and persists it. The returned seed is fed to
    /// [`Keypair::from_seed`] to construct the IDB-backed primary, and the
    /// derived peer-id keys the IndexedDB database name — both stable
    /// across reloads, which is what makes the Direct arm durable.
    ///
    /// Returns `(seed, was_persisted)`. `was_persisted` is `true` when a
    /// seed already existed in localStorage before this boot (a *warm*
    /// identity) and `false` on a first/cold boot or when storage is
    /// unavailable — the boot classifier uses it to decide whether to seed
    /// defaults (cold) or respect an existing durable tree (warm).
    ///
    /// Always returns a seed; if localStorage is unavailable the seed is
    /// fresh-per-session (the IDB store itself will then be the only
    /// durability surface, and an unavailable-storage path is already
    /// surfaced as ephemeral by the boot honesty banner).
    pub fn system_seed() -> ([u8; 32], bool) {
        let storage = get_storage();
        if let Some(storage) = storage.as_ref() {
            if let Ok(Some(hex)) = storage.get_item(SYSTEM_SEED_KEY) {
                if let Some(seed) = vault_codec::hex_to_seed(&hex) {
                    return (seed, true);
                }
            }
        }
        // First boot (or unreadable / malformed): generate + persist so
        // the next boot reconstructs the same identity.
        let seed = Keypair::generate().secret_key_bytes();
        if let Some(storage) = storage.as_ref() {
            if let Err(e) = storage.set_item(SYSTEM_SEED_KEY, &vault_codec::seed_to_hex(&seed)) {
                tracing::warn!(?e, "failed to persist system seed; primary identity will be ephemeral this session");
            }
        }
        (seed, false)
    }

    /// The IDB-durable system peer's id, for the Direct/IDB multi-tab
    /// Web-Lock key. Derives from `entity_system_seed`, **generating +
    /// persisting it on a fresh profile** (via [`system_seed`]) so even tab 1's
    /// FIRST boot acquires the lock.
    ///
    /// A read-only variant would return `None` on a fresh profile, leaving
    /// tab 1 lock-less for its *entire* first session — and because IDB (unlike
    /// OPFS sync access handles) has **no exclusivity backstop**, a second tab
    /// opened in that window would then ALSO open the same db `entity-peer-{id}`
    /// and race (silent last-writer-wins corruption). Generating here closes
    /// that window: tab 1 acquires the lock immediately, so any later tab finds
    /// it held → secondary.
    ///
    /// Idempotent with `system_seed()` / `new_wasm`, which read the same
    /// persisted seed. The derived id matches the IDB db name `entity-peer-{id}`
    /// (see `app::EntityApp::new_wasm`), so the lock protects exactly the
    /// contended database. Computable synchronously (localStorage) before IDB
    /// opens → "elect-then-open". (If localStorage is unavailable, `system_seed`
    /// yields a fresh per-session seed → a unique id/db per tab → no contention
    /// to guard anyway; the lock-key/db-name then differ within one boot, a
    /// benign degradation folded into the Phase-2 seed-threading cleanup.)
    pub fn system_seed_id() -> String {
        let (seed, _was_persisted) = system_seed();
        Keypair::from_seed(seed).peer_id().to_string()
    }

    /// localStorage flag marking the one-time roster backfill (set A →
    /// the authoritative `system/roster/` tree) as done. A flag — NOT a
    /// tree read — is the gate precisely because the Worker sync mirror
    /// returns the (unwatched) roster prefix as silently empty
    /// (`feedback_worker_cache_get_needs_subscription`), so an "is the
    /// roster empty?" tree read would re-trigger the bulk backfill every
    /// boot, violating model invariant 2 (no bulk re-save). Only set after
    /// a successful durable flush on a durable arm (see `boot_load`).
    ///
    /// **Keyed per system-peer-id** (`entity_roster_migrated:{sys}`). The
    /// roster lives on the *system peer*, whose id DIFFERS by arm today
    /// (Shape 1: Direct/IDB uses the `entity_system_seed` id; the Worker arm
    /// uses the set-A primary id) — so each arm has its own roster on its own
    /// prefix. A single global flag let a Direct boot suppress the Worker
    /// arm's never-run backfill, leaving the Worker roster empty-but-"migrated"
    /// → a permanent false `reconcile DRIFT` (every set-A peer reported
    /// missing). Per-arm keying makes each arm back-fill + reconcile its own
    /// roster independently. (The legacy global `entity_roster_migrated` key is
    /// left orphaned; a one-time re-backfill on the new key is idempotent —
    /// content-addressed re-puts.)
    const ROSTER_MIGRATED_KEY: &str = "entity_roster_migrated";

    fn roster_migrated_key(system_peer_id: &str) -> String {
        format!("{ROSTER_MIGRATED_KEY}:{system_peer_id}")
    }

    pub fn roster_migration_done(system_peer_id: &str) -> bool {
        let key = roster_migrated_key(system_peer_id);
        get_storage()
            .and_then(|s| s.get_item(&key).ok().flatten())
            .as_deref()
            == Some("1")
    }

    pub fn mark_roster_migration_done(system_peer_id: &str) {
        let key = roster_migrated_key(system_peer_id);
        if let Some(storage) = get_storage() {
            if let Err(e) = storage.set_item(&key, "1") {
                tracing::warn!(?e, "failed to persist roster-migrated flag");
            }
        }
    }

    // The 32-byte-seed hex codec and the `entity_peers` vault (de)serializer —
    // including its schema-version marker — now live in `crate::vault_codec`
    // (native-testable; the browser localStorage I/O below stays wasm-gated).
    // MIGRATION INVARIANT (MAP §8): the vault format is versioned — changing it
    // is a migration (bump `vault_codec::VAULT_VERSION` + add a parse branch),
    // never a silent edit.
    use crate::vault_codec::{self, VaultEntry};

    /// Save (or update) a peer entry. The first call site of a new
    /// peer should use [`save_peer_with_mode`]; this entrypoint exists
    /// so legacy code paths that don't know about modes (Tauri backend
    /// peer registration, etc.) keep working — defaults to Frontend.
    pub fn save_peer(peer_id: &str, keypair: &Keypair, label: Option<&str>) {
        save_peer_with_mode(peer_id, keypair, label, PeerMode::Frontend);
    }

    pub fn save_peer_with_mode(
        peer_id: &str,
        keypair: &Keypair,
        label: Option<&str>,
        mode: PeerMode,
    ) {
        let Some(storage) = get_storage() else { return };
        let existing = storage.get_item(STORAGE_KEY).ok().flatten().unwrap_or_default();
        let (_version, mut entries) = vault_codec::parse(&existing);
        // Remove existing entry for this peer_id (update). If the peer
        // already had a different mode persisted, this overrides it —
        // we never want two records for the same peer.
        entries.retain(|e| e.peer_id != peer_id);
        entries.push(VaultEntry {
            peer_id: peer_id.to_string(),
            seed: keypair.secret_key_bytes(),
            label: label.map(String::from),
            mode,
        });
        storage.set_item(STORAGE_KEY, &vault_codec::serialize(&entries)).ok();
        tracing::info!(peer_id = %peer_id, mode = %mode.persist_key(), "keypair saved to localStorage");
    }

    /// Legacy loader — returns `PersistedPeer` only, dropping mode
    /// info. Kept for parity with the native module's public surface;
    /// WASM bootstrap paths now use [`load_all_peer_entries`] so they
    /// can dispatch per-peer mode. `dead_code` allowed because nothing
    /// in WASM calls it today; the symbol is part of the cross-target
    /// API surface.
    #[allow(dead_code)]
    pub fn load_all_peers() -> Vec<PersistedPeer> {
        load_all_peer_entries()
            .into_iter()
            .map(|e| e.persisted)
            .collect()
    }

    pub fn load_all_peer_entries() -> Vec<PersistedPeerEntry> {
        let Some(storage) = get_storage() else { return Vec::new() };
        let data = storage.get_item(STORAGE_KEY).ok().flatten().unwrap_or_default();
        let (_version, entries) = vault_codec::parse(&data);
        let result: Vec<_> = entries.into_iter()
            .map(|e| PersistedPeerEntry {
                persisted: PersistedPeer {
                    keypair: Keypair::from_seed(e.seed),
                    label: e.label,
                    sqlite_path: None,
                },
                mode: e.mode,
            })
            .collect();
        // DEBUG: this is called by `peer_modes()` from every section
        // rebuild (badges, palette signature) — at INFO it floods the
        // log during normal interaction.
        tracing::debug!(count = result.len(), "loaded persisted peer entries from localStorage");
        result
    }

    // `primary_peer_id_for_boot` was the old per-arm multi-tab key (the
    // first Frontend entry's seed-derived id). The election now keys on the
    // system-seed id uniformly across both arms (`system_seed_id`, design
    // §9 step 2 / main.rs), so it was removed to avoid a second,
    // drift-prone key. The seed-derived-id discipline it carried lives on in
    // `system_seed_id` and `delete_peer` below.

    pub fn delete_peer(peer_id: &str) {
        let Some(storage) = get_storage() else { return };
        let existing = storage.get_item(STORAGE_KEY).ok().flatten().unwrap_or_default();
        let (_version, mut entries) = vault_codec::parse(&existing);
        let before = entries.len();
        // Match on the AUTHORITATIVE identity — the seed-derived peer_id —
        // not the stored `peer_id` field. Everything downstream
        // (`load_all_peer_entries`, the boot worker's InitParams, the Peers
        // window rows, hence the `peer_id` handed to this function) keys a
        // peer by `Keypair::from_seed(seed).peer_id()`, NOT by the stored
        // field. If the two ever disagree (identity/format drift across
        // app versions, or a record written before the field matched the
        // seed), a field-only match silently removes NOTHING — Delete logs
        // success, the row vanishes via the registry self-heal, and the
        // peer RESURRECTS on the next boot (it was never removed from
        // localStorage). That is the "delete doesn't stick" failure. Match
        // the derived id first, keep the field as a defensive fallback.
        entries.retain(|e| {
            let derived = Keypair::from_seed(e.seed).peer_id().to_string();
            derived != peer_id && e.peer_id != peer_id
        });
        let removed = before - entries.len();
        storage.set_item(STORAGE_KEY, &vault_codec::serialize(&entries)).ok();
        if removed == 0 {
            // Surface a no-op delete (D13 silence-is-the-enemy): a click
            // that removes nothing is a bug we want loud, not swallowed.
            tracing::warn!(
                peer_id = %peer_id,
                "delete_peer: NO localStorage entry matched (neither derived id \
                 nor stored field) — peer will resurrect on reload"
            );
        } else {
            tracing::info!(peer_id = %peer_id, removed, "deleted persisted keypair from localStorage");
        }
    }

    /// Mark a peer's OPFS subdirectory for cleanup at the next boot.
    /// We can't remove the dir immediately when the user clicks Delete
    /// because the peer's dedicated worker is still alive holding sync
    /// access handles on the journal files; the upstream
    /// `WorkerProxy` doesn't expose `terminate()`. Boot-time cleanup
    /// runs before any worker grabs handles, so it's race-free.
    pub fn mark_opfs_for_cleanup(peer_id: &str) {
        let Some(storage) = get_storage() else { return };
        let existing = storage
            .get_item(OPFS_TOMBSTONE_KEY)
            .ok()
            .flatten()
            .unwrap_or_default();
        // De-dup: if the peer-id is already pending cleanup, don't add it again.
        if existing.lines().any(|l| l == peer_id) {
            return;
        }
        let next = if existing.is_empty() {
            peer_id.to_string()
        } else {
            format!("{existing}\n{peer_id}")
        };
        storage.set_item(OPFS_TOMBSTONE_KEY, &next).ok();
        tracing::info!(peer_id = %peer_id, "OPFS subdir marked for boot-time cleanup");
    }

    pub fn load_opfs_tombstones() -> Vec<String> {
        let Some(storage) = get_storage() else { return Vec::new() };
        let data = storage
            .get_item(OPFS_TOMBSTONE_KEY)
            .ok()
            .flatten()
            .unwrap_or_default();
        data.lines()
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    }

    /// Replace the tombstone set. Pass an empty slice to clear it after
    /// a fully-successful cleanup pass, or a smaller slice if some
    /// removals failed and should be retried on a later boot.
    pub fn set_opfs_tombstones(remaining: &[String]) {
        let Some(storage) = get_storage() else { return };
        if remaining.is_empty() {
            storage.remove_item(OPFS_TOMBSTONE_KEY).ok();
        } else {
            storage.set_item(OPFS_TOMBSTONE_KEY, &remaining.join("\n")).ok();
        }
    }
}

// ---------------------------------------------------------------------------
// Public API — delegates to platform module
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
pub use native::{save_peer, save_peer_with_mode, load_all_peers, load_all_peer_entries, delete_peer};

#[cfg(target_arch = "wasm32")]
pub use wasm::{
    save_peer, save_peer_with_mode, load_all_peer_entries, delete_peer,
    mark_opfs_for_cleanup, load_opfs_tombstones, set_opfs_tombstones,
    system_seed, system_seed_id,
    roster_migration_done, mark_roster_migration_done,
};

/// Authoritative hosted-peer mode map (`peer_id` → [`PeerMode`]) from
/// the persisted store.
///
/// This is the *only* reliable runtime source for a backend peer's
/// memory-vs-OPFS mode: `PeerMetadata` doesn't carry it, and the old
/// code inferred it from the `persisted` flag — a flat
/// misrepresentation (a persisted backend-*memory* peer is not OPFS,
/// and a backend peer's `metadata.persisted` is never set true, so
/// every backend peer rendered as "memory"). Callers build this once
/// and resolve roles against it.
///
/// [`PeerMode`]: crate::peer_mode::PeerMode
pub fn peer_modes() -> std::collections::HashMap<String, crate::peer_mode::PeerMode> {
    load_all_peer_entries()
        .into_iter()
        .map(|e| (e.persisted.keypair.peer_id().to_string(), e.mode))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_peer_round_trip() {
        let kp = Keypair::generate();
        let seed = kp.secret_key_bytes();
        let pid = kp.peer_id().to_string();
        let restored = Keypair::from_seed(seed);
        assert_eq!(restored.peer_id().to_string(), pid);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_save_load_delete() {
        let dir = tempfile::tempdir().unwrap();
        let kp = Keypair::generate();
        let pid = kp.peer_id().to_string();
        let path = dir.path().join(&pid);

        kp.save_to_file(&path).unwrap();
        assert!(path.exists());

        let loaded = Keypair::load_from_file(&path).unwrap();
        assert_eq!(loaded.peer_id().to_string(), pid);

        std::fs::remove_file(&path).ok();
        assert!(!path.exists());
    }

    // -------------------------------------------------------------------
    // Spec-layout tests (native only). All filesystem-touching tests
    // serialize on ENV_LOCK so the ENTITY_DATA_DIR override doesn't race
    // across cargo's parallel test threads.
    // -------------------------------------------------------------------

    #[cfg(not(target_arch = "wasm32"))]
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn save_load_round_trip_in_spec_layout() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("ENTITY_DATA_DIR", tmp.path());

        let kp = Keypair::generate();
        let pid = kp.peer_id().to_string();
        save_peer(&pid, &kp, Some("My Peer"));

        let loaded = load_all_peers();
        assert_eq!(loaded.len(), 1);
        let p = &loaded[0];
        assert_eq!(p.keypair.peer_id().to_string(), pid);
        assert_eq!(p.label.as_deref(), Some("My Peer"));
        // Spec layout writes config.toml with storage_backend=sqlite by
        // default, so sqlite_path is populated on load.
        let store_db = p.sqlite_path.as_ref().expect("sqlite_path expected");
        assert!(store_db.ends_with("store.db"));

        std::env::remove_var("ENTITY_DATA_DIR");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn save_with_mode_round_trip_in_spec_layout() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("ENTITY_DATA_DIR", tmp.path());

        let kp_a = Keypair::generate();
        let kp_b = Keypair::generate();
        let pid_a = kp_a.peer_id().to_string();
        let pid_b = kp_b.peer_id().to_string();
        save_peer_with_mode(&pid_a, &kp_a, Some("Front"), PeerMode::Frontend);
        save_peer_with_mode(&pid_b, &kp_b, Some("Back"), PeerMode::BackendOpfs);

        let entries = load_all_peer_entries();
        assert_eq!(entries.len(), 2);
        let by_id: std::collections::HashMap<_, _> = entries
            .iter()
            .map(|e| (e.persisted.keypair.peer_id().to_string(), e.mode))
            .collect();
        assert_eq!(by_id.get(&pid_a), Some(&PeerMode::Frontend));
        assert_eq!(by_id.get(&pid_b), Some(&PeerMode::BackendOpfs));

        std::env::remove_var("ENTITY_DATA_DIR");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn legacy_config_without_mode_defaults_to_frontend() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("ENTITY_DATA_DIR", tmp.path());

        // Write a peer dir with a pre-Stage-2C config.toml (no `mode`).
        let dir = tmp.path().join("peers").join("legacy");
        std::fs::create_dir_all(&dir).unwrap();
        let kp = Keypair::generate();
        kp.save_to_file(&dir.join("keypair")).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            "storage_backend = \"sqlite\"\nlabel = \"Legacy\"\n",
        )
        .unwrap();

        let entries = load_all_peer_entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].mode, PeerMode::Frontend);
        assert_eq!(entries[0].persisted.label.as_deref(), Some("Legacy"));

        std::env::remove_var("ENTITY_DATA_DIR");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn delete_removes_peer_directory() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("ENTITY_DATA_DIR", tmp.path());

        let kp = Keypair::generate();
        let pid = kp.peer_id().to_string();
        save_peer(&pid, &kp, None);
        assert_eq!(load_all_peers().len(), 1);

        delete_peer(&pid);
        assert_eq!(load_all_peers().len(), 0);

        std::env::remove_var("ENTITY_DATA_DIR");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn migrates_legacy_keys_layout() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("ENTITY_DATA_DIR", tmp.path());

        // Seed the legacy layout: keys/{peer_id}.pem (or just `peer_id`)
        // plus an optional .meta sidecar — matches what the pre-migration
        // save_peer wrote.
        let legacy_dir = tmp.path().join("keys");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        let kp = Keypair::generate();
        let pid = kp.peer_id().to_string();
        kp.save_to_file(&legacy_dir.join(&pid)).unwrap();
        std::fs::write(legacy_dir.join(format!("{}.meta", pid)), "Legacy Peer").unwrap();

        // load_all_peers triggers the migration.
        let peers = load_all_peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].keypair.peer_id().to_string(), pid);
        assert_eq!(peers[0].label.as_deref(), Some("Legacy Peer"));
        assert!(peers[0].sqlite_path.is_some(), "migrated peer defaults to sqlite backend");

        // Re-running is idempotent (no duplicate dirs).
        assert_eq!(load_all_peers().len(), 1);

        std::env::remove_var("ENTITY_DATA_DIR");
    }
}
