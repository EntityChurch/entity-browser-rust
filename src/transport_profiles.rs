//! Publish the durable transport profile the kernel's dispatch ladder reads —
//! so "how do I reach peer X" is a tree lookup instead of an app-tier lookup.
//!
//! ## The inversion this closes
//!
//! `get_or_connect` (`core/peer/src/remote.rs`) resolves a peer to a transport
//! in five rungs; rung 2 is `resolve_transport_address`, which lists
//! `/{local}/system/peer/transport/{remote_hex}/{profile-id}` and dials the
//! winner. **Nothing in production wrote those entities** — every
//! `for_local_listener` caller upstream is a test — so rung 2 never fired in
//! our deployment, and the app filled the gap with its own address book
//! (`connections.rs` holds `(remote_pid, addr)`; `maintain-peer` hands the
//! address back to the kernel on every call). That is the same layering
//! inversion as the deleted `connection_health` mirror: an app-tier parallel
//! store for something the kernel is designed to own. Its user-visible symptom
//! is `no transport profile for peer` on the first dispatch after a reload.
//! (`REVIEW-CONNECTIVITY-LAYER-COHERENCE-2026-08-11` §4.)
//!
//! ## Why this is app-tier and not a spec question
//!
//! The profile lives under the **local** peer's own root. It is not a published
//! claim about someone else — it is a private note about how *I* reached *them*,
//! and upstream names this use explicitly (`TcpProfileData::to_entity`: "used …
//! by interop tests / sync code that mirrors a discovered peer's profile into
//! the local tree"). The genuinely spec-adjacent question — a peer advertising
//! profiles *about itself* for others to consume or gossip — is a different
//! question this module does not touch.
//!
//! ## The invariant that keeps traversal out
//!
//! **Only an explicitly dialed address is published.** A traversal (WebRTC)
//! connection MUST publish no durable profile — its reachability is ephemeral
//! by construction, asserted upstream in `core/peer/src/lib.rs`. That invariant
//! is held structurally rather than by a scheme allowlist: the only caller is
//! `Peers::connect_peer`, which exists precisely because a caller had an
//! address to dial. Rung-4 establishment never reaches this code. **If you add
//! a second call site, re-derive that it holds** — publishing a profile for a
//! peer reached by hole-punching would hand the ladder an address that was
//! never dialable.
//!
//! ## Known limits (deliberate, in this slice)
//!
//! - **Identity-form PeerIDs only.** `{remote_hex}` is the hex of the remote's
//!   `system/peer` content_hash. Identity-multihash PIDs (`hash_type = 0x00`,
//!   canonical Ed25519 — what every peer in this app uses) derive it locally
//!   with zero lookup. A SHA-256-form PID (Ed448 and anything above the v7.65
//!   §4 substrate floor) cannot, and upstream recovers it from the cached
//!   session entity via a `pub(crate)` helper we cannot call. Such a peer gets
//!   no profile and a `debug!` line — never a silent skip.
//! - **The entity type is `tcp` whatever the URL scheme is.** Upstream decodes
//!   exactly two live profile types — `system/peer/transport/{tcp,http}` — and
//!   `TcpProfileData::from_entity` fails closed unless the `transport_type`
//!   field equals `"tcp"` (D5). There is no decoded `…/transport/websocket`
//!   type, so a `ws://` endpoint must ride the tcp profile. That is the
//!   intended reading — the resolver returns `endpoint.url` **with its scheme**
//!   and the outbound dispatcher routes on that scheme (`MultiConnector`,
//!   D4) — but the missing websocket profile type is a real upstream gap worth
//!   routing, and if it lands this module should publish under it.
//! - **Staleness is not managed here.** A profile outlives the address that
//!   produced it (a backend on a dynamic port moves). A repeat connect
//!   overwrites `primary` in place, so the last address that *worked* wins;
//!   pruning an address that has stopped working is not built. Without that,
//!   this moves the stale address book rather than curing it.

use entity_entity::Entity;
use entity_peer::transport_profile::TcpProfileData;

use crate::writer_handle::WriterHandle;

/// The reserved profile-id upstream selects first (`remote::PROFILE_ID_PRIMARY`).
/// Selection is `(effective_priority asc, profile-id lex)`, and `primary` with no
/// explicit `priority` has effective priority 0 — so the address we just dialed
/// successfully is preferred over any sibling profile.
pub const PROFILE_ID_PRIMARY: &str = "primary";

/// Tree path of the profile for `remote_hex` under `local_peer_id`.
///
/// Fully qualified from the leading slash — the same `/{peer_id}/…` shape the
/// resolver builds. This must match `resolve_transport_address`'s prefix
/// (`/{local}/system/peer/transport/{hex}/`) byte for byte or the write lands
/// somewhere nothing reads.
pub fn profile_path(local_peer_id: &str, remote_hex: &str, profile_id: &str) -> String {
    format!("/{local_peer_id}/system/peer/transport/{remote_hex}/{profile_id}")
}

