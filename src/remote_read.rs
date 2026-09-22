//! `remote_read` — the one expression of *"read something out of another peer's
//! tree over whatever connection reaches them"*.
//!
//! ## Why this is a module and not a second copy of four lines
//!
//! It was written once, for file transfer, and lived inside
//! [`file_offer`](crate::file_offer) — where every line of it is about a
//! *remote read* and not one is about a *file*. The moment a second consumer
//! appeared ([`feed_peer`](crate::feed_peer), reading a followed publisher's
//! feed off their live tree) the choice was to copy it or to move it, and C15's
//! rule says which: **one expression, call sites gated against it.** The
//! precedent is EMBED's payload union, which was implemented once under
//! `content_site::format::AssetPayload` with a comment claiming the factoring
//! the code did not have. *A comment claiming a factoring is not the
//! factoring* — so this is the extraction made at the moment a second caller
//! proved the boundary, rather than a standalone rename.
//!
//! **Transport-immaterial by construction.** Every call goes through
//! [`DispatchHandle`], which dispatches over whatever pooled connection reaches
//! the target — a WebRTC data channel exactly as readily as a WebSocket or the
//! in-process memory transport a native gate stands up. There is no branch on
//! transport in this file and there must never be one. It is **arm-agnostic**
//! for the same reason: the handle branches on Direct/Worker once, in one file,
//! and nothing here knows which arm it is on.
//!
//! ## The four outcomes, and why they are four
//!
//! [`read_entity_at`] is the whole read surface, and its result is
//! [`RemoteReadError`] plus `Ok(None)` — **four facts that must not merge**:
//!
//! | | Means | What it licenses you to say |
//! |---|---|---|
//! | `Ok(Some(e))` | 200 | here it is |
//! | `Ok(None)` | 404 | **they do not have this** |
//! | `Err(NotShared)` | 403 | they have it and have not shared it with you |
//! | `Err(Faulted)` | any other status | their peer answered with a fault |
//! | `Err(Unreachable)` | transport, after the establish ladder | we could not reach them |
//!
//! **Folding 403 into `Ok(None)` is the defect this table exists to refuse**,
//! and it is AP54's wrong sentence with a shorter fuse: a reader told *"this
//! publisher has posted nothing"* about a publisher who has posted plenty and
//! simply has not granted them the prefix will go and look at the publisher,
//! which is the one place the problem is not. The default connection grants
//! (`default_connection_grants`, `core/capability`) cover `system/tree:get` at
//! `system/type/*` and `system/handler/*` **and nothing else**, so a 403 is the
//! *expected* answer for any app-tier path the author has not shared —
//! i.e. this is the common case, not the exotic one.
//!
//! Same reason `Unreachable` and `Faulted` stay apart: *nobody answered* and
//! *somebody answered with a fault* send a person to different places, which is
//! the split `deployment_config::read_document` already carries one tier down.

use entity_capability::ResourceTarget;
use entity_ecf::{to_ecf, Value};
use entity_entity::Entity;
use entity_handler::ExecuteOptions;

use crate::dispatch_handle::DispatchHandle;

/// How many times a **remote** dispatch is retried while a path is still being
/// established, and how long to wait between tries. See [`remote_execute`].
pub(crate) const ESTABLISH_TRIES: usize = 10;
pub(crate) const ESTABLISH_GAP_MS: u32 = 1_000;

/// Scope one dispatch to a single resource path.
pub(crate) fn resource_opts(target: &str) -> ExecuteOptions {
    ExecuteOptions {
        resource: Some(ResourceTarget { targets: vec![target.to_string()], exclude: vec![] }),
        ..Default::default()
    }
}

/// The params entity for an operation that takes none.
pub(crate) fn empty_params() -> Entity {
    Entity::new("system/empty", to_ecf(&Value::Null)).expect("system/empty Null is well-formed")
}

