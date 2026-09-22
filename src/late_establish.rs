//! **A §6.5 establisher slot that can be filled after the peer is built.**
//!
//! # The defect this removes
//!
//! `EXTENSION-NETWORK` §10.3's seam is a **constructor argument**:
//! [`Peers::new_direct_idb_with_establish`](crate::peers::Peers::new_direct_idb_with_establish)
//! takes it because it must be captured before the peer's `PeerShared` clones,
//! and there is no `&mut Peer` on this arm. So the establisher was decided
//! entirely at boot, from `resolve_provisioning`: `?webrtc_node=` in the URL,
//! then the localStorage selection mirror, then the build knob.
//!
//! **A fresh profile has none of the three.** A private window, a first visit, a
//! newly launched desktop app — nothing is configured yet, so no seam was
//! installed, and a rendezvous node chosen *afterwards* could not be installed
//! into the running peer. The whole first session was therefore unreachable:
//! discovery worked (it is an ordinary websocket call to the node and needs no
//! seam) while every connect-back was structurally impossible, so peers met each
//! other and no message ever moved. The only escape was a reload, and nothing
//! reliably said so.
//!
//! Reported from a desktop + private-window session on 2026-09-07, where it read
//! as *"chat regressed"*. Nothing had regressed; the flow had simply never
//! worked without a warm profile carrying the selection in localStorage.
//!
//! # The fix, and why it is sound rather than a trick
//!
//! Install a seam **always** — this one — holding nothing. §10.3's own contract
//! is what makes that free: *"Returning `None` — including 'no establisher
//! registered at all' — makes the ladder byte-identical to the pre-seam
//! behavior"*, and `Err` is documented as *"a reason, never a branch. Every
//! variant maps to the same fall-through to relay at the call site."* An unarmed
//! slot answers [`LiveEstablishError::NotAttempted`], whose own doc is exactly
//! this case — *"never started… this establisher does not handle this peer at
//! all… nothing was tried, so it says nothing about whether a path exists."*
//!
//! The seam is consulted **per dispatch**, at step 3b, so filling the slot later
//! is picked up by the next attempt with nothing to restart.
//!
//! # Two things this deliberately does not do
//!
//! **It does not claim reachability.** *A seam slot exists* and *a rendezvous
//! node is configured* are now different facts, and conflating them would make
//! [`Peers::peer_has_webrtc`](crate::peers::Peers::peer_has_webrtc) — which the
//! meet surfaces warn from — permanently true and the warning permanently
//! silent. The caller declares reachability from [`LateEstablisher::is_armed`];
//! the constructor no longer infers it from the seam being present (AP40: the
//! two facts got one variable, and the default arm took the stronger claim).
//!
//! **Re-arming with the same node is not an event.** [`Arm::Unchanged`] exists
//! so the frame loop can ask every frame without writing, logging or dirtying
//! anything — an idempotent operation reported as a change is AP43.
//!
//! # Stated cost
//!
//! An always-present slot is consulted where none used to be, so a session that
//! never configures a node accrues failed consultations against §10.3's
//! sequential backoff. That costs nothing in practice and is bounded by the
//! kernel's own constants: the first `ESTABLISH_FREE_CONSULTATIONS` (10) are
//! free, and the spacing is capped at `ESTABLISH_BACKOFF_CAP` (**10 s**). So the
//! worst case for a peer that was dispatched to before a node was chosen is that
//! the first attempt after arming is up to ten seconds late — *"a latency, not a
//! failure"*, in the words of the constant's own doc.

use std::sync::{Arc, RwLock};

use entity_peer::live_establish::{
    EstablishCtx, LiveEstablish, LiveEstablishError, LivePath,
};