/// The `{peer_id_hex}` path segment for `remote_peer_id`, or `None` when it
/// cannot be derived locally (SHA-256-form PID — see the module's known limits).
pub fn remote_hex(remote_peer_id: &str) -> Option<String> {
    entity_crypto::PeerId::from(remote_peer_id).identity_hex_local()
}

/// Build the §6.5.2a profile entity for "`remote_peer_id` is reachable at
/// `address`".
///
/// `advertised_at` is informational only — D3 forbids it from feeding a
/// selection or correctness decision — so it is threaded in rather than read
/// from a clock here, which also keeps the encoding deterministic under test.
/// `priority` is left unset: `primary` already sorts first by convention, and
/// an explicit value would only matter once we publish siblings.
pub fn profile_entity(remote_peer_id: &str, address: &str, advertised_at: Option<u64>) -> Entity {
    let mut data = match advertised_at {
        Some(ms) => TcpProfileData::for_local_listener(remote_peer_id, address, ms),
        None => TcpProfileData::for_local_listener_no_clock(remote_peer_id, address),
    };
    // `for_local_listener` names the canonical case — a peer publishing its own
    // listener. Ours describes a remote we dialed; the shape is identical and
    // the constructor is the upstream-sanctioned way to build it, but the
    // `transport_type` field MUST stay `"tcp"` to satisfy the D5 fail-closed
    // check even when `address` is `ws://` or `memory://`. Assert that rather
    // than trusting the constructor to keep doing it.
    debug_assert_eq!(
        data.transport_type,
        entity_peer::transport_profile::TRANSPORT_TCP,
        "D5: a system/peer/transport/tcp entity must carry transport_type=tcp"
    );
    data.priority = None;
    data.to_entity()
}

/// Record that `remote_peer_id` was just reached at `address`, in the shape the
/// dispatch ladder's rung 2 reads.
///
/// Idempotent on the path — a repeat connect overwrites `primary`, so the last
/// address that worked is the one the ladder will try. Fire-and-forget through
/// [`WriterHandle`], so both arms are covered by construction and neither can
/// be silently stubbed. A miss is observability-only: it costs a rung-2
/// resolution later, never the connection in hand.
pub fn publish_dialed(
    writer: &WriterHandle,
    local_peer_id: &str,
    remote_peer_id: &str,
    address: &str,
    advertised_at: Option<u64>,
) {
    if remote_peer_id == local_peer_id {
        return;
    }
    if !address.contains("://") {
        tracing::debug!(
            address = %address,
            "transport profile not published: address carries no scheme, so nothing could dial it"
        );
        return;
    }
    let Some(hex) = remote_hex(remote_peer_id) else {
        tracing::debug!(
            remote = %remote_peer_id,
            "transport profile not published: the peer-id hex path segment is not locally \
             derivable for this PeerID form (SHA-256-form); the ladder falls back to an \
             app-supplied dial"
        );
        return;
    };
    let path = profile_path(local_peer_id, &hex, PROFILE_ID_PRIMARY);
    tracing::debug!(path = %path, address = %address, "publishing transport profile");
    writer.put(path, profile_entity(remote_peer_id, address, advertised_at));
}

/// The prefix every published route for `local_peer_id` lives under.
///
/// **Worker-arm subscription target.** A tree read on the Worker arm hits a
/// main-thread cache mirror populated only for *subscribed* prefixes, so any
/// surface calling [`address_for`] must watch this or every address reads
/// `None` for peers that are perfectly reachable. One prefix, one place to get
/// it right (`MODEL-REMOTE-PEER-FACTS` §4).
pub fn routes_prefix(local_peer_id: &str) -> String {
    format!("/{local_peer_id}/system/peer/transport/")
}

/// The address `local_peer_id` last reached `remote_peer_id` at, read from the
/// kernel's own route entity — **the single durable home of a peer's address**
/// (`MODEL-REMOTE-PEER-FACTS` §1).
///
/// `None` when no route is published: never connected, a SHA-256-form PeerID
/// whose hex we cannot derive, or (Worker arm) the prefix is not subscribed.
/// All three mean "we cannot say how to reach this peer", which is the honest
/// answer — the app must not substitute a remembered guess.
pub fn address_for(peers: &crate::peers::Peers, local_peer_id: &str, remote_peer_id: &str) -> Option<String> {
    let hex = remote_hex(remote_peer_id)?;
    let path = profile_path(local_peer_id, &hex, PROFILE_ID_PRIMARY);
    let entity = peers.get_entity(local_peer_id, &path)?;
    TcpProfileData::from_entity(&entity)
        .ok()
        .map(|p| p.endpoint_url)
        .filter(|u| !u.is_empty())
}

