//! Application-layer display classification for managed peers.
//!
//! The SDK is classification-agnostic — a peer is just "a peer you built
//! with certain configuration options and extensions installed." From the
//! SDK's perspective there's no inherent Primary/Local/Remote distinction.
//!
//! These types live at the application layer because they encode
//! entity-browser UX policy:
//!   - which peer is the "primary" (the default peer)
//!   - which peers are directly accessible vs protocol-only
//!   - which peers the user is allowed to delete
//!
//! Views should classify peers through [`PeerDisplay::classify`] rather
//! than reading an SDK field.

use std::collections::HashMap;

use crate::peer_mode::PeerMode;
use crate::peers::Peers;

/// Display classification for a managed peer, derived from SDK facts
/// plus application policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerDisplay {
    /// The SDK's default peer. Always present, not user-deletable.
    Primary,
    /// A local peer with a PeerContext — direct tree access available.
    Local,
    /// A protocol-only peer (metadata-only; accessed via execute over a
    /// connection).
    Remote,
}

impl PeerDisplay {
    /// Derive the display classification for `peer_id` from Peers state.
    /// Routes the same query to Direct vs Worker arm via `Peers`.
    pub fn classify(peers: &Peers, peer_id: &str) -> Self {
        if peer_id == peers.default_peer_id() {
            Self::Primary
        } else if peers.is_backend_hosted(peer_id) {
            // Backend (Memory/OPFS) worker peer: it has a PeerContext
            // in its OWN dedicated SDK, so the `has_peer_context`
            // check below would wrongly call it a frontend ("Local")
            // peer. Classify as Remote → role_name/glyph render
            // "backend (...)". This is why every backend peer used to
            // show up as "frontend".
            Self::Remote
        } else if peers.has_peer_context(peer_id) {
            Self::Local
        } else {
            Self::Remote
        }
    }

    /// Short lowercase label (for CSS class names, logs, etc).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Local => "local",
            Self::Remote => "remote",
        }
    }

    /// Inverse of [`as_str`](Self::as_str). Unknown tags fall back to
    /// `Remote` (the conservative classification — no direct tree
    /// access assumed). Lets a consumer reconstruct the classification
    /// from a persisted registry record without re-querying `Peers`.
    pub fn from_tag(tag: &str) -> Self {
        match tag {
            "primary" => Self::Primary,
            "local" => Self::Local,
            _ => Self::Remote,
        }
    }

}

impl std::fmt::Display for PeerDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a peer is app infrastructure (System) or user-created (User).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerRole {
    /// An always-on, un-deletable control-plane peer: the in-app system peer,
    /// or the native system peer in a desktop deployment.
    System,
    /// A peer the user created.
    User,
}

/// Where a peer's isolation host runs — the "where does it live" fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerRuntime {
    /// The main browser/WebView thread (in-page).
    MainThread,
    /// A dedicated in-page Web Worker (own thread, same process).
    Worker,
    /// A separate native OS process, reached over the network.
    Native,
}

impl PeerRuntime {
    pub fn label(self) -> &'static str {
        match self {
            Self::MainThread => "main thread",
            Self::Worker => "worker",
            Self::Native => "native",
        }
    }
}

/// Where a peer's tree is persisted — the "does it survive / what backs it" fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerStorage {
    /// RAM only — gone on reload.
    InMemory,
    /// Browser IndexedDB (durable).
    IndexedDb,
    /// Origin-Private File System (durable).
    Opfs,
    /// The native process's own on-disk store.
    NativeStore,
}

impl PeerStorage {
    pub fn label(self) -> &'static str {
        match self {
            Self::InMemory => "in-memory",
            Self::IndexedDb => "IndexedDB",
            Self::Opfs => "OPFS",
            Self::NativeStore => "native store",
        }
    }

    /// Whether the tree survives a reload. `false` only for [`Self::InMemory`].
    #[allow(dead_code)] // Phase 2: the create-peer form + a durability hint consume this.
    pub fn is_durable(self) -> bool {
        !matches!(self, Self::InMemory)
    }
}

/// The three orthogonal, truthful facts about a peer — replaces the single
/// "role" string whose fall-through mislabeled the **native** system peer as
/// "backend (memory)". Each facet is derived from real runtime state:
/// - `role` — System (always-on infra) vs User (you created it).
/// - `runtime` — where the isolation host runs (main thread / worker / native).
/// - `storage` — where the tree is persisted (memory / IndexedDB / OPFS / native).
///
/// "Frontend" / "backend" are retired: they conflated *three* different runtimes
/// (in-page worker + memory, in-page worker + OPFS, and a separate native
/// process). `(runtime, storage)` says what those words never did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerDescriptor {
    pub role: PeerRole,
    pub runtime: PeerRuntime,
    pub storage: PeerStorage,
}

