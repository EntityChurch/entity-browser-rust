//! **Foreign-namespace writes — the app's most important write, and the
//! ceiling that briefly forbade it.**
//!
//! This browser holds other peers' subtrees in ITS OWN store at their natural
//! universal paths (V7 §1.4 Category A):
//!
//! - a **cached foreign content site** at `/{them}/sites/{S}/…`
//!   (`src/content_site/paths.rs`, written by `src/content_site/discovery.rs`'s
//!   `warm_peer_sites` and by the resolver's cache-on-read),
//! - a **chat delivery mirror** at `/{them}/app/chat/{conv}/messages/…`
//!   (`src/views/chat/delivery.rs` — both the `follow(Payload)` reactive path
//!   and the poll path's `dispatch_write`).
//!
//! Browsing somebody else's site and receiving somebody else's chat messages
//! *are the product*, so a regression that denies these writes is a total
//! functional failure — and until this file there was no NATIVE test of it at
//! all. The coverage lived only in the Selenium-gated `e2e` suite (Phase 21b /
//! Phase 27 in `tests/e2e_worker.rs`), which is off by default and therefore
//! silent on a `make check`.
//!
//! ## Why this file exists (entity-core-rust `db21e24`, 2026-08-23)
//!
//! core-rust `7c21d04` (2026-08-18) gave the in-process sub-dispatch path a
//! §5.2 capability ceiling where it had previously run no capability check at
//! all. That ceiling began reading the `resources` field of the §6.9 default
//! per-handler self-grant, which was encoded as a bare `*` — and bare `*`
//! canonicalizes to `/{local}/*`, i.e. OWN NAMESPACE ONLY. For five days a
//! handler-dispatched write into `/{them}/…` returned 403. `db21e24` re-encoded
//! that default in the R-5 cross-namespace form `/*/*`
//! (`entity_capability::default_handler_self_grant`).
//!
//! **We were not hit**, and these tests are the executed proof of both halves:
//!
//! - every foreign-namespace write this app performs runs under
//!   [`DispatchCeiling::PeerRoot`] — L0 `tree.put` on the Direct arm, and
//!   `PeerContext::put` / `Peer::execute_with_options` / the `follow(Payload)`
//!   `MirrorWriter` everywhere else — and PeerRoot does not consult the
//!   resource dimension at all (`connection.rs`: `PeerRoot => true`). The first
//!   three tests exercise those shapes;
//! - the last test reproduces the denial and its fix directly, at the ceiling
//!   the app does NOT use, so this file also fails loudly if the dependency is
//!   ever rolled back to a core-rust that lacks `db21e24`.
//!
//! The app registers exactly one handler of its own (`SignalingHandler`,
//! `src/connectors.rs` + `src-tauri/src/lib.rs`) and it declares an explicit
//! EMPTY `internal_scope`, so it never rode the §6.9 default either.
//!
//! There is no `[lib]` target in this crate, so — as in
//! `tests/local_files_write_persistence.rs` — these reproduce the app's call
//! shapes with the same public `entity_peer` primitives the app uses, rather
//! than importing app modules.

use entity_capability::{
    default_handler_self_grant, wildcard_handler_grant, CapabilityToken, Granter, ResourceTarget,
};
use entity_crypto::Keypair;
use entity_ecf::{text, to_ecf, Value};
use entity_entity::Entity;
use entity_handler::ExecuteOptions;
use entity_peer::connection::{make_execute_fn, DispatchCeiling};
use entity_peer::PeerBuilder;

/// A real, well-formed peer id that is not ours — the `{them}` of the mirror.
fn foreign_peer_id() -> String {
    Keypair::from_seed([7u8; 32]).peer_id().to_string()
}

fn cached_page(body: &str) -> Entity {
    Entity::new("content/page", to_ecf(&text(body))).unwrap()
}

/// `system/tree:put` params — `{entity: {type, data}}`, the shape
/// `entity_sdk`'s `build_put_params` sends (§3.2 path-as-resource, so the
/// mirror path travels in the resource target, not the params).
fn put_params(entity: &Entity) -> Entity {
    let data_value: Value = ciborium::from_reader(entity.data.as_slice()).unwrap();
    let params = Value::Map(vec![(
        text("entity"),
        Value::Map(vec![
            (text("type"), text(&entity.entity_type)),
            (text("data"), data_value),
        ]),
    )]);
    Entity::new("system/tree/put/params", to_ecf(&params)).unwrap()
}

fn resource_opts(path: &str) -> ExecuteOptions {
    ExecuteOptions {
        resource: Some(ResourceTarget {
            targets: vec![path.to_string()],
            exclude: vec![],
        }),
        ..Default::default()
    }
}

/// **The Direct arm's cache write** — `WriterHandle::Direct` (`src/writer_handle.rs`)
/// is a raw L0 `shared.tree.put`, which is how `warm_peer_sites` caches another
/// peer's site manifest on boot. No dispatch, so no capability dimension of any
/// kind applies; the store does not validate the path's peer-segment.
#[test]
fn direct_arm_l0_write_lands_in_a_foreign_namespace() {
    let peer = PeerBuilder::new().keypair(Keypair::generate()).build().unwrap();
    let shared = peer.shared();
    let them = foreign_peer_id();
    let path = format!("/{them}/sites/blog/manifest");

    let entity = cached_page("their manifest, cached in our store");
    shared
        .tree
        .put(&path, entity.clone())
        .expect("L0 put of a foreign-namespace path");

    let got = shared.tree.get(&path).expect("present");
    assert_eq!(
        got.content_hash, entity.content_hash,
        "the Direct-arm site cache must hold the foreign peer's entity at its \
         universal path"
    );
}