/// One remote dispatch, retried while the transport error looks like "there is
/// no path *yet*".
///
/// **This is not defensive padding — it is the caller's half of §7.2.1.** The
/// §6.5 establisher runs exactly ONE negotiation per consultation and never
/// retries it (`caller_owns_retry`, structural in `main_thread_establish.rs`),
/// so a first cross-peer dispatch between two peers that have only ever met by
/// name reliably arrives before any channel exists: the ladder consults, the
/// negotiation loses the race (`no live path … sdp_exchange=INCOMPLETE`), and a
/// one-shot caller reports "no transport profile for peer" — which reads like
/// the peer is unreachable when the truth is "ask again in a second".
///
/// Chat never had to think about this because its 5 Hz delivery poll *is* the
/// retry (the A3 finding: the poll is load-bearing). The transfer verbs were
/// the first genuinely one-shot §10.3 caller here, and they failed on exactly
/// that — measured, in `e2e-webrtc-file`, before this existed. A feed read is
/// the second, and it inherits the fix rather than rediscovering it.
///
/// Bounded on purpose, and bounded *here* rather than by adding app-tier "how
/// many times has §6.5 failed" state: the per-peer consultation backoff lives
/// one layer down in `core/peer` and is keyed correctly; a second counter in
/// the app is how `connection_health` happened. Ten tries at a second apart is
/// well inside the free-consultation window a healthy meet uses.
///
/// A returned `HandlerResult` — **including a 403 or 404 — is an answer** and
/// is never retried; only a transport `Err` is.
pub(crate) async fn remote_execute(
    dispatch: &DispatchHandle,
    handler_uri: String,
    operation: String,
    params: Entity,
    opts: ExecuteOptions,
) -> Result<entity_handler::HandlerResult, String> {
    let mut last = String::new();
    for attempt in 0..ESTABLISH_TRIES {
        if attempt > 0 {
            crate::dispatch_handle::delay_ms(ESTABLISH_GAP_MS).await;
        }
        match dispatch
            .execute(handler_uri.clone(), operation.clone(), params.clone(), opts.clone())
            .await
        {
            Ok(result) => return Ok(result),
            Err(e) => {
                tracing::debug!(
                    "remote-read: {handler_uri} {operation} attempt {} failed: {e}",
                    attempt + 1
                );
                last = e;
            }
        }
    }
    Err(format!(
        "{last} (gave up after {ESTABLISH_TRIES} attempts — no path to the peer was \
         established; if you met by name, check both sides installed an establisher)"
    ))
}

/// Why a remote read did not return an entity — **never why it returned none.**
///
/// Absence is `Ok(None)` and is an ordinary fact; everything here is a failure
/// to *look*. See the module doc's table for what each licenses you to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteReadError {
    /// 403. They hold this path and have not granted it to us.
    NotShared { path: String },
    /// Any other non-2xx. Their peer answered, with a fault.
    Faulted { status: u32, path: String },
    /// The dispatch never landed, after the establish ladder gave up.
    Unreachable { detail: String },
}

impl RemoteReadError {
    /// The one-word label for this outcome, so a gate can assert the four are
    /// four without asserting on a whole sentence a translator may move.
    pub fn kind(&self) -> &'static str {
        match self {
            RemoteReadError::NotShared { .. } => "not-shared",
            RemoteReadError::Faulted { .. } => "faulted",
            RemoteReadError::Unreachable { .. } => "unreachable",
        }
    }
}

impl std::fmt::Display for RemoteReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteReadError::NotShared { path } => write!(
                f,
                "this peer has not shared {path} with you — they are reachable, \
                 so this is a permission they have not granted, not an outage"
            ),
            RemoteReadError::Faulted { status, path } => {
                write!(f, "this peer answered with status {status} for {path}")
            }
            RemoteReadError::Unreachable { detail } => {
                write!(f, "could not reach this peer: {detail}")
            }
        }
    }
}

/// What a status code means for a remote read. **Pure, and separated from the
/// dispatch for exactly the charter's reason:** the only other way to reach this
/// mapping is to stand up two peers and get a real peer to refuse you, and under
/// today's posture (`debug_open_grants`) no peer here refuses anything — so
/// every arm but one would be unreachable and therefore ungated. A pure decision
/// is gated by `make test` on both arms and on a posture neither arm runs yet.
///
/// It is also the **one** expression of the mapping: [`read_entity_at`] and
/// [`list_keys_at`] had the identical four-arm match, which is the shape where
/// one of them grows a case and the other does not (C15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadOutcome {
    /// 2xx — the peer served it.
    Served,
    /// 404 — the peer does not have this. An ordinary fact, never an error.
    Absent,
    /// Anything else — a failure to look, with its own word.
    Refused(RemoteReadError),
}

/// The mapping. See [`ReadOutcome`] for why this is not inline.
pub fn classify(status: u32, path: &str) -> ReadOutcome {
    match status {
        200..=299 => ReadOutcome::Served,
        404 => ReadOutcome::Absent,
        403 => ReadOutcome::Refused(RemoteReadError::NotShared { path: path.to_string() }),
        status => {
            ReadOutcome::Refused(RemoteReadError::Faulted { status, path: path.to_string() })
        }
    }
}