impl PeerDescriptor {
    /// Terse scan glyph: `★` in-app system · `⚙` native process ·
    /// `●` main-thread user peer · `●⛁` main-thread durable (IndexedDB) ·
    /// `◆` worker · `◆⛁` worker (OPFS).
    pub fn glyph(self) -> &'static str {
        match (self.role, self.runtime) {
            (_, PeerRuntime::Native) => "⚙",
            (PeerRole::System, _) => "★",
            (PeerRole::User, PeerRuntime::MainThread) => {
                // Mirror the worker OPFS distinction: a durable main-thread
                // (frontend-idb) peer must be visually distinct from an
                // ephemeral in-memory one.
                if self.storage == PeerStorage::IndexedDb {
                    "●⛁"
                } else {
                    "●"
                }
            }
            (PeerRole::User, PeerRuntime::Worker) => {
                if self.storage == PeerStorage::Opfs {
                    "◆⛁"
                } else {
                    "◆"
                }
            }
        }
    }

    /// A single, terse human role string (registry `role` field + logs), free
    /// of "frontend"/"backend".
    pub fn role_name(self) -> &'static str {
        match (self.role, self.runtime) {
            (PeerRole::System, PeerRuntime::Native) => "system (native)",
            (PeerRole::System, _) => "system",
            (PeerRole::User, PeerRuntime::Native) => "native",
            (PeerRole::User, PeerRuntime::MainThread) => {
                if self.storage == PeerStorage::IndexedDb {
                    "main thread (IndexedDB)" // i18n-ignore — registry `role` data field + technical tokens; user-facing twin is peers.kind_label.*
                } else {
                    "main thread"
                }
            }
            (PeerRole::User, PeerRuntime::Worker) => {
                if self.storage == PeerStorage::Opfs {
                    "worker (OPFS)" // i18n-ignore — registry `role` data field + technical tokens; user-facing twin is peers.kind_label.*
                } else {
                    "worker"
                }
            }
        }
    }

    /// Derive the descriptor for a **hosted/registry** peer from authoritative
    /// `Peers` state + the persisted-mode map. Pure read; no tree writes.
    ///
    /// Native detection is *structural*, not label-based: within the local peer
    /// registry, the only `Remote`-classified peer that isn't worker-hosted is a
    /// native-process backend (connection-pool remotes aren't in this set). The
    /// `system-backend` label only distinguishes the always-on *system* native
    /// peer (→ [`PeerRole::System`]) from a user-created native backend, a
    /// lifecycle concern — both share the same runtime + storage.
    pub fn describe(
        peers: &Peers,
        peer_id: &str,
        modes: &HashMap<String, PeerMode>,
    ) -> Self {
        let is_system_peer = peer_id == peers.system_peer_id();
        let is_backend_hosted = peers.is_backend_hosted(peer_id);
        // A registry peer with no local context and no dedicated worker SDK is a
        // native-process backend (see the structural note above).
        let is_native = !is_system_peer
            && !is_backend_hosted
            && PeerDisplay::classify(peers, peer_id) == PeerDisplay::Remote;

        let is_system_native = is_native
            && peers
                .peer_metadata(peer_id)
                .and_then(|m| m.label)
                .as_deref()
                == Some(crate::views::system_overview::model::SYSTEM_BACKEND_LABEL);

        let runtime = if is_native {
            PeerRuntime::Native
        } else if is_backend_hosted {
            PeerRuntime::Worker
        } else {
            PeerRuntime::MainThread
        };

        let role = if is_system_peer || is_system_native {
            PeerRole::System
        } else {
            PeerRole::User
        };

        let storage = if is_native {
            PeerStorage::NativeStore
        } else if is_system_peer {
            // The boot/system peer: IndexedDB on the Direct arm (the main-thread
            // default), OPFS on the Worker arm (`?worker=1` opt-in).
            if peers.primary_as_direct().is_some() {
                PeerStorage::IndexedDb
            } else {
                PeerStorage::Opfs
            }
        } else {
            match modes.get(peer_id) {
                Some(PeerMode::Frontend) => PeerStorage::InMemory,
                Some(PeerMode::FrontendIdb) => PeerStorage::IndexedDb,
                Some(PeerMode::BackendMemory) => PeerStorage::InMemory,
                Some(PeerMode::BackendOpfs) => PeerStorage::Opfs,
                // Unpersisted/ephemeral user peer (backends are normally
                // persisted, so this is the rare unsaved case).
                None => PeerStorage::InMemory,
            }
        };

        Self {
            role,
            runtime,
            storage,
        }
    }
}

/// Resolve a hosted peer's display role truthfully — the legacy
/// `(kind, glyph, role_name)` triple, now expressed through
/// [`PeerDescriptor::describe`] so there is one source of truth and the native
/// system peer can never fall through to "backend (memory)" again.
///
/// - `kind` — structural classification (Primary/Local/Remote), a *runtime*
///   fact from `classify` (which SDK hosts it). Drives badge color.
/// - `glyph` / `role_name` — from the descriptor's `(role, runtime, storage)`.
pub fn resolve_role(
    peers: &Peers,
    peer_id: &str,
    modes: &HashMap<String, PeerMode>,
) -> (PeerDisplay, &'static str, &'static str) {
    let kind = PeerDisplay::classify(peers, peer_id);
    let desc = PeerDescriptor::describe(peers, peer_id, modes);
    (kind, desc.glyph(), desc.role_name())
}

