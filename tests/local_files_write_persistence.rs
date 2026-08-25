//! Reproduction + regression guard for the Tori "upload vanishes" bug: an
//! upload reports `write status=200`, a read-back returns 200 (file provably on
//! disk that instant), yet seconds later the file is gone.
//!
//! **ROOT CAUSE (found + fixed 2026-07-03).** The Tauri backend writes every
//! file via `atomic_write` (temp file + `std::fs::rename` into place). On Linux
//! the rename destination fires inotify `IN_MOVED_TO` → notify
//! `Modify(Name(To))`, and `local-files/watcher.rs::classify` mapped ANY
//! `Modify(Name(_))` to `FsEvent::Deleted`. So the fs watcher recorded the
//! freshly-written file's *arrival* as a *deletion*, and `flush_pending` removed
//! the tree entity (without checking the file still existed) → a `Deleted` tree
//! event → the reverse-writer `remove_file`d it, ~1 debounce window later.
//!
//! Why it looked intermittent / cross-peer-specific: the watcher only runs when
//! the handler's `load()` auto-starts it, which happens only for roots
//! **rehydrated from a persisted tree** — i.e. the SQLite-backed backend on its
//! 2nd+ run. A fresh in-memory peer never starts a watcher, so the earlier
//! cross-peer / SQLite / WebSocket repros below (which use a fresh temp db) all
//! PASS. The missing ingredient was the watcher, not the arm or the wire.
//!
//! The fix (entity-core-rust `local-files/watcher.rs`): `flush_pending` no
//! longer honors a `Deleted` for a path that still exists on disk — it treats it
//! as an arrival and ingests it. `write_then_watcher_debounce_does_not_delete`
//! is the isolated regression guard (fails pre-fix, passes post-fix).
//!
//! The cross-peer ladder (`…_backend`, `…_sqlite`, `…_sqlite_ws`) stays as
//! evidence that the write path itself is sound, and `live_upload_against_running_backend`
//! (ignored; `LIVE_BACKEND_WS=…`) drives a real WS upload against a running
//! backend — the reproduction that actually caught the watcher.
//!
//! Native only; uses the same `entity-peer` the Tauri backend builds. The
//! `sqlite` + `websocket` features are enabled test-only via a dev-dependency
//! (see Cargo.toml) so the `make wasm` lib build is unaffected.

#![cfg(not(target_arch = "wasm32"))]

use std::time::Duration;

use entity_capability::ResourceTarget;
use entity_crypto::Keypair;
use entity_entity::Entity;
use entity_handler::ExecuteOptions;
use entity_peer::local_files::RootConfigData;
use entity_peer::{PeerBuilder, PeerConfig};