/// What [`LateEstablisher::arm`] did.
///
/// Three outcomes, not a `bool`, because *"there was nothing here"* and *"you
/// pointed it somewhere else"* are different facts to a reader of the log, and
/// *"it already said that"* must be distinguishable from both or the frame loop
/// cannot call this idempotently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arm {
    /// The slot was empty and now holds an establisher for this node.
    Armed,
    /// It held one for a different node, which has been replaced — the user
    /// switched rendezvous nodes mid-session.
    Rearmed { from: String },
    /// Already armed for this exact node. Nothing was written.
    Unchanged,
}

/// A §6.5 seam whose inner establisher can be installed after construction.
///
/// See the module docs. Implements [`LiveEstablish`] by delegating when armed
/// and answering [`LiveEstablishError::NotAttempted`] when not.
pub struct LateEstablisher {
    /// `(node_peer_id, establisher)`. The node id is kept so `arm` can tell a
    /// re-point from a repeat without asking the establisher, which has no way
    /// to answer.
    inner: RwLock<Option<(String, Arc<dyn LiveEstablish>)>>,
}

impl Default for LateEstablisher {
    fn default() -> Self {
        Self::new()
    }
}

impl LateEstablisher {
    pub fn new() -> Self {
        Self { inner: RwLock::new(None) }
    }

    /// Is a rendezvous node installed right now?
    ///
    /// **This, not "a seam exists", is what reachability means.** It is the
    /// input to `Peers::set_webrtc_peer`, and through that to every meet
    /// surface's warning.
    pub fn is_armed(&self) -> bool {
        self.armed_node().is_some()
    }

    /// Which node this slot is pointed at, if any.
    pub fn armed_node(&self) -> Option<String> {
        self.inner.read().ok()?.as_ref().map(|(n, _)| n.clone())
    }

    /// Install (or replace) the establisher for `node_peer_id`.
    ///
    /// Idempotent by comparison: re-arming the same node writes nothing and
    /// reports [`Arm::Unchanged`], so a per-frame caller is free.
    pub fn arm(&self, node_peer_id: &str, establisher: Arc<dyn LiveEstablish>) -> Arm {
        let Ok(mut slot) = self.inner.write() else {
            // A poisoned lock cannot be repaired here and must not panic a
            // frame; the slot keeps whatever it had, which is the safe side.
            return Arm::Unchanged;
        };
        match slot.as_ref() {
            Some((node, _)) if node == node_peer_id => Arm::Unchanged,
            Some((node, _)) => {
                let from = node.clone();
                *slot = Some((node_peer_id.to_string(), establisher));
                Arm::Rearmed { from }
            }
            None => {
                *slot = Some((node_peer_id.to_string(), establisher));
                Arm::Armed
            }
        }
    }

