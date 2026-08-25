//! Connection-health mirror — a **local, subscribable** "is this remote
//! reachable" signal, one entity per remote peer under
//! [`connection_health_prefix`](crate::app_paths::connection_health_prefix) on
//! the system peer.
//!
//! Motivation: there is no live-connection query in the app (the remembered-peer
//! registry only means "connected at least once"), so windows showed stale
//! "Reconnect" state. Rather than poll or add a manual button, we **write the
//! liveness we already observe** — a successful connect, the backend-auth probe
//! outcome — into this mirror, and windows *subscribe* to it (the subscription-
//! first design). A real transport-close event from the SDK can later feed the
//! same entity without changing any reader.
//!
//! Mirrors [`crate::backend_auth`] in shape: a plain `WriterHandle` (arm-
//! branching) writer + a tolerant entity codec.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use entity_ecf::{text, to_ecf, Value};
use entity_entity::Entity;

use crate::peers::Peers;
use crate::writer_handle::WriterHandle;

pub const CONN_HEALTH_TYPE: &str = "app/entity-browser/conn-health";

/// Last observed reachability of a remote peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Liveness {
    /// No signal yet — remembered but not confirmed either way.
    #[default]
    Unknown,
    /// A recent successful connect / dispatch / probe.
    Connected,
    /// A recent failed connect / dispatch (transport error).
    Unreachable,
}

impl Liveness {
    fn token(self) -> &'static str {
        match self {
            Liveness::Unknown => "unknown",
            Liveness::Connected => "connected",
            Liveness::Unreachable => "unreachable",
        }
    }

    fn from_token(s: &str) -> Self {
        match s {
            "connected" => Liveness::Connected,
            "unreachable" => Liveness::Unreachable,
            _ => Liveness::Unknown,
        }
    }
}

/// One remote peer's health record — the projection stored in the mirror.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConnHealth {
    pub remote_pid: String,
    pub liveness: Liveness,
    /// Optional detail (e.g. the transport error on `Unreachable`).
    pub detail: Option<String>,
}

impl ConnHealth {
    pub fn to_entity(&self) -> Entity {
        let mut fields = vec![
            (text("remote"), text(&self.remote_pid)),
            (text("state"), text(self.liveness.token())),
        ];
        match &self.detail {
            Some(d) => fields.push((text("detail"), text(d))),
            None => fields.push((text("detail"), Value::Null)),
        }
        let data = to_ecf(&Value::Map(fields));
        Entity::new(CONN_HEALTH_TYPE, data).expect("conn-health entity construction is infallible")
    }

    /// Tolerant decode: a malformed / bodyless entity yields `Unknown`, never a
    /// panic.
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
                Some("remote") => {
                    if let Some(s) = v.as_text() {
                        out.remote_pid = s.to_string();
                    }
                }
                Some("state") => {
                    if let Some(s) = v.as_text() {
                        out.liveness = Liveness::from_token(s);
                    }
                }
                Some("detail") => {
                    if let Some(s) = v.as_text() {
                        if !s.is_empty() {
                            out.detail = Some(s.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }
}

/// Writes health records to the local mirror. Mirrors `ConnectionsWriter` /
/// `BackendAuthWriter` — a `WriterHandle` (arm-branching) + the system peer id.
#[derive(Clone)]
pub struct ConnectionHealthWriter {
    system_peer_id: String,
    handle: Option<WriterHandle>,
    /// Last liveness written per remote pid (shared across clones). Dedupes so
    /// repeated identical states — e.g. the backend-auth probe firing every few
    /// seconds while steadily connected — don't rewrite the tree and wake every
    /// health subscriber for nothing.
    last: Arc<Mutex<HashMap<String, Liveness>>>,
}

impl ConnectionHealthWriter {
    pub fn new(peers: &Peers) -> Self {
        Self {
            system_peer_id: peers.system_peer_id().to_string(),
            handle: peers.writer_handle(),
            last: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Record a remote peer's liveness. Keyed by `remote_pid`; a later write
    /// overwrites the prior snapshot (the subscription wakes the readers). A
    /// no-op when the liveness is unchanged since the last write (dedup).
    pub fn record(&self, remote_pid: &str, liveness: Liveness, detail: Option<String>) {
        let Some(handle) = &self.handle else { return };
        {
            let mut last = self.last.lock().unwrap();
            if last.get(remote_pid) == Some(&liveness) {
                return; // unchanged — don't rewrite / wake subscribers
            }
            last.insert(remote_pid.to_string(), liveness);
        }
        let health = ConnHealth {
            remote_pid: remote_pid.to_string(),
            liveness,
            detail,
        };
        let path = crate::app_paths::connection_health_entry_path(
            crate::app_paths::APP_ID,
            &self.system_peer_id,
            remote_pid,
        );
        handle.put(path, health.to_entity());
    }
}

/// Read a remote peer's last-observed liveness from the local mirror. Returns
/// `Unknown` when there's no record. Callers must `watch_prefix` the
/// [`connection_health_prefix`](crate::app_paths::connection_health_prefix) so
/// the Worker-arm cache is seeded and the read repaints on change.
pub fn read(peers: &Peers, remote_pid: &str) -> Liveness {
    let sys = peers.system_peer_id();
    let path =
        crate::app_paths::connection_health_entry_path(crate::app_paths::APP_ID, sys, remote_pid);
    peers
        .get_entity(sys, &path)
        .map(|e| ConnHealth::from_entity(&e).liveness)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_through_entity() {
        for (lv, detail) in [
            (Liveness::Connected, None),
            (Liveness::Unreachable, Some("transport closed".to_string())),
            (Liveness::Unknown, None),
        ] {
            let h = ConnHealth {
                remote_pid: "REMOTE_X".into(),
                liveness: lv,
                detail: detail.clone(),
            };
            let decoded = ConnHealth::from_entity(&h.to_entity());
            assert_eq!(decoded.remote_pid, "REMOTE_X");
            assert_eq!(decoded.liveness, lv);
            assert_eq!(decoded.detail, detail);
        }
    }

    #[test]
    fn malformed_entity_decodes_unknown() {
        let e = Entity::new(CONN_HEALTH_TYPE, vec![0xff, 0xff]).unwrap();
        assert_eq!(ConnHealth::from_entity(&e).liveness, Liveness::Unknown);
    }

    #[test]
    fn writer_records_and_reads_back() {
        let peers = Peers::new_direct();
        let writer = ConnectionHealthWriter::new(&peers);
        writer.record("REMOTE_B", Liveness::Connected, None);
        assert_eq!(read(&peers, "REMOTE_B"), Liveness::Connected);
        writer.record("REMOTE_B", Liveness::Unreachable, Some("dial failed".into()));
        assert_eq!(read(&peers, "REMOTE_B"), Liveness::Unreachable);
        assert_eq!(read(&peers, "NEVER_SEEN"), Liveness::Unknown);
    }
}