fn write_params(bytes: &[u8]) -> Entity {
    // Same shape the app sends and the handler decodes (WriteRequestData).
    let data = entity_ecf::to_ecf(&entity_ecf::cbor_map! {
        "bytes" => entity_ecf::Value::Bytes(bytes.to_vec())
    });
    Entity::new("local/files/write-request", data).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_persists_with_engines_running() {
    let tmp = tempfile::tempdir().unwrap();
    let kp = Keypair::from_seed([7u8; 32]);
    let pid = kp.peer_id().to_string();

    let peer = PeerBuilder::new()
        .keypair(kp)
        .config(PeerConfig {
            debug_open_grants: true,
            ..PeerConfig::default()
        })
        .build()
        .expect("peer builds");
    let shared = peer.shared();
    // NB: start_engines starts the reverse-write loop but does NOT start the
    // fs watcher for local-files roots (add_root doesn't either — only the
    // handler's `load()` auto-starts watchers, for roots rehydrated from a
    // persisted tree). This test therefore has NO watcher — see
    // `write_then_watcher_debounce_does_not_delete` for the watcher case, which
    // is where the upload-vanishes bug lives.
    peer.start_engines(&shared);

    peer.local_files_handler()
        .add_root(
            "shared",
            RootConfigData {
                prefix: "local/files/shared/".to_string(),
                filesystem_root: tmp.path().to_string_lossy().into_owned(),
                read_only: false,
                ..Default::default()
            },
        )
        .expect("add root");

    // Fully-qualified resource, like the wire path delivers it.
    let target = format!("/{}/local/files/shared/hello.txt", pid);
    let opts = ExecuteOptions {
        resource: Some(ResourceTarget {
            targets: vec![target],
            exclude: vec![],
        }),
        ..Default::default()
    };

    let res = peer
        .execute_with_options("local/files", "write", write_params(b"hello upload"), opts)
        .await
        .expect("write dispatches");
    assert_eq!(res.status, 200, "write should return 200, got {}", res.status);

    let file = tmp.path().join("hello.txt");
    assert!(file.exists(), "file must exist immediately after write returns 200");

    // Let the watcher debounce-flush and the reverse-write loop churn.
    tokio::time::sleep(Duration::from_secs(2)).await;

    assert!(
        file.exists(),
        "REGRESSION: hello.txt was created then DELETED within 2s of a 200 write \
         (this is the Tori upload bug — reverse-delete chasing a fresh write)"
    );
    assert_eq!(
        std::fs::read(&file).unwrap(),
        b"hello upload",
        "on-disk content must match what was written"
    );
}

/// THE BUG, isolated in-process. Identical to the test above except the fs
/// **watcher is running** — the one ingredient that the live Tauri backend has
/// (its root config is persisted in SQLite, so the handler's `load()`
/// auto-starts a watcher on the 2nd+ run) and that fresh in-memory peers lack.
///
/// Root cause it pins: `handle_write` writes every file via `atomic_write`
/// (temp file + `std::fs::rename` into place). On Linux the rename destination
/// fires inotify `IN_MOVED_TO` → notify `Modify(Name(To))`, and
/// `watcher::classify` maps ANY `Modify(Name(_))` to `FsEvent::Deleted`. So the
/// watcher records the freshly-written file's *arrival* as a *deletion*, and
/// `flush_pending` removes the tree entity (without checking the file still
/// exists) → a `Deleted` tree event → the reverse-writer `remove_file`s it.
///
/// This test FAILS on the current code (file deleted ~debounce after the write)
/// and is the regression guard for the classify/flush fix.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_then_watcher_debounce_does_not_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let kp = Keypair::from_seed([11u8; 32]);
    let pid = kp.peer_id().to_string();

    let peer = PeerBuilder::new()
        .keypair(kp)
        .config(PeerConfig { debug_open_grants: true, ..PeerConfig::default() })
        .build()
        .expect("peer builds");
    let shared = peer.shared();
    peer.start_engines(&shared);

    peer.local_files_handler()
        .add_root(
            "shared",
            RootConfigData {
                prefix: "local/files/shared/".to_string(),
                filesystem_root: tmp.path().to_string_lossy().into_owned(),
                read_only: false,
                ..Default::default()
            },
        )
        .expect("add root");

    // The live-backend ingredient: start the fs watcher on the root (short
    // debounce to keep the test fast). This is exactly what `load()` does on a
    // rehydrated root in the Tauri backend.
    peer.local_files_handler()
        .start_watcher("shared", 300)
        .expect("start watcher");
    // Let the watcher's initial scan settle before we write.
    tokio::time::sleep(Duration::from_millis(400)).await;

    let target = format!("/{}/local/files/shared/hello.txt", pid);
    let opts = ExecuteOptions {
        resource: Some(ResourceTarget { targets: vec![target], exclude: vec![] }),
        ..Default::default()
    };
    let res = peer
        .execute_with_options("local/files", "write", write_params(b"hello upload"), opts)
        .await
        .expect("write dispatches");
    assert_eq!(res.status, 200, "write should return 200, got {}", res.status);

    let file = tmp.path().join("hello.txt");
    assert!(file.exists(), "file must exist immediately after write returns 200");

    // Wait past the debounce window so the watcher flushes its (mis)classified
    // rename-to-Deleted and the reverse-writer acts on it.
    tokio::time::sleep(Duration::from_secs(1)).await;

    assert!(
        file.exists(),
        "THE UPLOAD-VANISHES BUG: with the fs watcher running, hello.txt was \
         created by a 200 write then DELETED within ~debounce — the watcher \
         misclassified the atomic_write rename-into-place (Modify(Name(To))) as \
         a delete and the reverse-writer removed it. Fix: classify Name(To) as \
         Created/Updated (or guard flush's Deleted branch on file non-existence)."
    );
    assert_eq!(std::fs::read(&file).unwrap(), b"hello upload");
}

