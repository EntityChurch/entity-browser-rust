//! The desktop backend as a **§6.5 rendezvous** — `EXTENSION-SIGNALING`'s
//! `system/signaling` handler mounted on the peer that already listens.
//!
//! # Why this exists
//!
//! A browser peer has no listener, so two browsers cannot find each other
//! without a third party holding a bulletin board: A deposits an offer under a
//! rendezvous key, B collects it. That is the *only* infrastructure a
//! browser↔browser connection needs on a LAN — `make e2e-webrtc-lan` proved
//! host candidates carry the media with **zero** reflectors, but it did not
//! remove the rendezvous, and nothing else can.
//!
//! Before this module the only way to get one was to run
//! `cmd/entity-signaling-node` yourself. Meanwhile *this* process already binds
//! a WebSocket listener a browser can reach (`start_backend_peer`), and
//! `entity-peer`'s `signaling` feature was already on. The node was three lines
//! away and mounted only in `#[cfg(test)]`. Mounting it here is the difference
//! between the ecosystem **hosting** the rendezvous and **federating** it: one
//! person on the Wi-Fi opens the desktop app and everyone else's browser can
//! meet through it.
//!
//! # The one rule this module exists to hold [AP22]
//!
//! **Being a node is one decision, and both halves come from one expression.**
//! A node needs (a) the handler mounted and (b) an admission grant that lets a
//! connecting peer actually call `offer`/`collect`. Those are separate
//! mechanisms — `PeerBuilder::handler` and `PeerBuilder::with_seed_policy` —
//! and splitting the decision across them is precisely the shape that shipped a
//! build resolving a signaling node and installing no establisher: every
//! deployment satisfied one half and nobody could reach the other. So
//! [`mount`] returns both or neither, and there is no way to ask for one.
//!
//! Note what makes the trap invisible here: the desktop default posture is
//! `debug_open_grants` (`ENTITY_BROWSER_ENFORCE` unset), which authorizes
//! everything. A node mounted with no seed policy therefore **works perfectly
//! in every default build** and 403s the moment anyone enforces — the same way
//! the extension's own in-process tests missed a real admission bug for
//! exactly this reason (`signaling_seed_grants` doc: *"the in-process live
//! tests missed it because they seed a wildcard, which authorizes everything
//! and so proves nothing about admission"*). The gate below runs enforced.

use std::sync::Arc;

use entity_capability::GrantEntry;
use entity_signaling::{signaling_seed_grants, Limits, SignalingCore, SignalingHandler};

/// Environment override for the per-peer persisted setting. `1`/`true` forces
/// the node on, `0`/`false` forces it off, absent defers to what was persisted.
///
/// Exists for the e2e and for a headless box with no UI to click, **not** as
/// the user-facing knob — the toggle in System Overview is that, and it writes
/// the persisted setting this can override.
pub const ENV_ENABLE: &str = "ENTITY_BROWSER_SIGNALING_NODE";

/// Everything needed to make one peer a rendezvous, produced together.
///
/// There is deliberately no way to take one field and drop the other — see the
/// module header. `endpoint` is carried through so the caller can report what
/// the node publishes without re-deriving it.
pub struct NodeMount {
    pub handler: Arc<SignalingHandler>,
    pub seed_policy: Vec<(String, Vec<GrantEntry>)>,
    pub endpoint: String,
}

/// Resolve whether this backend serves rendezvous: the environment wins when it
/// says anything, otherwise the persisted per-peer setting decides.
///
/// **Off is the default and it is the fail-closed direction.** Serving
/// rendezvous means strangers who can reach this listener may deposit blobs
/// here; that is a deliberate act of hosting for other people, and our posture
/// everywhere else (no default node, no default registry) is explicit-or-
/// nothing. What it is *not* is dangerous-by-default: see [`mount`] for the
/// bound on what an enabled node exposes.
pub fn resolve_enabled(persisted: bool) -> bool {
    match std::env::var(ENV_ENABLE) {
        Ok(v) if v == "1" || v.eq_ignore_ascii_case("true") => true,
        Ok(v) if v == "0" || v.eq_ignore_ascii_case("false") => false,
        // A value we do not recognise is not a vote. Falling through to the
        // persisted setting beats guessing, and beats refusing to boot over a
        // typo in an env var that only overrides a default.
        _ => persisted,
    }
}

