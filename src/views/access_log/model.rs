//! Access Log model — a thin reader over the app-tier
//! [`crate::access_log_store`] global ring.
//!
//! The store is fed at the `ops::execute` chokepoint (every app-issued execute,
//! local OR remote — see the store's module doc for why not the per-peer inspect
//! sink). The window subscribes its dirty flag to the store so a new access
//! re-renders it; this model just snapshots the ring on render.
//!
//! Two views over the same stream: the live activity log (`render_output`) and
//! the **observed-capability map** (`capability_map`) — per acting peer, the
//! distinct grants it exercised = the minimal grant it'd need under enforcement
//! (PLAN-OF-RECORD-capability-enforcement.md §4). Still ephemeral per session; a
//! durable, subscribable map is the next step.

use std::collections::HashMap;

use crate::access_log_store::{self, AccessStore};
use crate::peers::Peers;
use crate::window::WindowId;

use super::output::{
    subject_key, AccessLogOutput, CapabilityMapOutput, DirectionFilter, ObservedGrant,
    PeerCapabilities, PeerOption,
};

/// Stable subject key for inbound rows when the System backend peer id isn't
/// resolvable yet (it registers asynchronously at boot). Base58 peer ids never
/// start with `@`, so this can't collide with a real actor.
pub const SYSTEM_BACKEND_KEY: &str = "@system-backend";

pub struct AccessLogModel {
    store: AccessStore,
}

impl AccessLogModel {
    // `window_id`/`peer_id` accepted for factory-signature parity, not stored —
    // the access store is app-global, not per-peer (a file transfer's actor and
    // target are both recorded, whichever peer this window is bound to).
    pub fn new(_window_id: WindowId, _peer_id: String) -> Self {
        Self {
            store: access_log_store::global().clone(),
        }
    }

    pub fn store(&self) -> &AccessStore {
        &self.store
    }

    /// Snapshot the ring (newest first), attribute each row to its **subject
    /// peer** (whose access log it is), build the peer dropdown, and narrow to the
    /// active direction + peer filters. Filtering here (not in the DOM) keeps the
    /// rebuilt row count proportional to what's shown — the point of the dropdowns
    /// when one peer/direction floods.
    ///
    /// Peers is needed to resolve the System backend peer (so inbound rows read as
    /// "Native backend", not a raw caller) and to label each subject.
    pub fn render_output(
        &self,
        peers: &Peers,
        direction: DirectionFilter,
        peer_filter: &str,
    ) -> AccessLogOutput {
        let all = self.store.snapshot_newest_first();
        let ctx = self.label_ctx(peers);

        // Label every distinct subject present, preserving first-seen order but
        // floating the two named system peers to the top of the dropdown.
        let mut labels: HashMap<String, String> = HashMap::new();
        let mut order: Vec<String> = Vec::new();
        for e in &all {
            let key = subject_key(e, &ctx.backend_key);
            if !labels.contains_key(&key) {
                labels.insert(key.clone(), ctx.label(peers, &key));
                order.push(key);
            }
        }
        order.sort_by_key(|k| subject_rank(k, &ctx.system_pid, &ctx.backend_key));
        let peer_options: Vec<PeerOption> = order
            .iter()
            .map(|k| PeerOption { key: k.clone(), label: labels[k].clone() })
            .collect();

        let backend_key = ctx.backend_key.clone();
        let entries = all
            .into_iter()
            .filter(|e| direction.matches(e.direction))
            .filter(|e| peer_filter.is_empty() || subject_key(e, &backend_key) == peer_filter)
            .collect();

        AccessLogOutput {
            entries,
            direction,
            peer_filter: peer_filter.to_string(),
            peer_options,
            backend_key,
            labels,
        }
    }