/// The faithful reproduction of the phone→backend path: peer A writes to peer
/// B's `local/files` over a (memory) wire. Single-peer persists (test above),
/// so the delete is cross-peer specific.
///
/// The earlier `#[ignore]` was a workaround for Bug B, NOT a limitation of the
/// harness: raw `peer_a.connect_to()` / `peer_a.execute_with_options()` each
/// call `self.shared()` internally, and every `Peer::shared()` mints a *new*
/// `PeerShared` with a fresh empty `RemoteState` (`core/peer/src/lib.rs:278`).
/// So `connect_to` pooled the connection into one throwaway snapshot and
/// `execute_with_options` looked in another → "no transport profile".
///
/// The fix here mirrors exactly what `PeerContext::connect_to` does in the SDK
/// (`bindings/sdk/src/sdk.rs:3861`) and what the app relies on: build ONE
/// `shared` for A, hold it, and dial + dispatch against that same persistent
/// pool via the public `entity_peer` primitives. No SDK dependency needed —
/// this is the minimal reproduction of the app's single-`shared` discipline.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cross_peer_write_persists_on_backend() {
    run_cross_peer_write(false, false).await;
}

/// Same reproduction, but B is built with a **SQLite-backed tree** — exactly
/// how the Tauri desktop backend peer is built (`src-tauri/src/lib.rs`
/// `.sqlite(db_path)`), which the in-memory variant above does NOT exercise.
/// This is the first production ingredient the minimal repro was missing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cross_peer_write_persists_on_backend_sqlite() {
    run_cross_peer_write(true, false).await;
}

/// The closest in-process reproduction of the desktop path: SQLite-backed B +
/// a **real WebSocket** transport (the Tauri backend binds a `WebSocketListener`
/// and clients dial `ws://…`, `src-tauri/src/lib.rs`). This exercises real
/// async framing / handshake timing that the in-memory transport elides — the
/// last structural difference from production that is testable in-process
/// (the remaining one being the phone's Worker/IDB arm).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cross_peer_write_persists_on_backend_sqlite_ws() {
    run_cross_peer_write(true, true).await;
}

