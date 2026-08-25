//! In-memory dial-transient markers — the app-owned "a dial is in flight" /
//! "a dial gave up before ever connecting" facts the kernel liveness surface
//! deliberately does NOT model, so they live in memory, **not** the tree.
//!
//! Why in-memory and not an entity (the usual "state lives in the tree" rule):
//! `system/peer/status` is a spec-locked **three-state** enum —
//! `connected`/`suspect`/`disconnected` (ENTITY-CORE-PROTOCOL §3.13; Amendment
//! 12 rung-1 ruling D explicitly rejected a `connecting`/`reconnecting` value,
//! routing "reconnecting" to the *derived* `system/network/peer-summary`). The
//! kernel writes a status entity only **after** a handshake, so a dial in
//! progress — or one that gave up before ever connecting — has no kernel shape.
//! That transient is not *liveness* (an observation about the remote); it is the
//! local runtime's own **action-in-progress**, tied to a live dial future and
//! meaningless across a reload. It belongs with the other in-memory control-loop
//! state (`SystemBackendConnect`'s `attempts`/`cooldown`), not the data model.
//!
//! Writing it to a tree store would (a) recreate the exact stale-mirror bug this
//! arc deleted the `connection_health` mirror to fix — a reload mid-dial would
//! strand a `Dialing` entity forever, with nothing to clear it — and (b) add the
//! fourth parallel liveness store AP12/D8 forbid. Keeping it in memory is
//! stale-proof by construction: it starts empty every load and is reconstructed
//! only from live dials.
//!
//! **The kernel read-model stays authoritative.** [`crate::peer_liveness::conn_display`]
//! resolves a marker only while the kernel is silent (`Unknown` — no status
//! entity yet); a real `connected`/`suspect`/`disconnected` always supersedes it.
//! So a stale marker can never mask a real kernel state — the whole point.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::peer_liveness::DialHint;

/// Cheap-to-clone handle to the app's single in-memory dial-marker store, keyed
/// by remote Base58 `peer_id`. A missing entry ⇒ [`DialHint::None`]. Cloned into
/// the render context ([`crate::dom::util::DomCtx`]) so the display models can
/// read it without a parallel per-window copy — one owner, read at render.
#[derive(Clone, Default)]
pub struct DialMarkers {
    inner: Arc<Mutex<HashMap<String, DialHint>>>,
}

impl DialMarkers {
    pub fn new() -> Self {
        Self::default()
    }

    /// A dial to `remote_pid` is in flight (no handshake yet). Shown as
    /// "Connecting…" only while the kernel has written no status.
    pub fn set_dialing(&self, remote_pid: &str) {
        self.set(remote_pid, DialHint::Dialing);
    }

    /// A dial to `remote_pid` gave up without ever connecting — the kernel wrote
    /// no status, so this is the app's own "couldn't reach it" knowledge.
    pub fn set_failed(&self, remote_pid: &str) {
        self.set(remote_pid, DialHint::Failed);
    }

    fn set(&self, remote_pid: &str, hint: DialHint) {
        if let Ok(mut m) = self.inner.lock() {
            m.insert(remote_pid.to_string(), hint);
        }
    }

    /// Drop any dial marker for `remote_pid` — the dial resolved (the kernel now
    /// owns this peer's liveness) or the peer was forgotten.
    pub fn clear(&self, remote_pid: &str) {
        if let Ok(mut m) = self.inner.lock() {
            m.remove(remote_pid);
        }
    }

    /// This peer's dial transient, [`DialHint::None`] when the app isn't dialing
    /// it. A `try_borrow`-style tolerant read: a poisoned lock reads as `None`
    /// rather than panicking in the render path.
    pub fn hint(&self, remote_pid: &str) -> DialHint {
        self.inner
            .lock()
            .ok()
            .and_then(|m| m.get(remote_pid).copied())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_peer_reads_none() {
        let m = DialMarkers::new();
        assert_eq!(m.hint("NEVER_SEEN"), DialHint::None);
    }

    #[test]
    fn set_and_read_transients() {
        let m = DialMarkers::new();
        m.set_dialing("REMOTE_B");
        assert_eq!(m.hint("REMOTE_B"), DialHint::Dialing);
        m.set_failed("REMOTE_B");
        assert_eq!(m.hint("REMOTE_B"), DialHint::Failed, "later write overwrites");
    }

    #[test]
    fn clear_drops_the_marker() {
        let m = DialMarkers::new();
        m.set_dialing("REMOTE_B");
        m.clear("REMOTE_B");
        assert_eq!(m.hint("REMOTE_B"), DialHint::None);
    }

    #[test]
    fn clones_share_one_store() {
        // The handle is cloned into the render context; a clone must see writes
        // made through the original (one owner, shared inner).
        let a = DialMarkers::new();
        let b = a.clone();
        a.set_dialing("REMOTE_B");
        assert_eq!(b.hint("REMOTE_B"), DialHint::Dialing);
    }
}
