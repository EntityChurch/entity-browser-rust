//! End-to-end proof that the backend inbound access log works on **real** kernel
//! events — not synthetic fixtures. The src-tauri `access_log` module correlates
//! a `WireEvent` (the caller) with a `DispatchEvent` (the target + outcome) by
//! `request_id`; that correlation is only sound if the kernel actually fires both
//! hooks, with `peer_address` populated and matching `request_id`s, on a live
//! inbound EXECUTE. Its unit tests use hand-built events, so this integration
//! test closes the gap: a real B with the real hooks + a real WebSocket peer A
//! reaching into B's share.
//!
//! Mirrors the cross-peer harness in `tests/authorize_gate.rs` (real WebSocket,
//! two peers) — the Tauri WebView path needs webkit2gtk + a display; the
//! hook/correlation mechanism does not. Native only.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use entity_crypto::Keypair;
use entity_entity::Entity;
use entity_handler::ExecuteOptions;
use entity_peer::local_files::RootConfigData;
use entity_peer::transport::{Connector, WebSocketConnector, WebSocketListener};
use entity_peer::{
    DispatchEvent, DispatchPhase, PeerBuilder, PeerConfig, PeerShared, WireDirection, WireEvent,
};

fn empty_params() -> Entity {
    Entity::new("system/empty", entity_ecf::to_ecf(&entity_ecf::Value::Null)).unwrap()
}

/// One stitched inbound access — the exact tuple `src-tauri/access_log` emits.
#[derive(Debug, Clone)]
struct Row {
    caller: String,
    target_uri: String,
    operation: String,
    status: u32,
}

/// The same correlation `src-tauri/src/access_log.rs` runs, replicated here so
/// the test proves the *algorithm* on *real* events: park `request_id → caller`
/// on an inbound wire frame, emit a row on the matching dispatch exit.
#[derive(Default)]
struct Correlator {
    pending: HashMap<String, String>,
    rows: Vec<Row>,
}

impl Correlator {
    fn on_wire(&mut self, ev: &WireEvent) {
        if ev.direction == WireDirection::Recv && !ev.peer_address.is_empty() {
            self.pending.insert(ev.request_id.clone(), ev.peer_address.clone());
        }
    }
    fn on_dispatch(&mut self, ev: &DispatchEvent) {
        if let DispatchPhase::Exit { status, .. } = ev.phase {
            if let Some(caller) = self.pending.remove(&ev.request_id) {
                self.rows.push(Row {
                    caller,
                    target_uri: ev.target_uri.clone(),
                    operation: ev.operation.clone(),
                    status,
                });
            }
        }
    }
}

/// Build B with a real `shared` file root AND the two access-log hooks feeding a
/// shared `Correlator`, then a WS listener. Returns (shared, pid_b, ws addr,
/// correlator, tmpdir).
async fn build_backend_with_hooks() -> (
    Arc<PeerShared>,
    String,
    String,
    Arc<Mutex<Correlator>>,
    tempfile::TempDir,
) {
    let tmp = tempfile::tempdir().unwrap();
    // Put a file in the share so `list` has something to return (and definitely
    // reaches the handler → a clean 2xx exit).
    std::fs::write(tmp.path().join("hello.txt"), b"hi").unwrap();

    let kp_b = Keypair::from_seed([9u8; 32]);
    let pid_b = kp_b.peer_id().to_string();

    let corr = Arc::new(Mutex::new(Correlator::default()));
    let corr_wire = corr.clone();
    let corr_disp = corr.clone();

    let peer_b = PeerBuilder::new()
        .keypair(kp_b)
        // Open grants so the inbound `list` is ALLOWED and reaches the handler —
        // we're proving the hooks fire + correlate, not enforcement.
        .config(PeerConfig { debug_open_grants: true, ..PeerConfig::default() })
        .with_wire_hook("test/access-log", move |ev| {
            corr_wire.lock().unwrap().on_wire(ev);
        })
        .with_dispatch_hook("test/access-log", move |ev| {
            corr_disp.lock().unwrap().on_dispatch(ev);
        })
        .build()
        .expect("peer B builds");
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

    let shared_b_run = shared_b.clone();
    let listener = WebSocketListener::bind("127.0.0.1:0").await.expect("ws bind");
    let addr = format!("ws://{}", listener.socket_addr());
    tokio::spawn(async move {
        let _ = entity_peer::server::run(listener, shared_b_run).await;
    });
    (shared_b, pid_b, addr, corr, tmp)
}