    /// Aggregate the retained access stream into the **observed-capability map**:
    /// per acting peer (the grantee), the distinct `(target, handler, operation,
    /// resource)` tuples it exercised — i.e. the minimal grant it would need under
    /// enforcement. This is the analytical projection of the log (a dedup), and the
    /// input to the enforcement decision (observed vs. authored grant). Bounded by
    /// the store's ring today; durability + the resource-capture gap are tracked in
    /// PLAN-OF-RECORD-capability-enforcement.md §3.
    pub fn capability_map(&self, peers: &Peers) -> CapabilityMapOutput {
        let all = self.store.snapshot_newest_first();
        let ctx = self.label_ctx(peers);

        // actor → its grants (in first-seen order), with an index for dedup.
        let mut actor_order: Vec<String> = Vec::new();
        let mut grants: HashMap<String, Vec<ObservedGrant>> = HashMap::new();
        // (actor, target, handler, operation, resource) → index into grants[actor].
        let mut index: HashMap<(String, String, String, String, String), usize> = HashMap::new();

        for e in &all {
            let target_id = e.target_peer.clone().unwrap_or_default();
            let resource = e.resource.clone().unwrap_or_default();
            let k = (
                e.actor.clone(),
                target_id.clone(),
                e.handler.clone(),
                e.operation.clone(),
                resource.clone(),
            );
            let denied = e.outcome == crate::access_log_store::AccessOutcome::Denied;
            if let Some(&i) = index.get(&k) {
                let g = &mut grants.get_mut(&e.actor).unwrap()[i];
                g.count += 1;
                g.any_denied |= denied;
                continue;
            }
            if !grants.contains_key(&e.actor) {
                grants.insert(e.actor.clone(), Vec::new());
                actor_order.push(e.actor.clone());
            }
            let list = grants.get_mut(&e.actor).unwrap();
            index.insert(k, list.len());
            list.push(ObservedGrant {
                target_label: e.target_peer.as_deref().map(|t| ctx.label(peers, t)),
                handler: e.handler.clone(),
                operation: e.operation.clone(),
                resource: e.resource.clone().filter(|r| !r.is_empty()),
                count: 1,
                any_denied: denied,
            });
        }

        // System peer first, System backend second, then the rest — same ordering
        // as the log's peer dropdown, for a predictable read.
        actor_order.sort_by_key(|a| subject_rank(a, &ctx.system_pid, &ctx.backend_key));
        let peers_out = actor_order
            .into_iter()
            .map(|actor| {
                let mut gs = grants.remove(&actor).unwrap_or_default();
                gs.sort_by(|a, b| {
                    (&a.handler, &a.operation, &a.resource)
                        .cmp(&(&b.handler, &b.operation, &b.resource))
                });
                PeerCapabilities {
                    actor_label: ctx.label(peers, &actor),
                    authorized: authored_grant(peers, &ctx, &actor),
                    actor_key: actor,
                    grants: gs,
                }
            })
            .collect();

        CapabilityMapOutput { peers: peers_out }
    }

    /// Resolve the label context once (the System backend peer + persisted modes),
    /// shared by the log view and the capability map so both name peers the same.
    fn label_ctx(&self, peers: &Peers) -> LabelCtx {
        // The System backend peer (System + Native), if provisioned — the same
        // structural test System Overview uses (no magic label).
        let modes = crate::persistence::peer_modes();
        let backend_pid = crate::peer_registry::read_registry(peers)
            .into_iter()
            .map(|r| r.peer_id)
            .find(|pid| {
                let d = crate::peer_display::PeerDescriptor::describe(peers, pid, &modes);
                d.role == crate::peer_display::PeerRole::System
                    && d.runtime == crate::peer_display::PeerRuntime::Native
            });
        let backend_key = backend_pid
            .clone()
            .unwrap_or_else(|| SYSTEM_BACKEND_KEY.to_string());
        LabelCtx {
            system_pid: peers.system_peer_id().to_string(),
            backend_pid,
            backend_key,
            modes,
        }
    }
}

/// Resolved label context: the System backend peer id (if any), the key inbound
/// rows attribute to, and the persisted modes — everything `label_for` needs.
struct LabelCtx {
    system_pid: String,
    backend_pid: Option<String>,
    backend_key: String,
    modes: HashMap<String, crate::peer_mode::PeerMode>,
}

impl LabelCtx {
    fn label(&self, peers: &Peers, key: &str) -> String {
        label_for(peers, key, &self.system_pid, self.backend_pid.as_deref(), &self.modes)
    }
}

/// Read a peer's **authored** grant on the System backend (its profile from the
/// `authz` mirror, expanded to bits), for the observed-vs-authorized pairing.
/// `None` when no grant is recorded for the peer (our own system peers; an
/// un-authorized device; or an unrecognized profile token). `read_authz` handles
/// the hex/base58 id reconciliation, so the raw actor key resolves either form.
fn authored_grant(
    peers: &Peers,
    ctx: &LabelCtx,
    actor: &str,
) -> Option<super::output::AuthoredGrant> {
    let token = crate::connections::read_authz(peers, &ctx.system_pid, actor)?;
    let profile = crate::backend_auth::GrantProfile::from_token(&token)?;
    let backend = ctx.backend_pid.as_deref().unwrap_or("<backend>");
    let view = profile.grant_view(backend);
    Some(super::output::AuthoredGrant {
        profile_label: profile.label().to_string(),
        summary: profile.scope_summary().to_string(),
        handlers: view.handlers,
        resources: view.resources,
        operations: view.operations,
    })
}