/// Drop the published route to `remote_peer_id` from `local_peer_id`'s tree.
///
/// **Forget must forget the route, not just the row.** Before profiles existed,
/// forgetting a peer meant dropping its `connections.rs` registry entry and the
/// dial marker, and nothing durable remained. Publishing changed that: a
/// forgotten peer would keep a durable address in the kernel's own tree, and
/// the ladder would go on dialing a peer the user explicitly dismissed. That is
/// the `connection_health` disease in a new place — a surface saying "gone"
/// while a parallel durable store says "here" — so the two teardowns travel
/// together.
///
/// Idempotent: removing an absent path is a no-op, which matters because a
/// metadata-derived row has no profile and a peer may have been reached from
/// only some local peers.
pub fn forget(writer: &WriterHandle, local_peer_id: &str, remote_peer_id: &str) {
    let Some(hex) = remote_hex(remote_peer_id) else {
        return;
    };
    let path = profile_path(local_peer_id, &hex, PROFILE_ID_PRIMARY);
    tracing::debug!(path = %path, "forgetting transport profile");
    writer.remove(path);
}

/// Epoch-ms for the informational `advertised_at` field.
#[cfg(target_arch = "wasm32")]
pub fn now_epoch_ms() -> Option<u64> {
    Some(js_sys::Date::now() as u64)
}

/// Epoch-ms for the informational `advertised_at` field.
#[cfg(not(target_arch = "wasm32"))]
pub fn now_epoch_ms() -> Option<u64> {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The written path must be the prefix `resolve_transport_address` lists.
    /// This is the whole point of the module — a path that doesn't match is a
    /// write nothing reads, and it would look exactly like success.
    #[test]
    fn the_path_is_the_one_the_resolver_lists() {
        assert_eq!(
            profile_path("LOCAL", "abcd", PROFILE_ID_PRIMARY),
            "/LOCAL/system/peer/transport/abcd/primary"
        );
        // The resolver builds `/{local}/system/peer/transport/{hex}/` and lists
        // under it; our path must sit inside that prefix.
        let prefix = format!("/{}/system/peer/transport/{}/", "LOCAL", "abcd");
        assert!(profile_path("LOCAL", "abcd", PROFILE_ID_PRIMARY).starts_with(&prefix));
    }

    /// A round-trip through the upstream decoder — if this fails, the entity we
    /// write is one the resolver skips as a malformed sibling (it logs into
    /// diagnostics and walks on, so the failure would be invisible at runtime).
    #[test]
    fn the_entity_decodes_through_the_upstream_resolver_path() {
        let entity = profile_entity("REMOTE_PID", "ws://127.0.0.1:4041", Some(1_700_000_000_000));
        assert_eq!(
            entity.entity_type,
            entity_peer::transport_profile::TYPE_PEER_TRANSPORT_TCP
        );
        let decoded = TcpProfileData::from_entity(&entity).expect("upstream must decode our entity");
        assert_eq!(decoded.peer_id, "REMOTE_PID");
        assert_eq!(
            decoded.endpoint_url, "ws://127.0.0.1:4041",
            "the URL keeps its scheme — the outbound dispatcher routes on it"
        );
        assert_eq!(decoded.freshness, entity_peer::transport_profile::FRESHNESS_LIVE);
        assert!(decoded
            .supported_ops
            .contains(&entity_peer::transport_profile::OP_EXECUTE.to_string()));
    }

    /// A non-tcp scheme must still decode. The D5 check is on the
    /// `transport_type` *field*, not on the URL, and getting that backwards
    /// would make every `ws://` profile we publish undecodable.
    #[test]
    fn a_non_tcp_scheme_still_satisfies_the_d5_field_check() {
        for addr in ["ws://h:1", "wss://h:1", "tcp://h:1", "memory://PID"] {
            let entity = profile_entity("REMOTE_PID", addr, None);
            let decoded =
                TcpProfileData::from_entity(&entity).unwrap_or_else(|e| panic!("{addr}: {e}"));
            assert_eq!(decoded.endpoint_url, addr);
            assert_eq!(decoded.advertised_at, None, "omitted, not zero");
        }
    }

    /// Identity-form (Ed25519) PIDs derive the hex locally — the case every
    /// peer in this app is. A non-PeerID string derives nothing rather than
    /// producing a garbage path.
    #[test]
    fn hex_derives_for_an_identity_form_pid_and_not_for_junk() {
        let keypair = entity_crypto::Keypair::generate();
        let pid = keypair.peer_id().to_string();
        let hex = remote_hex(&pid).expect("an identity-form PID derives its hex locally");
        assert_eq!(hex.len(), 66, "33-byte content_hash as lowercase hex");
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));

        assert!(remote_hex("not-a-peer-id").is_none());
    }
}
