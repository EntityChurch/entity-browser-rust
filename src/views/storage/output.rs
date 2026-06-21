//! Read-only storage-overview output — the data the DOM renderer consumes.
//!
//! Pure value types built by [`super::model::StorageModel`] from the live
//! stores; no behaviour, so they unit-test without WASM.

/// One bucket of live paths, keyed by the top-level tree segment they fall
/// under (`app`, `apps`, `sites`, `system`, …) — "where the paths live".
pub struct PrefixCount {
    pub label: String,
    pub count: usize,
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

/// The whole window's render input.
pub struct StorageOutput {
    pub peers: Vec<PeerStorage>,
    /// Origin disk estimate, once the async probe has resolved.
    pub estimate: Option<OriginEstimate>,
}