fn build_client(seed: u8) -> Arc<PeerShared> {
    let peer_a = PeerBuilder::new()
        .keypair(Keypair::from_seed([seed; 32]))
        .config(PeerConfig { debug_open_grants: true, ..PeerConfig::default() })
        .connector(Arc::new(WebSocketConnector) as Arc<dyn Connector>)
        .build()
        .expect("peer A builds");
    let shared_a = peer_a.shared();
    peer_a.start_engines(&shared_a);
    shared_a
}

async fn dial(shared_a: &Arc<PeerShared>, addr: &str) -> String {
    let conn = shared_a.connector.connect(addr).await.expect("A dials B");
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
    let pid = remote.remote_peer_id.clone();
    shared_a.remote.insert(&pid, remote);
    pid
}

async fn probe_list(shared_a: &Arc<PeerShared>, pid_b: &str) -> Option<u32> {
    let execute_fn = entity_peer::connection::make_execute_fn(
        shared_a.clone(),
        Some(shared_a.identity_hash),
        HashMap::new(),
        None,
        None,
    );
    let opts = ExecuteOptions {
        resource: Some(entity_capability::ResourceTarget {
            targets: vec![format!("/{}/local/files/shared/", pid_b)],
            exclude: vec![],
        }),
        ..Default::default()
    };
    match execute_fn(
        format!("entity://{}/local/files", pid_b),
        "list".to_string(),
        empty_params(),
        opts,
    )
    .await
    {
        Ok(res) => Some(res.status),
        Err(_) => None,
    }
}

/// The load-bearing test: a live inbound `local/files:list` from A produces a
/// correlated access row on B carrying A's authenticated PeerID (the "who"), the
/// target URI, the operation, and the real status.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbound_execute_produces_correlated_access_row_with_caller() {
    let (_shared_b, pid_b, addr, corr, _tmp) = build_backend_with_hooks().await;
    let shared_a = build_client(7);
    let pid_a = Keypair::from_seed([7u8; 32]).peer_id().to_string();

    dial(&shared_a, &addr).await;
    let status = probe_list(&shared_a, &pid_b).await;
    assert_eq!(status, Some(200), "open-grants inbound list should be allowed");

    let rows = corr.lock().unwrap().rows.clone();
    // Exactly the row the inbound log would show: who (A), what (B's local/files
    // target), the op, and the outcome — all from REAL kernel hook events.
    let hit = rows
        .iter()
        .find(|r| r.operation == "list" && r.target_uri.contains("local/files"))
        .unwrap_or_else(|| panic!("no correlated list row; captured rows = {rows:?}"));
    assert_eq!(hit.caller, pid_a, "the row must carry A's authenticated PeerID (the who)");
    assert_eq!(hit.status, 200, "the outcome must be the real handler status");
}

/// A handshake with no dispatch (or a Send frame) must not fabricate rows — the
/// ring is inbound-dispatch-only, same as the module's guard.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_without_execute_produces_no_access_rows() {
    let (_shared_b, _pid_b, addr, corr, _tmp) = build_backend_with_hooks().await;
    let shared_a = build_client(8);
    dial(&shared_a, &addr).await;
    // No probe — only the handshake happened.
    // Give any in-flight hook a beat to run, then assert nothing was logged.
    tokio::task::yield_now().await;
    let rows = corr.lock().unwrap().rows.clone();
    assert!(rows.is_empty(), "handshake alone must not log access rows; got {rows:?}");
}