/// Read one entity out of `peer`'s tree at the **fully qualified** `path`.
///
/// This is the live-transport twin of a signed-root `resolve`: same key space,
/// different authority. A published tree is **root-anchored** — the publisher's
/// signature over a trie is what makes a stranger's origin safe to fetch from.
/// A live read is **connection-anchored**: the kernel authenticated this peer id
/// at the handshake (a canonical Ed25519 peer id embeds its own key), so the
/// bytes came from the author by construction and there is no root to check
/// them against, nor any need for one — *asking the author to prove they are
/// themselves is what the handshake already did.*
///
/// **What this does NOT weaken:** anything carrying its own detached signature
/// is still verified independently of the transport. `FEED-R4` attribution runs
/// on entities, so a feed read over a live link is attributed exactly as one
/// over HTTP, by the same code.
pub async fn read_entity_at(
    dispatch: &DispatchHandle,
    peer: &str,
    path: &str,
) -> Result<Option<Entity>, RemoteReadError> {
    let result = remote_execute(
        dispatch,
        format!("entity://{peer}/system/tree"),
        "get".to_string(),
        empty_params(),
        resource_opts(path),
    )
    .await
    .map_err(|detail| RemoteReadError::Unreachable { detail })?;

    match classify(result.status, path) {
        ReadOutcome::Served => Ok(Some(result.result)),
        ReadOutcome::Absent => Ok(None),
        ReadOutcome::Refused(e) => Err(e),
    }
}

/// List the immediate child key **names** bound under `prefix` in `peer`'s tree.
///
/// `prefix` must be fully qualified and end in `/` — a trailing slash is what
/// makes `system/tree:get` answer with a `system/tree/listing` rather than with
/// the entity at that exact key.
///
/// **404 is an empty list, not an error.** A prefix with nothing under it and a
/// prefix that exists and is empty are the same fact to every caller here: the
/// peer has bound nothing there. A **403 is still an error**, for the module
/// doc's reason — *they have not shared this* must never arrive as *they have
/// nothing*.
pub async fn list_keys_at(
    dispatch: &DispatchHandle,
    peer: &str,
    prefix: &str,
) -> Result<Vec<String>, RemoteReadError> {
    debug_assert!(prefix.ends_with('/'), "a listing prefix needs its trailing slash");
    let result = remote_execute(
        dispatch,
        format!("entity://{peer}/system/tree"),
        "get".to_string(),
        empty_params(),
        resource_opts(prefix),
    )
    .await
    .map_err(|detail| RemoteReadError::Unreachable { detail })?;

    match classify(result.status, prefix) {
        ReadOutcome::Served => Ok(crate::backend_auth::parse_listing_keys(&result.result)),
        ReadOutcome::Absent => Ok(Vec::new()),
        ReadOutcome::Refused(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Every outcome has its own word, and there are exactly three.**
    ///
    /// The count is asserted so a fourth cannot quietly reuse one of these —
    /// the same shape as `every_hydration_outcome_has_its_own_word`. Absence is
    /// deliberately not in this enum: it is `Ok(None)`, because *"they do not
    /// have it"* is not a failure to look and folding it in here is exactly the
    /// merge the module doc refuses.
    #[test]
    fn every_remote_read_failure_has_its_own_word() {
        let all = [
            RemoteReadError::NotShared { path: "/p/app/feed/index".into() },
            RemoteReadError::Faulted { status: 502, path: "/p/app/feed/index".into() },
            RemoteReadError::Unreachable { detail: "no route".into() },
        ];
        let kinds: std::collections::BTreeSet<_> = all.iter().map(|e| e.kind()).collect();
        assert_eq!(kinds.len(), all.len(), "two outcomes share a word");
        assert_eq!(all.len(), 3, "a fourth outcome needs its own word and its own row here");

        // The sentences differ too, and the one that matters most is that a
        // refusal never reads as an outage.
        let not_shared = all[0].to_string();
        assert!(not_shared.contains("not shared"), "a 403 must say so: {not_shared}");
        assert!(
            !not_shared.contains("could not reach"),
            "a permission is not an outage: {not_shared}"
        );
    }

    /// **A 403 is a REFUSAL and a 404 is an ABSENCE, and the whole point is that
    /// they do not merge.** Folding the first into the second is the defect this
    /// module exists to refuse: it renders *"they have not shared their feed with
    /// you"* as *"this publisher has posted nothing"*, which sends a reader to
    /// look at the publisher — the one place the problem is not.
    ///
    /// ⚠ **Gated here and NOT through two peers, because under today's posture
    /// no peer refuses anything** (`debug_open_grants`; see
    /// `feed_peer::live_tests::the_live_read_needs_no_grant_today`). A two-peer
    /// gate for this would be a test that cannot fail.
    #[test]
    fn a_refusal_and_an_absence_are_different_answers() {
        let p = "/PEER/app/feed/index";
        assert_eq!(classify(200, p), ReadOutcome::Served);
        assert_eq!(classify(404, p), ReadOutcome::Absent, "404 is an ordinary absence");
        assert_eq!(
            classify(403, p),
            ReadOutcome::Refused(RemoteReadError::NotShared { path: p.into() }),
            "a 403 must NOT arrive as an absence"
        );
        assert_eq!(
            classify(502, p),
            ReadOutcome::Refused(RemoteReadError::Faulted { status: 502, path: p.into() }),
            "a fault is not a permission and not an absence"
        );
        // A 2xx that is not literally 200 is still served — a reader that
        // matched `200` alone would treat a 204 as a fault.
        assert_eq!(classify(204, p), ReadOutcome::Served);
    }
}
