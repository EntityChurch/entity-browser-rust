//! Backend-auth observability — the app-tier logic behind the Peer Connections
//! **authorize gate** (`DESIGN-AUTHORIZE-GATE-INCREMENT-3 §2, §2.2, §3 Step 3-4`).
//!
//! The wire and DOM layers stay thin; the real work lives here as four pure /
//! near-pure pieces, each unit-testable without a browser or a live peer:
//!
//! - [`parse_listing_keys`] — the child keys of a remote `system/tree` listing
//!   (`system/peer/session/*` and `system/capability/policy/*`), the two
//!   spec-defined prefixes the observability read fetches from backend B.
//! - [`GrantProfile`] — the named scopes (§2.2) an **Authorize** click confers,
//!   and [`build_authorize_params`], the `configure`-op params entity that
//!   authors one onto B's policy table (same body shape the kernel's
//!   connect-time union decodes — mirrors `src-tauri/src/manager_grant.rs`,
//!   which the app crate can't reuse across the IPC boundary).
//! - [`BackendAuthObservation`] — the **watchable local mirror** of a backend's
//!   derived pending/authorized rows (+ a loud error, §5). The observability
//!   read is an async one-shot `execute` against a *remote* peer; its result is
//!   written here on the local system peer so the synchronous window render can
//!   read it and a `WindowWatch` on the prefix wakes the re-render (the
//!   "subscribe, don't poll" discipline — state lives in the tree).
//! - [`BackendAuthWriter`] — writes that mirror (system-peer local; mirrors
//!   `ConnectionsWriter`).
//!
//! Keying: session/policy keys are identity-hash **hex** (see
//! `peer_auth`'s keying contract); this module treats keys as opaque strings.

#![allow(dead_code)] // wired into app.rs + the Peer Connections window (step 4)

use entity_capability::{encode_grant_entry, GrantEntry, IdScope, PathScope};
use entity_ecf::{text, to_ecf, Value};
use entity_entity::Entity;
use entity_types::{TYPE_CAP_POLICY_ENTRY, TYPE_TREE_LISTING};

use crate::peer_auth::{AuthState, PeerAuthRow};
use crate::peers::Peers;
use crate::writer_handle::WriterHandle;

/// Entity type for the backend-auth observation mirror.
pub const BACKEND_AUTH_TYPE: &str = "app/entity-browser/backend-auth";

// ---------------------------------------------------------------------------
// Listing parse
// ---------------------------------------------------------------------------

/// Extract the immediate child key names from a `system/tree` listing entity
/// (`TYPE_TREE_LISTING`, built by `handle_listing` — body
/// `{count, entries: {name: {has_children, hash}}, offset, path}`).
///
/// Used to read the peer-id keys under B's `session/*` and `policy/*`. Returns
/// the `entries` map keys, sorted; an empty vec for a wrong type or any decode
/// failure (a malformed remote result must read as "no peers", never panic).
pub fn parse_listing_keys(listing: &Entity) -> Vec<String> {
    if listing.entity_type != TYPE_TREE_LISTING {
        return Vec::new();
    }
    let value: ciborium::Value = match ciborium::from_reader(listing.data.as_slice()) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let Some(map) = value.as_map() else {
        return Vec::new();
    };
    let entries = map
        .iter()
        .find(|(k, _)| k.as_text() == Some("entries"))
        .and_then(|(_, v)| v.as_map());
    let Some(entries) = entries else {
        return Vec::new();
    };
    let mut keys: Vec<String> = entries
        .iter()
        .filter_map(|(k, _)| k.as_text().map(|s| s.to_string()))
        .filter(|s| !s.is_empty())
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

// ---------------------------------------------------------------------------
// Grant profiles (§2.2)
// ---------------------------------------------------------------------------

/// A named authorization scope conferred by one **Authorize** click (§2.2).
/// Adding exchange functionality never means hand-authoring `GrantEntry`
/// scopes at a call site — pick a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantProfile {
    /// Pull-only: `local/files` `list`+`read` on B's `shared` subtree. This is
    /// the scope proven end-to-end in `tests/authorize_gate.rs`.
    FileTransfer,
    /// Two-way: adds `write`+`delete` on the same subtree.
    FileTransferRw,
    /// Broad per-peer grant (≈ today's `debug_open_grants`, but one deliberate
    /// click on one device). Dev/iteration; build-gated out of prod (§2.2).
    Trusted,
}