/// A friendly label for a subject peer — the two named system peers get plain,
/// operator-legible names; anything else falls back to its descriptor + short id.
fn label_for(
    peers: &Peers,
    key: &str,
    system_pid: &str,
    backend_pid: Option<&str>,
    modes: &HashMap<String, crate::peer_mode::PeerMode>,
) -> String {
    // Canonical UI names (see docs/architecture/specs/TERMINOLOGY-AND-WINDOWS.md):
    // the frontend peer is the "System peer"; the native backend is the
    // "System backend". One name each, used on every surface.
    if key == SYSTEM_BACKEND_KEY || Some(key) == backend_pid {
        return crate::i18n::t("accesslog.actor_system_backend", &[]);
    }
    if key == system_pid {
        return crate::i18n::t("accesslog.actor_system_peer", &[]);
    }
    let d = crate::peer_display::PeerDescriptor::describe(peers, key, modes);
    format!("{} · {}", d.role_name(), crate::views::short_pid(key))
}

/// Sort key: system peer first, native backend second, everyone else after (by
/// key) — a stable, predictable dropdown ordering.
fn subject_rank(key: &str, system_pid: &str, backend_key: &str) -> (u8, String) {
    if key == system_pid {
        (0, String::new())
    } else if key == backend_key {
        (1, String::new())
    } else {
        (2, key.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access_log_store::{AccessDirection, AccessEntry, AccessOutcome};

    fn entry(actor: &str, op: &str) -> AccessEntry {
        AccessEntry {
            direction: AccessDirection::Local,
            actor: actor.into(),
            target_peer: None,
            handler: "system/tree".into(),
            operation: op.into(),
            resource: None,
            outcome: AccessOutcome::Allowed,
            detail: "status 200".into(),
        }
    }

    fn inbound(actor: &str, op: &str) -> AccessEntry {
        AccessEntry {
            direction: AccessDirection::Inbound,
            actor: actor.into(),
            target_peer: None,
            handler: "local/files".into(),
            operation: op.into(),
            resource: None,
            outcome: AccessOutcome::Allowed,
            detail: "status 200".into(),
        }
    }

    #[test]
    fn render_output_reflects_the_store() {
        // The model reads the app-global store, shared across parallel tests, so
        // assert on presence of a uniquely-tagged row rather than exact counts.
        let peers = Peers::new_direct();
        let model = AccessLogModel::new(1, "peer".into());
        let uniq = "op-model-render-test";
        model.store().record(entry("PEER_S", uniq));
        let rows = model.render_output(&peers, DirectionFilter::All, "").entries;
        assert!(
            rows.iter().any(|e| e.operation == uniq && e.actor == "PEER_S"),
            "recorded row should be visible in render_output"
        );
    }

    #[test]
    fn direction_filter_narrows_to_one_direction() {
        let peers = Peers::new_direct();
        let model = AccessLogModel::new(1, "peer".into());
        // A uniquely-tagged Local row (the app-global store is shared across
        // parallel tests, so assert on the tag, not counts).
        let uniq = "op-filter-local-test";
        model.store().record(entry("PEER_S", uniq)); // entry() is Local
        // Inbound must not appear when filtering to Local.
        let local = model.render_output(&peers, DirectionFilter::Local, "").entries;
        assert!(local.iter().any(|e| e.operation == uniq), "Local row shows under Local");
        let inbound = model.render_output(&peers, DirectionFilter::Inbound, "").entries;
        assert!(
            !inbound.iter().any(|e| e.operation == uniq),
            "a Local row must not appear under the Inbound filter"
        );
    }

    #[test]
    fn peer_filter_narrows_to_one_subject_and_inbound_buckets_to_backend() {
        let peers = Peers::new_direct();
        let model = AccessLogModel::new(1, "peer".into());
        let out_op = "op-peer-out-test";
        let in_op = "op-peer-in-test";
        model.store().record(entry("PEER_ACTOR_X", out_op)); // Local, subject = actor
        model.store().record(inbound("REMOTE_CALLER", in_op)); // Inbound, subject = backend

        // Inbound rows are attributed to the native-backend key, not the caller.
        let out = model.render_output(&peers, DirectionFilter::All, &model_backend_key());
        assert!(out.entries.iter().any(|e| e.operation == in_op), "inbound shows under the backend subject");
        assert!(
            !out.entries.iter().any(|e| e.operation == out_op),
            "a local actor's row must not appear under the backend subject"
        );

        // Filtering to the local actor shows its row, not the inbound one.
        let byactor = model.render_output(&peers, DirectionFilter::All, "PEER_ACTOR_X");
        assert!(byactor.entries.iter().any(|e| e.operation == out_op));
        assert!(!byactor.entries.iter().any(|e| e.operation == in_op));
    }

    // No native backend is registered in this unit test, so inbound buckets under
    // the sentinel key.
    fn model_backend_key() -> String {
        SYSTEM_BACKEND_KEY.to_string()
    }

    #[test]
    fn capability_map_dedups_by_grant_and_counts_uses() {
        let peers = Peers::new_direct();
        let model = AccessLogModel::new(1, "peer".into());
        // A unique actor so the app-global store shared across parallel tests
        // doesn't pollute the assertion.
        let actor = "CAP_MAP_ACTOR_1";
        let mk = |op: &str, res: Option<&str>, outcome: AccessOutcome| AccessEntry {
            direction: AccessDirection::Outbound,
            actor: actor.into(),
            target_peer: Some("TARGET_B".into()),
            handler: "local/files".into(),
            operation: op.into(),
            resource: res.map(|r| r.into()),
            outcome,
            detail: String::new(),
        };
        // Same (target, handler, op, resource) three times → one grant, count 3.
        model.store().record(mk("list", None, AccessOutcome::Allowed));
        model.store().record(mk("list", None, AccessOutcome::Allowed));
        model.store().record(mk("list", None, AccessOutcome::Denied));
        // A distinct resource → a separate grant.
        model.store().record(mk("read", Some("a.txt"), AccessOutcome::Allowed));

        let map = model.capability_map(&peers);
        let me = map
            .peers
            .iter()
            .find(|p| p.actor_key == actor)
            .expect("actor present in the map");
        assert_eq!(me.grants.len(), 2, "two distinct grants (list; read a.txt)");

        let list = me.grants.iter().find(|g| g.operation == "list").unwrap();
        assert_eq!(list.count, 3, "the three identical list ops collapse to count 3");
        assert!(list.any_denied, "a denied occurrence flags the grant");
        assert_eq!(list.resource, None);

        let read = me.grants.iter().find(|g| g.operation == "read").unwrap();
        assert_eq!(read.count, 1);
        assert_eq!(read.resource.as_deref(), Some("a.txt"));
        assert!(!read.any_denied);
    }

    #[test]
    fn capability_map_pairs_the_authored_grant() {
        use crate::connections::ConnectionsWriter;
        let peers = Peers::new_direct();
        let model = AccessLogModel::new(1, "peer".into());
        let device = "DEVICE_CAP_AUTH_1";
        // Observed activity for the device...
        model.store().record(AccessEntry {
            direction: AccessDirection::Inbound,
            actor: device.into(),
            target_peer: None,
            handler: "local/files".into(),
            operation: "list".into(),
            resource: None,
            outcome: AccessOutcome::Allowed,
            detail: String::new(),
        });
        // ...and a recorded authorization for it.
        ConnectionsWriter::new(&peers).set_authorized(device, "file-transfer");

        let map = model.capability_map(&peers);
        let me = map.peers.iter().find(|p| p.actor_key == device).expect("device present");
        let auth = me.authorized.as_ref().expect("authored grant paired to the observed one");
        assert_eq!(auth.profile_label, "File transfer (pull only)");
        assert!(auth.handlers.contains(&"local/files".to_string()));
        assert!(auth.operations.contains(&"list".to_string()));
        assert!(auth.operations.contains(&"read".to_string()));

        // A peer with no recorded grant pairs to None.
        model.store().record(entry("UNGRANTED_ACTOR_1", "op-ungranted"));
        let map2 = model.capability_map(&peers);
        let ung = map2.peers.iter().find(|p| p.actor_key == "UNGRANTED_ACTOR_1").unwrap();
        assert!(ung.authorized.is_none(), "no grant recorded → no authored pairing");
    }
}