/// **The dispatched write** — `Peer::execute_with_options` is what the Worker
/// arm reaches through `PeerContext::put` (`wasm-worker-host`'s `handle_put`)
/// and what `Peers::execute` reaches on either arm. It dispatches under
/// `DispatchCeiling::PeerRoot`, so §5.2's resource dimension does not bind:
/// the peer is root over its own store, whatever peer-segment the path names.
#[tokio::test]
async fn dispatched_write_lands_in_a_foreign_namespace() {
    let peer = PeerBuilder::new().keypair(Keypair::generate()).build().unwrap();
    peer.local_only();
    let shared = peer.shared();
    let them = foreign_peer_id();
    let path = format!("/{them}/app/chat/conv-1/messages/00000001");

    let entity = cached_page("a message they authored");
    let result = peer
        .execute_with_options("system/tree", "put", put_params(&entity), resource_opts(&path))
        .await
        .expect("dispatch");
    assert_eq!(
        result.status, 200,
        "a chat-delivery mirror write into /{{them}}/app/... must be authorized \
         — this is the write that 403'd between core-rust 7c21d04 and db21e24 \
         when made under a HANDLER ceiling"
    );
    assert!(
        shared.tree.get(&path).is_some(),
        "the mirrored message must be readable at the author's universal path \
         (ChatModel::load_messages reads the union)"
    );
}

/// **The `follow(Payload)` mirror write, exactly** — `entity_sdk`'s
/// `MirrorWriter::local_tree_put` builds its dispatch with
/// `make_execute_fn(..., DispatchCeiling::PeerRoot)` and puts at the remote's
/// qualified path. `src/views/chat/delivery.rs` follows each conversation
/// participant in `FollowMode::Payload`, so this is the reactive delivery path
/// on the Direct arm, reproduced at the same seam.
///
/// (The mode that DID break upstream is `FollowMode::Continuation`, whose
/// standing leg is materialized by a continuation handler — a handler ceiling.
/// This app does not use that mode.)
#[tokio::test]
async fn follow_payload_mirror_write_shape_lands() {
    let peer = PeerBuilder::new().keypair(Keypair::generate()).build().unwrap();
    peer.local_only();
    let shared = peer.shared();
    let identity = shared.identity_hash;
    let them = foreign_peer_id();
    let path = format!("/{them}/app/chat/conv-1/messages/00000002");

    let execute = make_execute_fn(
        shared.clone(),
        Some(identity),
        Default::default(),
        None,
        None,
        DispatchCeiling::PeerRoot,
    );
    let entity = cached_page("delivered in-band by follow(Payload)");
    let result = execute(
        "system/tree".into(),
        "put".into(),
        put_params(&entity),
        resource_opts(&path),
    )
    .await
    .expect("dispatch");

    assert_eq!(result.status, 200, "the follow(Payload) mirror write must land");
    assert!(shared.tree.get(&path).is_some());
}

/// **The regression itself, reproduced — and the fix pinned.**
///
/// Same dispatch as above, but under a HANDLER ceiling, which is what a
/// sub-dispatch made *by a handler* gets (`connection.rs` builds
/// `DispatchCeiling::Handler(handler_grant)` for every handler invocation).
///
/// - the pre-fix encoding of §6.9's "all resources" — `wildcard_handler_grant`,
///   a bare `*` that canonicalizes to `/{local}/*` — DENIES the foreign path;
/// - the post-fix encoding — `default_handler_self_grant`, the R-5 `/*/*` form
///   that `db21e24` made the builder seed — ALLOWS it.
///
/// This app takes neither branch today (it registers one handler, with an
/// explicit empty `internal_scope`), but the day a browser handler mirrors a
/// foreign subtree it will, and this test is the tripwire. It also fails if the
/// path dependency is ever pointed back at a core-rust without `db21e24`.
#[tokio::test]
async fn handler_ceiling_denies_the_old_encoding_and_allows_the_fixed_one() {
    let peer = PeerBuilder::new().keypair(Keypair::generate()).build().unwrap();
    peer.local_only();
    let shared = peer.shared();
    let identity = shared.identity_hash;
    let them = foreign_peer_id();

    let token = |grants| CapabilityToken {
        grants,
        granter: Granter::Single(identity),
        grantee: identity,
        parent: None,
        created_at: 0,
        expires_at: None,
        not_before: None,
        delegation_caveats: None,
    };
    let dispatch = |grants| {
        make_execute_fn(
            shared.clone(),
            Some(identity),
            Default::default(),
            None,
            None,
            DispatchCeiling::Handler(Some(Box::new(token(grants)))),
        )
    };
    let entity = cached_page("mirrored by a handler");

    let denied = dispatch(wildcard_handler_grant())(
        "system/tree".into(),
        "put".into(),
        put_params(&entity),
        resource_opts(&format!("/{them}/sites/blog/pages/index")),
    )
    .await
    .expect("dispatch");
    assert_eq!(
        denied.status, 403,
        "bare `*` canonicalizes to /{{local}}/* — under it a handler cannot \
         write the foreign subtrees this store legitimately holds. This is the \
         2026-08-18 → 2026-08-23 defect (core-rust 7c21d04)."
    );

    let allowed = dispatch(default_handler_self_grant())(
        "system/tree".into(),
        "put".into(),
        put_params(&entity),
        resource_opts(&format!("/{them}/sites/blog/pages/index")),
    )
    .await
    .expect("dispatch");
    assert_eq!(
        allowed.status, 200,
        "the /*/* form is what §6.9's \"all resources\" means, and what the \
         peer builder seeds since core-rust db21e24"
    );
}