/// Build both halves of the node decision for a peer listening at `endpoint`.
///
/// `endpoint` MUST be the **connectable** address (`connectable_addr`), never
/// the bind address: `advertise` publishes it verbatim for peers to dial, and a
/// node telling the world it lives at `ws://0.0.0.0:4041` has published an
/// address nobody can reach. The caller therefore has to bind before it builds
/// the peer — which is also why `start_backend_peer` moves its
/// `WebSocketListener::bind` above the `PeerBuilder`.
///
/// # What an enabled node exposes, precisely
///
/// The seeded grant is [`signaling_seed_grants`] — exactly the three signaling
/// operations on exactly `system/signaling`, with an **empty resource scope**.
/// It is not a wildcard, so it does not hand a caller any other handler on this
/// peer; in particular it grants nothing on `local/files`, so turning on the
/// rendezvous does **not** widen access to the file share.
///
/// It is seeded under the `default` peer-pattern, i.e. **anyone who can reach
/// this listener may rendezvous** — the `--open` posture of
/// `cmd/entity-signaling-node`. That is what a node *is*: peers that have never
/// met cannot be on an allow-list, and a rendezvous only helps strangers. The
/// exposure it buys is bounded by [`Limits::default`] (the §5 pins: ≤8 KiB per
/// blob, ≤32 blobs per key, 60 s TTL, a cap on live keys) and is **memory only
/// — a node writes no disk and keeps nothing** (§1.3: losing a node drops
/// in-flight handshakes and nothing that mattered).
///
/// # No reflection endpoints, deliberately
///
/// `advertise` can carry `reflection_endpoints` (§4.5.1) and this node
/// publishes **none**. The field is *"this node's own §9.3 STUN listener(s),
/// never a directory of anyone else's"* — we run no STUN server, so there is
/// nothing truthful to put there, and pointing it at a public reflector would
/// be advertising infrastructure we neither run nor vouch for. A user who wants
/// a reflector types one on the connector row, where it is their own choice.
pub fn mount(endpoint: &str, node_peer_id: &str) -> NodeMount {
    let core = Arc::new(SignalingCore::with_limits(
        endpoint.to_string(),
        Limits::default(),
    ));
    NodeMount {
        handler: Arc::new(SignalingHandler::new(core, node_peer_id)),
        seed_policy: vec![("default".to_string(), signaling_seed_grants())],
        endpoint: endpoint.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The env override is a three-way answer, not a boolean: present-and-true,
    /// present-and-false, and *absent* are distinct, and only the third defers
    /// to what the user persisted. An override that could not say "off" would
    /// leave an e2e unable to test the disabled path on a box where a previous
    /// run persisted `true`.
    #[test]
    fn the_environment_can_force_either_way_and_silence_defers() {
        // Serialized by construction: this is the only test touching the var.
        std::env::remove_var(ENV_ENABLE);
        assert!(resolve_enabled(true), "absent env defers to persisted true");
        assert!(!resolve_enabled(false), "absent env defers to persisted false");

        std::env::set_var(ENV_ENABLE, "1");
        assert!(resolve_enabled(false), "env forces on over persisted off");
        std::env::set_var(ENV_ENABLE, "true");
        assert!(resolve_enabled(false), "`true` spelling also forces on");

        std::env::set_var(ENV_ENABLE, "0");
        assert!(!resolve_enabled(true), "env forces off over persisted on");
        std::env::set_var(ENV_ENABLE, "false");
        assert!(!resolve_enabled(true), "`false` spelling also forces off");

        // A typo is not a vote — it must not silently mean "off", which would
        // turn a misspelled override into a node that quietly stopped serving.
        std::env::set_var(ENV_ENABLE, "yes-please");
        assert!(resolve_enabled(true), "unrecognised value defers to persisted");
        std::env::remove_var(ENV_ENABLE);
    }

    /// The grant is narrow, and the assertion that matters is the *negative*
    /// one: enabling the rendezvous must not widen access to anything else on
    /// this peer, above all the file share it sits beside.
    #[test]
    fn the_seeded_grant_covers_signaling_only_and_never_the_share() {
        let m = mount("ws://192.168.1.10:4041", "2KTestNode");
        assert_eq!(m.seed_policy.len(), 1);
        let (pattern, grants) = &m.seed_policy[0];
        assert_eq!(pattern, "default", "a rendezvous serves peers it has never met");
        assert_eq!(grants.len(), 1);

        let handlers = format!("{:?}", grants[0].handlers);
        assert!(
            handlers.contains(entity_signaling::PATTERN),
            "grant must cover system/signaling, got {handlers}"
        );
        assert!(
            !handlers.contains("local/files"),
            "the rendezvous grant must not reach the file share: {handlers}"
        );
        assert!(
            !handlers.contains('*'),
            "a wildcard here hands every caller every handler on this peer: {handlers}"
        );
    }

    /// We publish no reflectors because we run no STUN server. A future edit
    /// that "helpfully" points this at a public one is advertising
    /// infrastructure we do not run — the misuse §4.5.1's own wording names.
    #[test]
    fn the_node_advertises_no_reflectors_it_does_not_run() {
        let m = mount("ws://192.168.1.10:4041", "2KTestNode");
        assert_eq!(m.endpoint, "ws://192.168.1.10:4041");
        // Reached through the handler's core, which is what `advertise` reads.
        assert!(
            m.handler.core().reflection_endpoints().is_empty(),
            "this deployment serves no §9.3 reflection, so it must claim none"
        );
    }
}