    /// Install `establisher` whatever the slot holds — including a different
    /// establisher for the **same** node.
    ///
    /// [`Self::arm`] treats the same node as nothing to do, which is right for
    /// a caller asking every frame and wrong when the caller has already
    /// established that the provisioning changed (a node's reflectors arriving
    /// after it was added). That caller decides; this one obeys, and reports
    /// `Rearmed { from }` with `from` equal to the node in that case.
    pub fn install(&self, node_peer_id: &str, establisher: Arc<dyn LiveEstablish>) -> Arm {
        let Ok(mut slot) = self.inner.write() else {
            return Arm::Unchanged;
        };
        let previous = slot.as_ref().map(|(n, _)| n.clone());
        *slot = Some((node_peer_id.to_string(), establisher));
        match previous {
            Some(from) => Arm::Rearmed { from },
            None => Arm::Armed,
        }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl LiveEstablish for LateEstablisher {
    async fn establish_live(
        &self,
        ctx: EstablishCtx,
        peer_id: &str,
    ) -> Result<LivePath, LiveEstablishError> {
        // **Clone the handle out and drop the guard before awaiting.** Holding
        // a read guard across the await would deadlock the frame that re-arms
        // the slot — and re-arming during a traversal is exactly what happens
        // when a user switches nodes while a connection is being attempted.
        let armed = match self.inner.read() {
            Ok(slot) => slot.as_ref().map(|(_, e)| e.clone()),
            Err(_) => None,
        };
        match armed {
            Some(inner) => inner.establish_live(ctx, peer_id).await,
            // Not `NoPath`: nothing was tried, so this says nothing about
            // whether a path exists — the distinction that variant documents.
            None => Err(LiveEstablishError::NotAttempted {
                substrate: "late-establish",
                reason: "no rendezvous node is configured for this session yet"
                    .to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in inner establisher. Native has no WebRTC, so without one the
    /// delegating arm could never be exercised and a slot that always answered
    /// `NotAttempted` would look identical to a correct one.
    struct Stub(&'static str);

    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    impl LiveEstablish for Stub {
        async fn establish_live(
            &self,
            _ctx: EstablishCtx,
            _peer_id: &str,
        ) -> Result<LivePath, LiveEstablishError> {
            Err(LiveEstablishError::Refused {
                substrate: "stub",
                reason: self.0.to_string(),
            })
        }
    }

    #[test]
    fn a_fresh_slot_is_not_armed_and_names_no_node() {
        let late = LateEstablisher::new();
        assert!(!late.is_armed());
        assert_eq!(late.armed_node(), None);
    }

    /// The whole point: the slot is filled after construction.
    #[test]
    fn arming_a_fresh_slot_reports_armed_and_becomes_reachable() {
        let late = LateEstablisher::new();
        assert_eq!(late.arm("2KNodeA", Arc::new(Stub("a"))), Arm::Armed);
        assert!(late.is_armed(), "reachability follows the ARM, not the slot existing");
        assert_eq!(late.armed_node().as_deref(), Some("2KNodeA"));
    }

    /// **AP43** — a per-frame caller must be able to ask without writing. If
    /// this reported a change every frame, the log and any dirty signal wired
    /// to it would fire forever.
    #[test]
    fn re_arming_the_same_node_is_not_an_event() {
        let late = LateEstablisher::new();
        assert_eq!(late.arm("2KNodeA", Arc::new(Stub("a"))), Arm::Armed);
        for _ in 0..5 {
            assert_eq!(late.arm("2KNodeA", Arc::new(Stub("a"))), Arm::Unchanged);
        }
        assert_eq!(late.armed_node().as_deref(), Some("2KNodeA"));
    }

    /// Switching rendezvous nodes mid-session re-points the slot and says so —
    /// a different fact from a first arm, and the reason `arm` is not a `bool`.
    #[test]
    fn switching_nodes_re_points_the_slot_and_names_the_one_it_left() {
        let late = LateEstablisher::new();
        late.arm("2KNodeA", Arc::new(Stub("a")));
        assert_eq!(
            late.arm("2KNodeB", Arc::new(Stub("b"))),
            Arm::Rearmed { from: "2KNodeA".to_string() }
        );
        assert_eq!(late.armed_node().as_deref(), Some("2KNodeB"));
    }

    /// **`install` replaces an establisher for the same node** — the case `arm`
    /// deliberately ignores. Asserted through delegation, not `armed_node`,
    /// because the node name is the one thing that does not change: a slot that
    /// kept the old establisher would report the right node and dial with the
    /// wrong reflectors.
    #[tokio::test]
    async fn install_replaces_the_establisher_for_the_same_node() {
        let late = LateEstablisher::new();
        late.arm("2KNodeA", Arc::new(Stub("without reflectors")));
        assert_eq!(
            late.install("2KNodeA", Arc::new(Stub("with reflectors"))),
            Arm::Rearmed { from: "2KNodeA".to_string() }
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        match late.establish_live(EstablishCtx::dispatch(deadline), "2KPeer").await {
            Err(LiveEstablishError::Refused { reason, .. }) => {
                assert_eq!(reason, "with reflectors", "the slot must delegate to the new establisher")
            }
            Err(other) => panic!("expected the stub's refusal, got {other:?}"),
            Ok(_) => panic!("expected the stub's refusal, got a path"),
        }
    }
}