/// Whether the user interface should allow deleting this peer.
///
/// Application policy: the primary (default) peer is never user-deletable
/// because the SDK relies on it always being present.
///
/// Only called from the WASM DOM render path (`render_dom`), so native
/// builds would flag it as unused without the allow attribute.
#[allow(dead_code)]
pub fn is_user_deletable(peers: &Peers, peer_id: &str) -> bool {
    peer_id != peers.default_peer_id()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_peers() -> Peers {
        Peers::new_direct()
    }

    #[test]
    fn default_peer_classifies_as_primary() {
        let peers = make_peers();
        let id = peers.primary_peer_id().to_string();
        assert_eq!(PeerDisplay::classify(&peers, &id), PeerDisplay::Primary);
    }

    #[test]
    fn backend_peer_classifies_as_remote() {
        let mut peers = make_peers();
        let pid = "2KBackendDisplay123".to_string();
        peers.register_backend_peer_primary(pid.clone(), None, Vec::new());
        assert_eq!(PeerDisplay::classify(&peers, &pid), PeerDisplay::Remote);
    }

    #[test]
    fn unknown_peer_classifies_as_remote() {
        let peers = make_peers();
        assert_eq!(PeerDisplay::classify(&peers, "unknown-peer-id"), PeerDisplay::Remote);
    }

    #[test]
    fn default_peer_is_not_user_deletable() {
        let peers = make_peers();
        let id = peers.primary_peer_id().to_string();
        assert!(!is_user_deletable(&peers, &id));
    }

    #[test]
    fn other_peers_are_user_deletable() {
        let mut peers = make_peers();
        let pid = "2KOtherPeerId999".to_string();
        peers.register_backend_peer_primary(pid.clone(), None, Vec::new());
        assert!(is_user_deletable(&peers, &pid));
    }

    // --- PeerDescriptor::describe — the truthful role · runtime · storage ---

    #[test]
    fn system_peer_describes_as_system_main_thread_indexeddb() {
        let peers = make_peers();
        let sys = peers.system_peer_id().to_string();
        let d = PeerDescriptor::describe(&peers, &sys, &HashMap::new());
        assert_eq!(d.role, PeerRole::System);
        assert_eq!(d.runtime, PeerRuntime::MainThread);
        // Direct arm → IndexedDB (the main-thread default).
        assert_eq!(d.storage, PeerStorage::IndexedDb);
        assert_eq!(d.glyph(), "★");
        assert_eq!(d.role_name(), "system");
    }

    /// Regression lock: the native system peer must NEVER read as
    /// "backend (memory)" again — the fall-through bug the descriptor replaced.
    #[test]
    fn native_system_backend_describes_as_system_native() {
        let mut peers = make_peers();
        let pid = "2KNativeSystemBackend1".to_string();
        peers.register_backend_peer_primary(
            pid.clone(),
            Some(crate::views::system_overview::model::SYSTEM_BACKEND_LABEL.to_string()),
            vec!["ws://127.0.0.1:4042".to_string()],
        );
        let d = PeerDescriptor::describe(&peers, &pid, &HashMap::new());
        assert_eq!(d.role, PeerRole::System, "the labeled native peer is infra");
        assert_eq!(d.runtime, PeerRuntime::Native);
        assert_eq!(d.storage, PeerStorage::NativeStore);
        assert_eq!(d.glyph(), "⚙");
        assert_eq!(d.role_name(), "system (native)");
    }

    /// A native-process peer the user created (any non-system label) is Native
    /// runtime but a User role — Start/Stop-able, unlike the system one.
    #[test]
    fn user_native_backend_describes_as_user_native() {
        let mut peers = make_peers();
        let pid = "2KUserNativeBackend1".to_string();
        peers.register_backend_peer_primary(
            pid.clone(),
            Some("my-server".to_string()),
            vec!["ws://127.0.0.1:5001".to_string()],
        );
        let d = PeerDescriptor::describe(&peers, &pid, &HashMap::new());
        assert_eq!(d.role, PeerRole::User);
        assert_eq!(d.runtime, PeerRuntime::Native);
        assert_eq!(d.storage, PeerStorage::NativeStore);
        assert_eq!(d.role_name(), "native");
    }

    /// A durable this-tab (frontend-idb) peer must be visually distinct from an
    /// ephemeral in-memory one — same as the worker OPFS/memory distinction.
    /// Both are (User, MainThread); only the storage axis differs.
    #[test]
    fn durable_main_thread_peer_is_distinct_from_ephemeral() {
        let ephemeral = PeerDescriptor {
            role: PeerRole::User,
            runtime: PeerRuntime::MainThread,
            storage: PeerStorage::InMemory,
        };
        let durable = PeerDescriptor {
            role: PeerRole::User,
            runtime: PeerRuntime::MainThread,
            storage: PeerStorage::IndexedDb,
        };
        assert_eq!(ephemeral.glyph(), "●");
        assert_eq!(ephemeral.role_name(), "main thread");
        assert!(!ephemeral.storage.is_durable());

        assert_eq!(durable.glyph(), "●⛁");
        assert_eq!(durable.role_name(), "main thread (IndexedDB)");
        assert!(durable.storage.is_durable());
    }
}
