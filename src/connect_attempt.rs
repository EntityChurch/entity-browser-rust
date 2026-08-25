//! In-memory outcome of the **last manual connect** — the app-owned "I pressed
//! Connect and here is what happened" fact, so that action can never again
//! complete in silence.
//!
//! Why this exists: `Action::ConnectPeer`'s result used to reach the user only
//! as a line in the *Event Log window* (`handle_connect_peer` logs, and on
//! failure `tracing::error!`s). The Peer Connections window itself had no
//! outcome surface at all — so pressing Connect cleared the address box and
//! showed nothing, whether the dial failed **or succeeded**. A success against
//! the system backend was the worst case: the row it produces is filtered out
//! of Known devices as infrastructure, so a connect that fully worked was
//! indistinguishable from one that did nothing. Silence is the enemy (D13).
//!
//! Why in-memory and not a tree entity — the same reasoning as
//! [`crate::dial_markers`], which this deliberately mirrors: this is the local
//! runtime's own **action-in-progress / action-just-finished**, tied to one
//! button press, meaningless across a reload. A tree store would strand a
//! `Dialing` row forever when a reload lands mid-dial, and would grow the
//! parallel-liveness mirror AP12/D8 forbid.
//!
//! **Keyed by address, not peer id** — that is the whole point. A manual connect
//! begins with an address typed by a human and does not learn the remote's
//! peer id until the handshake succeeds, so the failure case has no peer id to
//! key on. [`crate::dial_markers`] keys by `remote_pid` and therefore cannot
//! represent "the thing you typed didn't resolve to anyone."
//!
//! **Not liveness.** Nothing here feeds `conn_display`; the kernel
//! `system/peer/status` read-model stays the single authority on whether a peer
//! is connected. This reports on *the button press*, which is a different fact:
//! a connect can succeed while the peer drops a second later, and the two
//! surfaces should disagree in exactly that way.

use std::sync::{Arc, Mutex};

/// What became of the last manual connect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectOutcome {
    /// Dial in flight — no handshake yet.
    Dialing,
    /// The dial failed. Carries the reason as reported by the connect future,
    /// so the user sees *why*, not just "failed".
    Failed(String),
    /// Handshake completed. Carries the resolved remote display name so the
    /// success names who answered — an address can resolve to a peer whose row
    /// is then filtered from Known devices (the system backend), and this is
    /// the surface that keeps that case honest.
    Connected(String),
}

/// Cheap-to-clone handle to the app's single last-manual-connect slot. Cloned
/// into the render context ([`crate::dom::util::DomCtx`]) so the Peer
/// Connections model can read it at render — one owner, no per-window copy.
///
/// Only the most recent attempt is kept: there is one address box, so an older
/// attempt's outcome is never the one the user is waiting on.
#[derive(Clone, Default)]
pub struct ConnectAttempt {
    inner: Arc<Mutex<Option<(String, ConnectOutcome)>>>,
}

impl ConnectAttempt {
    pub fn new() -> Self {
        Self::default()
    }

    /// A dial to `addr` is in flight.
    pub fn set_dialing(&self, addr: &str) {
        self.set(addr, ConnectOutcome::Dialing);
    }

    /// The dial to `addr` failed with `reason` — the message the connect future
    /// produced, passed through verbatim rather than flattened to "failed".
    pub fn set_failed(&self, addr: &str, reason: &str) {
        self.set(addr, ConnectOutcome::Failed(reason.to_string()));
    }

    /// The dial to `addr` handshook with `remote` (a display name or peer id).
    pub fn set_connected(&self, addr: &str, remote: &str) {
        self.set(addr, ConnectOutcome::Connected(remote.to_string()));
    }

    fn set(&self, addr: &str, outcome: ConnectOutcome) {
        if let Ok(mut slot) = self.inner.lock() {
            *slot = Some((addr.to_string(), outcome));
        }
    }

    /// Drop the outcome — the user dismissed it or started somewhere new.
    pub fn clear(&self) {
        if let Ok(mut slot) = self.inner.lock() {
            *slot = None;
        }
    }

    /// The last attempt's address + outcome, `None` when nothing has been
    /// attempted this session. Tolerant read: a poisoned lock reads as `None`
    /// rather than panicking in the render path.
    pub fn read(&self) -> Option<(String, ConnectOutcome)> {
        self.inner.lock().ok().and_then(|slot| slot.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_attempted_reads_none() {
        assert_eq!(ConnectAttempt::new().read(), None);
    }

    #[test]
    fn a_failure_carries_its_reason_not_just_a_flag() {
        // The reason is the entire value of this surface — a bare "failed"
        // would leave the user exactly as stuck as the silence did.
        let a = ConnectAttempt::new();
        a.set_failed("ws://10.0.0.9:4041", "connection refused");
        assert_eq!(
            a.read(),
            Some((
                "ws://10.0.0.9:4041".to_string(),
                ConnectOutcome::Failed("connection refused".to_string())
            ))
        );
    }

    #[test]
    fn success_names_who_answered() {
        // Load-bearing for the system-backend case: the address resolves to a
        // peer whose Known-devices row is filtered out as infrastructure, so
        // this string is the ONLY thing that reports the connect worked.
        let a = ConnectAttempt::new();
        a.set_connected("ws://192.168.68.55:4041", "system-backend");
        assert_eq!(
            a.read(),
            Some((
                "ws://192.168.68.55:4041".to_string(),
                ConnectOutcome::Connected("system-backend".to_string())
            ))
        );
    }

    #[test]
    fn the_latest_attempt_replaces_the_previous_one() {
        // One address box ⇒ one live outcome. A stale success sitting under a
        // fresh failing dial would be worse than no surface at all.
        let a = ConnectAttempt::new();
        a.set_connected("ws://a:4041", "peer-a");
        a.set_dialing("ws://b:4041");
        assert_eq!(
            a.read(),
            Some(("ws://b:4041".to_string(), ConnectOutcome::Dialing))
        );
    }

    #[test]
    fn clones_share_one_slot() {
        // The handle is cloned into the render context; a clone must observe
        // writes made through the original.
        let a = ConnectAttempt::new();
        let b = a.clone();
        a.set_dialing("ws://x:4041");
        assert_eq!(b.read(), Some(("ws://x:4041".to_string(), ConnectOutcome::Dialing)));
        a.clear();
        assert_eq!(b.read(), None);
    }
}
