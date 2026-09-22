//! Read-only storage-overview output — the data the DOM renderer consumes.
//!
//! Pure value types built by [`super::model::StorageModel`] from the live
//! stores; no behaviour, so they unit-test without WASM.

/// One bucket of live paths, keyed by the top-level tree segment they fall
/// under (`app`, `apps`, `sites`, `system`, …) — "where the paths live".
pub struct PrefixCount {
    pub label: String,
    pub count: usize,
    /// Bytes of the entities bound under this segment — their own data, not
    /// the blobs a file entity points at (those are counted by file, below).
    pub bytes: u64,
}

/// One File Manager place on this peer: how many files, and the bytes they
/// declare. Answers *what is using my space* in the words the File Manager
/// uses, where a person can act on it.
pub struct PlaceUsage {
    pub place: crate::file_kinds::Place,
    pub files: usize,
    pub bytes: u64,
}

/// One hosted peer's storage stats.
pub struct PeerStorage {
    pub peer_id: String,
    /// True for Worker/OPFS-hosted backend peers; false for the Direct/IDB
    /// main-thread peer(s). Drives the arm label + the breakdown caveat.
    pub is_backend: bool,
    /// Total blobs in the content store (content-addressed). **Includes
    /// superseded / orphaned values** — this is the number that grows
    /// unbounded under save-state churn (the append-only store never reaps).
    pub content_blobs: usize,
    /// Live paths in the location index — the "size of the current tree".
    pub live_paths: usize,
    /// Per-top-level-segment breakdown of the live paths.
    pub buckets: Vec<PrefixCount>,
    /// Bytes of every live entity's own data (the sum of the buckets).
    pub live_bytes: u64,
    /// The peer's files, by File Manager place (places holding none omitted).
    pub files: Vec<PlaceUsage>,
    /// Live save-state paths under `app/entity-browser/apps/*/state/` — the
    /// design's headline churn source, called out on its own.
    pub save_state_paths: usize,
}

impl PeerStorage {
    /// Approximate count of orphaned / superseded blobs: content the store
    /// holds that no live path points at. **A signal, not exact** — dedup
    /// (shared bytes) and ref-graph reachability mean the true reachable set
    /// can differ; see GUIDE-GC §2. Still the clearest "is bloat
    /// accumulating?" number we can compute app-side, O(1).
    pub fn approx_orphans(&self) -> usize {
        self.content_blobs.saturating_sub(self.live_paths)
    }
}

/// Origin-level disk estimate (`navigator.storage.estimate()`). Shared by
/// **all** peers + IndexedDB + caches on this origin — NOT per-peer.
#[derive(Clone, Copy, Default)]
pub struct OriginEstimate {
    pub usage_bytes: f64,
    pub quota_bytes: f64,
    /// `navigator.storage.persisted()` — `Some(true)` eviction-protected,
    /// `Some(false)` best-effort/evictable, `None` API unavailable.
    pub persisted: Option<bool>,
}

/// Why the browser gave no storage estimate. Kept apart because each sends a
/// person somewhere different — and because *"Loading…"* forever, which is what
/// both surfaces showed for all three, sends them nowhere (field report
/// 2026-09-14, a phone on the LAN link).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EstimateUnavailable {
    /// `navigator.storage` is `[SecureContext]`: a page served over plain
    /// http from another machine's address does not get it at all.
    InsecureContext,
    /// A secure page, and still no `navigator.storage.estimate`.
    NoApi,
    /// The call exists and did not answer with figures.
    Failed,
}

impl EstimateUnavailable {
    /// The sentence that explains it, localized.
    pub fn explanation(self) -> String {
        crate::i18n::t(
            match self {
                Self::InsecureContext => "storage.unavailable_insecure",
                Self::NoApi => "storage.unavailable_api",
                Self::Failed => "storage.unavailable_failed",
            },
            &[],
        )
    }
}

/// The whole window's render input.
pub struct StorageOutput {
    pub peers: Vec<PeerStorage>,
    /// Origin disk estimate, once the async probe has resolved.
    /// `None` while the probe is out; its answer, figures or the reason there
    /// are none, once it is back.
    pub estimate: Option<Result<OriginEstimate, EstimateUnavailable>>,
    /// The native system backend's store, once its IPC probe resolves (desktop
    /// only; `None` in a browser or before the first probe).
    pub backend: Option<BackendStoreView>,
}

/// The canonical **native** system backend's store, surfaced over IPC — it is a
/// remote peer over the connection pool, so it never appears in the per-peer
/// (local-arm) list above. On-disk size is available even when B is stopped;
/// the live counts are `None` then (no running store to read).
#[derive(Clone, Default)]
pub struct BackendStoreView {
    /// First 12 chars of B's peer id, for a compact heading.
    pub short_id: String,
    pub running: bool,
    /// SQLite file size on disk, in bytes.
    pub sqlite_bytes: Option<u64>,
    /// Live content-store blob count (running only).
    pub entity_count: Option<u64>,
    /// Live tree path count (running only).
    pub path_count: Option<u64>,
}