impl GrantProfile {
    /// The stable wire/mirror token (matches the `authz` mirror profile string).
    pub fn as_str(&self) -> &'static str {
        match self {
            GrantProfile::FileTransfer => "file-transfer",
            GrantProfile::FileTransferRw => "file-transfer-rw",
            GrantProfile::Trusted => "trusted",
        }
    }

    /// Human-legible label for the profile picker.
    pub fn label(&self) -> &'static str {
        match self {
            GrantProfile::FileTransfer => "File transfer (pull only)",
            GrantProfile::FileTransferRw => "File transfer (two-way)",
            GrantProfile::Trusted => "Trusted (full access)",
        }
    }

    pub fn from_token(s: &str) -> Option<Self> {
        match s {
            "file-transfer" => Some(GrantProfile::FileTransfer),
            "file-transfer-rw" => Some(GrantProfile::FileTransferRw),
            "trusted" => Some(GrantProfile::Trusted),
            _ => None,
        }
    }

    /// The `GrantEntry` set this profile confers on `target_pid` over backend
    /// `backend_pid`. Four dimensions must all match per grant
    /// (`core/capability §5.4`).
    fn grants(&self, backend_pid: &str) -> Vec<GrantEntry> {
        let share = format!("/{}/local/files/shared/*", backend_pid);
        match self {
            GrantProfile::FileTransfer => vec![GrantEntry {
                handlers: PathScope::new(vec!["local/files".into()]),
                resources: PathScope::new(vec![share]),
                operations: IdScope::new(vec!["list".into(), "read".into()]),
                peers: None,
                constraints: None,
                allowances: None,
            }],
            GrantProfile::FileTransferRw => vec![GrantEntry {
                handlers: PathScope::new(vec!["local/files".into()]),
                resources: PathScope::new(vec![share]),
                operations: IdScope::new(vec![
                    "list".into(),
                    "read".into(),
                    "write".into(),
                    "delete".into(),
                ]),
                peers: None,
                constraints: None,
                allowances: None,
            }],
            // Mirrors `debug_open_grants`: handler `*`, resources `/*/*`, ops
            // `*`, peers `*` (`tests/authorize_gate.rs::wildcard_grant`).
            GrantProfile::Trusted => vec![GrantEntry {
                handlers: PathScope::new(vec!["*".into()]),
                resources: PathScope::new(vec!["/*/*".into()]),
                operations: IdScope::new(vec!["*".into()]),
                peers: Some(IdScope::new(vec!["*".into()])),
                constraints: None,
                allowances: None,
            }],
        }
    }
}

/// Build the `configure`-op params entity that authorizes `target_pid` on
/// backend `backend_pid` under `profile`. The capability handler's `configure`
/// op writes it to `system/capability/policy/{peer_pattern}` on B; the kernel
/// unions it onto `target_pid` at its **next handshake** (so the caller must
/// make the peer re-authenticate — see the design's §3 Step 4 constraint).
///
/// Body shape (matches `manager_grant` + `decode_policy_grants_at`): a `grants`
/// array plus the `peer_pattern` the `configure` writer keys the entry by.
pub fn build_authorize_params(
    backend_pid: &str,
    target_pid: &str,
    profile: GrantProfile,
) -> Result<Entity, String> {
    let arr: Vec<Value> = profile
        .grants(backend_pid)
        .iter()
        .map(encode_grant_entry)
        .collect();
    let data = to_ecf(&Value::Map(vec![
        (text("grants"), Value::Array(arr)),
        (text("peer_pattern"), text(target_pid)),
    ]));
    Entity::new(TYPE_CAP_POLICY_ENTRY, data)
        .map_err(|e| format!("authorize params construction failed: {e}"))
}

// ---------------------------------------------------------------------------
// Observation mirror
// ---------------------------------------------------------------------------

/// The derived authorization state of one backend, cached locally for the
/// synchronous window render. Either `rows` (the classified peers) or a loud
/// `error` (§5 — a read failure must never read as "no peers").
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BackendAuthObservation {
    /// Backend peer id (base58) this observation is about.
    pub backend_pid: String,
    /// A loud failure message when the remote read failed; `None` on success.
    pub error: Option<String>,
    /// Classified connected peers (pending / authorized), in `session/*` order.
    pub rows: Vec<PeerAuthRow>,
}

impl BackendAuthObservation {
    pub fn ok(backend_pid: impl Into<String>, rows: Vec<PeerAuthRow>) -> Self {
        Self {
            backend_pid: backend_pid.into(),
            error: None,
            rows,
        }
    }