async fn run_cross_peer_write(backend_sqlite: bool, real_ws: bool) {
    use std::collections::HashMap;
    use std::sync::Arc;
    use entity_peer::transport::{
        Connector, MemoryConnector, MemoryListener, MemoryTransportRegistry,
        WebSocketConnector, WebSocketListener,
    };

    // --- Peer B (backend): writable local/files root + listener ---
    let tmp = tempfile::tempdir().unwrap();      // the shared FS root
    let db_dir = tempfile::tempdir().unwrap();   // SQLite tree store (if enabled)
    let registry = MemoryTransportRegistry::new();

    let kp_b = Keypair::from_seed([9u8; 32]);
    let pid_b = kp_b.peer_id().to_string();
    let mut builder_b = PeerBuilder::new()
        .keypair(kp_b)
        .config(PeerConfig { debug_open_grants: true, ..PeerConfig::default() });
    if backend_sqlite {
        builder_b = builder_b
            .sqlite(db_dir.path().join("backend.sqlite"))
            .expect("open sqlite store");
    }
    let peer_b = builder_b.build().expect("peer B builds");
    let shared_b = peer_b.shared();
    peer_b.start_engines(&shared_b);
    peer_b
        .local_files_handler()
        .add_root(
            "shared",
            RootConfigData {
                prefix: "local/files/shared/".to_string(),
                filesystem_root: tmp.path().to_string_lossy().into_owned(),
                read_only: false,
                ..Default::default()
            },
        )
        .expect("add root");

    // Bind the listener (memory or real WS) and record the address A dials.
    let shared_b_run = shared_b.clone();
    let connect_addr = if real_ws {
        let listener = WebSocketListener::bind("127.0.0.1:0").await.expect("ws bind");
        let addr = format!("ws://{}", listener.socket_addr());
        tokio::spawn(async move {
            let _ = entity_peer::server::run(listener, shared_b_run).await;
        });
        addr
    } else {
        let listener = MemoryListener::bind("backend", registry.clone()).expect("bind");
        tokio::spawn(async move {
            let _ = entity_peer::server::run(listener, shared_b_run).await;
        });
        "memory://backend".to_string()
    };

    // --- Peer A (client): ONE cached `shared`, held for the whole test ---
    // (This is the discipline the SDK/app enforce; the raw `Peer::connect_to`
    // + `execute_with_options` helpers do NOT, which is Bug B.)
    let connector: Arc<dyn Connector> = if real_ws {
        Arc::new(WebSocketConnector)
    } else {
        Arc::new(MemoryConnector::new(registry.clone()))
    };
    let peer_a = PeerBuilder::new()
        .keypair(Keypair::from_seed([1u8; 32]))
        .config(PeerConfig { debug_open_grants: true, ..PeerConfig::default() })
        .connector(connector)
        .build()
        .expect("peer A builds");
    let shared_a = peer_a.shared();
    peer_a.start_engines(&shared_a);

    // Dial + handshake against the PERSISTENT shared, then pool the connection
    // into it — the exact body of `PeerContext::connect_to`.
    let conn = shared_a
        .connector
        .connect(&connect_addr)
        .await
        .expect("A dials B");
    let remote = entity_peer::remote::perform_connect_with_dispatch(
        conn,
        &shared_a.keypair,
        shared_a.config.home_hash_format,
        Some(shared_a.clone()),
        // §4.4: dial-by-address test harness — no §3 rendezvous key, so no
        // reciprocal grant (asymmetric establishment, §6.6).
        false,
    )
    .await
    .expect("A handshakes B");
    assert_eq!(remote.remote_peer_id, pid_b, "connected to B");
    let remote_pid = remote.remote_peer_id.clone();
    shared_a.remote.insert(&remote_pid, remote);

    // Cross-peer write via an execute_fn bound to that SAME shared, so the
    // pooled connection is the one dispatch reads from.
    let execute_fn = entity_peer::connection::make_execute_fn(
        shared_a.clone(),
        Some(shared_a.identity_hash),
        HashMap::new(),
        None,
        None,
    );
    let target = format!("/{}/local/files/shared/hello.txt", pid_b);
    let opts = ExecuteOptions {
        resource: Some(ResourceTarget {
            targets: vec![target],
            exclude: vec![],
        }),
        ..Default::default()
    };
    let res = execute_fn(
        format!("entity://{}/local/files", pid_b),
        "write".to_string(),
        write_params(b"hello upload"),
        opts,
    )
    .await
    .expect("cross-peer write dispatches");
    assert_eq!(res.status, 200, "cross-peer write should return 200, got {}", res.status);

    let file = tmp.path().join("hello.txt");
    assert!(file.exists(), "file must exist immediately after cross-peer write");

    // Let the wire's post-handler processing + watcher/reverse-write churn.
    tokio::time::sleep(Duration::from_secs(2)).await;

    assert!(
        file.exists(),
        "REGRESSION (THE TORI BUG): cross-peer write created hello.txt then it was \
         DELETED within 2s — reverse-delete chasing a fresh cross-peer write"
    );
    assert_eq!(std::fs::read(&file).unwrap(), b"hello upload");
}

