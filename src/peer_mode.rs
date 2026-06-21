//! `PeerMode` — host + persistence config for a newly-created peer.
//!
//! Stage 2B. Minimal three-variant surface aligned with
//! the user-facing peer modes:
//!
//! - **Frontend** — `{ MainThread, InMemory }`. Lives in the main
//!   wasm context, no persistence.
//! - **BackendMemory** — `{ WorkerThread, InMemory }`. Lives in a web
//!   worker, no persistence.
//! - **BackendOpfs** — `{ WorkerThread, Opfs }`. Lives in a web
//!   worker, tree backed by OPFS (persistent across reloads).
//!
//! Future stages will widen this to the full `(PeerHost, TreePersistence)`
//! product (e.g. `{ MainThread, Opfs }` if we ever wire it up). For
//! now the three modes are an enum because they're what the UI
//! exposes.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerMode {
    Frontend,
    BackendMemory,
    BackendOpfs,
}

impl PeerMode {
    #[allow(dead_code)] // Stage 2C will consume this for fallback-cascade logic.
    pub fn is_worker_hosted(self) -> bool {
        matches!(self, PeerMode::BackendMemory | PeerMode::BackendOpfs)
    }

    pub fn wants_opfs(self) -> bool {
        matches!(self, PeerMode::BackendOpfs)
    }

    pub fn label(self) -> &'static str {
        match self {
            PeerMode::Frontend => "frontend",
            PeerMode::BackendMemory => "backend (memory)",
            PeerMode::BackendOpfs => "backend (opfs)",
        }
    }

    /// Stable, terse identifier for persistence on disk and in
    /// localStorage. Don't ever change these without a migration —
    /// existing peers' persisted entries will get reloaded as Frontend
    /// (the default-on-unknown fallback).
    ///
    /// MIGRATION INVARIANT (MAP §8, danger site #2): the string→mode mapping
    /// *and the storage location each mode implies* are durable contract.
    /// Renaming a key, or moving where a mode's tree lives, silently strands
    /// every peer of that mode — that is a migration, not an edit.
    pub fn persist_key(self) -> &'static str {
        match self {
            PeerMode::Frontend => "frontend",
            PeerMode::BackendMemory => "backend-memory",
            PeerMode::BackendOpfs => "backend-opfs",
        }
    }

    /// Inverse of [`persist_key`]. Returns `None` on unknown strings
    /// so callers can decide whether to default to Frontend (boot) or
    /// drop the entry (defensive).
    pub fn from_persist_key(s: &str) -> Option<Self> {
        match s {
            "frontend" => Some(PeerMode::Frontend),
            "backend-memory" => Some(PeerMode::BackendMemory),
            "backend-opfs" => Some(PeerMode::BackendOpfs),
            _ => None,
        }
    }
}