    pub fn failed(backend_pid: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            backend_pid: backend_pid.into(),
            error: Some(message.into()),
            rows: Vec::new(),
        }
    }

    pub fn to_entity(&self) -> Entity {
        let rows: Vec<Value> = self
            .rows
            .iter()
            .map(|r| {
                Value::Map(vec![
                    (text("peer"), text(&r.peer_id)),
                    (text("state"), text(state_token(&r.state))),
                ])
            })
            .collect();
        let mut fields = vec![
            (text("backend"), text(&self.backend_pid)),
            (text("rows"), Value::Array(rows)),
        ];
        match &self.error {
            Some(e) => fields.push((text("error"), text(e))),
            None => fields.push((text("error"), Value::Null)),
        }
        let data = to_ecf(&Value::Map(fields));
        Entity::new(BACKEND_AUTH_TYPE, data).expect("backend-auth entity construction is infallible")
    }

    /// Tolerant decode: a malformed / bodyless entity yields an empty
    /// observation (no rows, no error), never a panic.
    pub fn from_entity(entity: &Entity) -> Self {
        let mut out = Self::default();
        let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
            Ok(v) => v,
            Err(_) => return out,
        };
        let Some(map) = value.as_map() else {
            return out;
        };
        for (k, v) in map {
            match k.as_text() {
                Some("backend") => {
                    if let Some(s) = v.as_text() {
                        out.backend_pid = s.to_string();
                    }
                }
                Some("error") => {
                    if let Some(s) = v.as_text() {
                        if !s.is_empty() {
                            out.error = Some(s.to_string());
                        }
                    }
                }
                Some("rows") => {
                    if let Some(arr) = v.as_array() {
                        for row in arr {
                            if let Some(r) = decode_row(row) {
                                out.rows.push(r);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Only the actionable rows — connected peers awaiting authorization.
    pub fn pending(&self) -> impl Iterator<Item = &PeerAuthRow> {
        self.rows.iter().filter(|r| r.state == AuthState::Pending)
    }

    /// The authorized rows.
    pub fn authorized(&self) -> impl Iterator<Item = &PeerAuthRow> {
        self.rows.iter().filter(|r| r.state == AuthState::Authorized)
    }
}

fn state_token(state: &AuthState) -> &'static str {
    match state {
        AuthState::Authorized => "authorized",
        AuthState::Pending => "pending",
    }
}

fn decode_row(row: &ciborium::Value) -> Option<PeerAuthRow> {
    let map = row.as_map()?;
    let mut peer = None;
    let mut state = None;
    for (k, v) in map {
        match k.as_text() {
            Some("peer") => peer = v.as_text().map(|s| s.to_string()),
            Some("state") => state = v.as_text().map(|s| s.to_string()),
            _ => {}
        }
    }
    let peer = peer?;
    if peer.is_empty() {
        return None;
    }
    let state = match state.as_deref() {
        Some("authorized") => AuthState::Authorized,
        // Default unknown/absent to pending — the actionable, safer reading.
        _ => AuthState::Pending,
    };
    Some(PeerAuthRow {
        peer_id: peer,
        state,
    })
}

// ---------------------------------------------------------------------------
// Writer (system-peer local mirror)
// ---------------------------------------------------------------------------

/// Writes the [`BackendAuthObservation`] mirror on the local system peer.
/// Mirrors `ConnectionsWriter` — a `WriterHandle` (arm-branching) plus the
/// system peer id. `None` handle (no writable arm) makes writes silent no-ops.
#[derive(Clone)]
pub struct BackendAuthWriter {
    system_peer_id: String,
    handle: Option<WriterHandle>,
}

impl BackendAuthWriter {
    pub fn new(peers: &Peers) -> Self {
        Self {
            system_peer_id: peers.system_peer_id().to_string(),
            handle: peers.writer_handle(),
        }
    }

    /// Record (overwrite) the observation for its backend. Idempotent on the
    /// path — a refresh replaces the prior snapshot.
    pub fn record(&self, obs: &BackendAuthObservation) {
        let Some(handle) = &self.handle else { return };
        let path = crate::app_paths::backend_auth_entry_path(
            crate::app_paths::APP_ID,
            &self.system_peer_id,
            &obs.backend_pid,
        );
        handle.put(path, obs.to_entity());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entity_ecf::integer;

    fn listing(entries: &[&str]) -> Entity {
        // Build a listing map: {count, entries:{name:{has_children,hash}}, offset, path}
        let entries_map: Vec<(Value, Value)> = entries
            .iter()
            .map(|name| {
                (
                    text(*name),
                    Value::Map(vec![
                        (text("has_children"), Value::Bool(false)),
                        (text("hash"), Value::Null),
                    ]),
                )
            })
            .collect();
        let data = to_ecf(&Value::Map(vec![
            (text("count"), integer(entries.len() as i64)),
            (text("entries"), Value::Map(entries_map)),
            (text("offset"), integer(0)),
            (text("path"), text("system/peer/session/")),
        ]));
        Entity::new(TYPE_TREE_LISTING, data).unwrap()
    }

    #[test]
    fn parse_listing_returns_sorted_child_keys() {
        let e = listing(&["CCCC", "AAAA", "BBBB"]);
        assert_eq!(parse_listing_keys(&e), vec!["AAAA", "BBBB", "CCCC"]);
    }

    #[test]
    fn parse_listing_wrong_type_is_empty() {
        let e = Entity::new("system/empty", to_ecf(&Value::Null)).unwrap();
        assert!(parse_listing_keys(&e).is_empty());
    }

    #[test]
    fn parse_listing_empty_entries_is_empty() {
        let e = listing(&[]);
        assert!(parse_listing_keys(&e).is_empty());
    }

    #[test]
    fn file_transfer_profile_is_list_read_on_share() {
        let grants = GrantProfile::FileTransfer.grants("BPID");
        assert_eq!(grants.len(), 1);
        let g = &grants[0];
        assert!(g.handlers.include.contains(&"local/files".to_string()));
        assert!(g
            .resources
            .include
            .contains(&"/BPID/local/files/shared/*".to_string()));
        assert!(g.operations.include.contains(&"list".to_string()));
        assert!(g.operations.include.contains(&"read".to_string()));
        assert!(!g.operations.include.contains(&"write".to_string()));
    }

    #[test]
    fn rw_profile_adds_write_and_delete() {
        let grants = GrantProfile::FileTransferRw.grants("BPID");
        let ops = &grants[0].operations.include;
        assert!(ops.contains(&"write".to_string()));
        assert!(ops.contains(&"delete".to_string()));
    }

    #[test]
    fn trusted_profile_is_wildcard() {
        let grants = GrantProfile::Trusted.grants("BPID");
        let g = &grants[0];
        assert!(g.handlers.include.contains(&"*".to_string()));
        assert!(g.resources.include.contains(&"/*/*".to_string()));
        assert!(g.operations.include.contains(&"*".to_string()));
        assert!(g.peers.as_ref().unwrap().include.contains(&"*".to_string()));
    }

    #[test]
    fn profile_token_round_trips() {
        for p in [
            GrantProfile::FileTransfer,
            GrantProfile::FileTransferRw,
            GrantProfile::Trusted,
        ] {
            assert_eq!(GrantProfile::from_token(p.as_str()), Some(p));
        }
        assert_eq!(GrantProfile::from_token("bogus"), None);
    }

    #[test]
    fn authorize_params_carry_grants_and_peer_pattern() {
        let e = build_authorize_params("BPID", "APID", GrantProfile::FileTransfer).unwrap();
        assert_eq!(e.entity_type, TYPE_CAP_POLICY_ENTRY);
        let val: ciborium::Value = ciborium::from_reader(e.data.as_slice()).unwrap();
        let map = val.as_map().unwrap();
        let pat = map
            .iter()
            .find(|(k, _)| k.as_text() == Some("peer_pattern"))
            .and_then(|(_, v)| v.as_text())
            .unwrap();
        assert_eq!(pat, "APID");
        let grants = map
            .iter()
            .find(|(k, _)| k.as_text() == Some("grants"))
            .and_then(|(_, v)| v.as_array())
            .unwrap();
        assert_eq!(grants.len(), 1, "one grant for the file-transfer profile");
    }

    #[test]
    fn observation_round_trips_through_entity() {
        let obs = BackendAuthObservation::ok(
            "BPID",
            vec![
                PeerAuthRow {
                    peer_id: "AAAA".into(),
                    state: AuthState::Pending,
                },
                PeerAuthRow {
                    peer_id: "BBBB".into(),
                    state: AuthState::Authorized,
                },
            ],
        );
        let decoded = BackendAuthObservation::from_entity(&obs.to_entity());
        assert_eq!(decoded, obs);
        assert_eq!(decoded.pending().count(), 1);
        assert_eq!(decoded.authorized().count(), 1);
    }

    #[test]
    fn observation_error_round_trips() {
        let obs = BackendAuthObservation::failed("BPID", "cannot read backend");
        let decoded = BackendAuthObservation::from_entity(&obs.to_entity());
        assert_eq!(decoded.error.as_deref(), Some("cannot read backend"));
        assert!(decoded.rows.is_empty());
    }

    #[test]
    fn observation_malformed_decodes_empty() {
        let e = Entity::new(BACKEND_AUTH_TYPE, vec![0xff, 0xff]).unwrap();
        let decoded = BackendAuthObservation::from_entity(&e);
        assert_eq!(decoded, BackendAuthObservation::default());
    }
}