/// Live "call the cut over WebSockets": connect to an ALREADY-RUNNING backend
/// peer over real WS and issue the exact upload the app issues (write + a
/// read-back verify + a re-read after a delay), printing each status so the run
/// itself shows whether the file survives. This is the faithful client-side of
/// the phone→backend path minus only the browser Worker arm, and it is the
/// reproduction that caught the watcher self-delete (write=200, read=200,
/// re-read after 3s=404 pre-fix; =200 post-fix).
///
/// Skipped unless `LIVE_BACKEND_WS` is set. Drive it against the live backend
/// (`ws_addr` is printed on the `ENTITY_BACKEND_LISTENER_READY` line):
/// ```text
/// LIVE_BACKEND_WS=ws://127.0.0.1:PORT \
///   cargo test --test local_files_write_persistence \
///   live_upload_against_running_backend -- --ignored --nocapture
/// ```
/// Then `ls` the backend's share dir to confirm the file is (still) there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a live backend; set LIVE_BACKEND_WS"]
async fn live_upload_against_running_backend() {
    use std::collections::HashMap;
    use std::sync::Arc;
    use entity_peer::transport::WebSocketConnector;

    let ws = match std::env::var("LIVE_BACKEND_WS") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("LIVE_BACKEND_WS not set — skipping live upload repro");
            return;
        }
    };
    let fname = std::env::var("LIVE_UPLOAD_NAME").unwrap_or_else(|_| "suze-test.txt".to_string());
    let body = std::env::var("LIVE_UPLOAD_BODY")
        .unwrap_or_else(|_| "Suze says hi from inside the portal".to_string());

    let peer_a = PeerBuilder::new()
        .keypair(Keypair::from_seed([42u8; 32]))
        .config(PeerConfig { debug_open_grants: true, ..PeerConfig::default() })
        .connector(Arc::new(WebSocketConnector))
        .build()
        .expect("peer A builds");
    let shared_a = peer_a.shared();
    peer_a.start_engines(&shared_a);

    println!("LIVE: dialing {ws} ...");
    let conn = shared_a.connector.connect(&ws).await.expect("dial live backend");
    let remote = entity_peer::remote::perform_connect_with_dispatch(
        conn,
        &shared_a.keypair,
        shared_a.config.home_hash_format,
        Some(shared_a.clone()),
        // §4.4: dial-by-address test harness — no §3 rendezvous key, so no
        // reciprocal grant (asymmetric establishment, §6.6).
        false,
    )
    .await
    .expect("handshake with live backend");
    let pid_b = remote.remote_peer_id.clone();
    println!("LIVE: connected to backend {pid_b}");
    shared_a.remote.insert(&pid_b, remote);

    let execute_fn = entity_peer::connection::make_execute_fn(
        shared_a.clone(),
        Some(shared_a.identity_hash),
        HashMap::new(),
        None,
        None,
    );
    let path = format!("/{}/local/files/shared/{}", pid_b, fname);
    let mk_opts = || ExecuteOptions {
        resource: Some(ResourceTarget { targets: vec![path.clone()], exclude: vec![] }),
        ..Default::default()
    };

    println!("LIVE: WRITE {} bytes → {}", body.len(), path);
    let res = execute_fn(
        format!("entity://{}/local/files", pid_b),
        "write".to_string(),
        write_params(body.as_bytes()),
        mk_opts(),
    )
    .await
    .expect("cross-peer write dispatches");
    println!("LIVE: write status = {}", res.status);

    tokio::time::sleep(Duration::from_millis(500)).await;
    let rd = execute_fn(
        format!("entity://{}/local/files", pid_b),
        "read".to_string(),
        Entity::new("system/empty", entity_ecf::to_ecf(&entity_ecf::Value::Null)).unwrap(),
        mk_opts(),
    )
    .await
    .expect("cross-peer read dispatches");
    println!("LIVE: read-back status = {} (200 ⇒ file is on disk right now)", rd.status);

    // Give any reverse-delete / reconcile / session churn time to fire.
    println!("LIVE: waiting 3s for any delete churn ...");
    tokio::time::sleep(Duration::from_secs(3)).await;

    let rd2 = execute_fn(
        format!("entity://{}/local/files", pid_b),
        "read".to_string(),
        Entity::new("system/empty", entity_ecf::to_ecf(&entity_ecf::Value::Null)).unwrap(),
        mk_opts(),
    )
    .await
    .expect("second read dispatches");
    println!("LIVE: read status after 3s = {} (404 ⇒ file was deleted)", rd2.status);
    println!("LIVE: done — inspect backend stdout for FILE-XFER lines + `ls` the share dir for {fname}");
}
