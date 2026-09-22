//! File Transfer model.
//!
//! Slice 0 holds only ephemeral session state (selected target + filename)
//! in an `Arc<Mutex<_>>` — no tree persistence yet. What's worth persisting
//! (transfer history, remembered shares, grants) is a later-phase decision
//! (DESIGN-CROSS-DEVICE-FILE-TRANSFER §0). Results surface via the shared
//! event-log cache, populated by an async subscription the factory installs.

use std::sync::{Arc, Mutex};

use crate::peers::Peers;
use crate::views::{EventCategory, EventEntry};
use crate::window::WindowId;

use super::browse::{FsBrowseCache, FsChild};
use super::output::{FileTransferOutput, OwnOffer, TargetAccess, TargetOption};

/// Tree prefix the backend peer exposes its share at. Mirrors the backend's
/// `SHARE_PREFIX` (`src-tauri/src/lib.rs`); kept in one const here on the
/// app side. Always ends with `/`.
pub const SHARE_PREFIX: &str = "local/files/shared/";

/// Default file to pull — the backend seeds this so a fresh pairing always
/// has something to transfer.
pub const DEFAULT_FILENAME: &str = "welcome.txt";

/// Classify the target's access from the event-log results — the honest,
/// ground-truth signal. Scans newest-first for the most recent operation
/// against `entity://{target}/local/files` and reads its `status=` (the
/// `format_handler_result` prefix): a `403`/`401` (or an explicit refusal
/// word) ⇒ `Some(false)` (denied); a `2xx` ⇒ `Some(true)` (allowed). Returns
/// `None` when nothing has been tried yet (unknown — never alarm). Connection
/// errors (`✗ … → <transport error>`, no `status=`) are skipped, not treated
/// as denials.
fn classify_target_access(messages: &[String], target: &str) -> Option<bool> {
    if target.is_empty() {
        return None;
    }
    let needle = format!("entity://{}/local/files", target);
    for msg in messages.iter().rev() {
        if !msg.contains(&needle) {
            continue;
        }
        let lower = msg.to_ascii_lowercase();
        if msg.contains("status=403")
            || msg.contains("status=401")
            || lower.contains("not authorized") // i18n-ignore — log-message match key, not UI text
            || lower.contains("forbidden")
            || lower.contains("unauthorized")
        {
            return Some(false);
        }
        if msg.contains("status=2") {
            return Some(true);
        }
        // A line for this target with no decisive status (e.g. a transport
        // error) — keep scanning older results.
    }
    None
}

/// Decode a `local/files` `list` result (a `TYPE_DIRECTORY` entity) into its
/// child entries. The body is CBOR `{ path, children: [{ name, entity_path,
/// entry_type, size }], .. }` (`entity-core-rust` `local-files/types.rs`).
/// `relpath` is `entity_path` minus the share prefix so the tree roots at the
/// share. Malformed/foreign entries are skipped (best-effort, never panics).
pub(crate) fn decode_listing(data: &[u8]) -> Vec<FsChild> {
    let Ok(value) = ciborium::from_reader::<ciborium::Value, _>(data) else {
        return Vec::new();
    };
    let Some(map) = value.as_map() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (k, v) in map {
        if k.as_text() != Some("children") {
            continue;
        }
        let Some(arr) = v.as_array() else { continue };
        for child in arr {
            let Some(cm) = child.as_map() else { continue };
            let (mut name, mut entity_path, mut entry_type) =
                (String::new(), String::new(), String::new());
            let mut size = None;
            for (ck, cv) in cm {
                match (ck.as_text(), cv) {
                    (Some("name"), ciborium::Value::Text(s)) => name = s.clone(),
                    (Some("entity_path"), ciborium::Value::Text(s)) => entity_path = s.clone(),
                    (Some("entry_type"), ciborium::Value::Text(s)) => entry_type = s.clone(),
                    (Some("size"), ciborium::Value::Integer(i)) => {
                        size = u64::try_from(i128::from(*i)).ok();
                    }
                    _ => {}
                }
            }
            if name.is_empty() || entity_path.is_empty() {
                continue;
            }
            let relpath = entity_path
                .strip_prefix(SHARE_PREFIX)
                .unwrap_or(&entity_path)
                .to_string();
            out.push(FsChild {
                name,
                relpath,
                full_path: entity_path,
                is_dir: entry_type == "directory",
                size,
                // A share listing is never an offer — the two sources meet in
                // the cache, not here.
                offer_blob: None,
            });
        }
    }
    out
}

/// Every device this profile has connected to, with whether it is reachable
/// now, reachable ones first (stable within each group).
fn known_targets(peers: &Peers) -> Vec<(String, bool)> {
    let mut known: Vec<(String, bool)> = crate::connections::read_connections(peers)
        .into_iter()
        .map(|p| {
            let reachable = crate::peer_liveness::liveness_of(peers, &p.remote_pid).is_connected();
            (p.remote_pid, reachable)
        })
        .collect();
    known.sort_by_key(|(_, reachable)| !*reachable);
    known
}

/// Which device the window points at.
///
/// **A person's choice is kept even while that device is unreachable** — the
/// window says it is offline instead of silently switching to another device,
/// which would put someone else's files under a Pull button they did not aim.
/// With no choice (or a choice this profile no longer knows), the first
/// **reachable** device; only if none is reachable, the first known one. The
/// old rule took the first entry of the registry, which is sorted by id and
/// holds every peer ever met — so the default could quietly be a dead one.
pub fn pick_target(selected: &str, known: &[(String, bool)]) -> String {
    if !selected.is_empty() && known.iter().any(|(pid, _)| pid == selected) {
        return selected.to_string();
    }
    known
        .iter()
        .find(|(_, reachable)| *reachable)
        .or_else(|| known.first())
        .map(|(pid, _)| pid.clone())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Default)]
struct FileTransferState {
    /// Selected target peer id, or empty for "auto / first connected".
    selected_target: String,
    filename: String,
}

#[derive(Debug)]
pub struct FileTransferModel {
    /// Retained for the later tree-persisted window state (§0); unused
    /// while Slice 0 keeps state ephemeral.
    #[allow(dead_code)]
    window_id: WindowId,
    peer_id: String,
    inner: Arc<Mutex<FileTransferState>>,
    event_log: crate::event_log_cache::EventLogCache,
    /// Lazy remote-FS browse cache — the share rendered as a tree.
    browse: FsBrowseCache,
}

impl FileTransferModel {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        Self {
            window_id,
            peer_id,
            inner: Arc::new(Mutex::new(FileTransferState {
                selected_target: String::new(),
                filename: DEFAULT_FILENAME.to_string(),
            })),
            event_log: crate::event_log_cache::EventLogCache::new(),
            browse: FsBrowseCache::new(),
        }
    }

    /// The lazy browse cache — cloned by the window into async `list` tasks so
    /// results land back here (then a `DirtyFlag` repaints).
    pub fn browse(&self) -> &FsBrowseCache {
        &self.browse
    }

    /// Resolve the effective transfer target: an explicit selection if it is
    /// still connected, else the first connected peer, else empty. Shared by
    /// `render_output` and the window's event handler so both agree.
    pub fn effective_target(&self, peers: &Peers) -> String {
        let selected = self.inner.lock().unwrap().selected_target.clone();
        let known = known_targets(peers);
        pick_target(&selected, &known)
    }

    /// Borrow the event-log cache so the factory can install the per-event
    /// subscription that feeds the results pane.
    pub fn event_log_cache(&self) -> &crate::event_log_cache::EventLogCache {
        &self.event_log
    }

    // -- Action methods --

    pub fn select_target(&self, value: &str) {
        self.inner.lock().unwrap().selected_target = value.to_string();
    }

    pub fn set_filename(&self, value: &str) {
        self.inner.lock().unwrap().filename = value.to_string();
    }

    // -- Pure read API --

    #[allow(dead_code)] // called from the WASM render path
    pub fn render_output(
        &self,
        peers: &Peers,
        dials: &crate::dial_markers::DialMarkers,
    ) -> FileTransferOutput {
        let state = self.inner.lock().unwrap().clone();

        // Read the enriched records (not just ids) so each target carries its
        // `authorized` status from the `authz` mirror — File Transfer consumes
        // this to show the authorize affordance (§2.1). Keyed by Base58
        // `remote_pid`, reconciled to the hex-keyed mirror in read_connections.
        let connected = crate::connections::read_connections(peers);
        let known = known_targets(peers);
        let effective_target = pick_target(&state.selected_target, &known);

        // Reachable devices first, and an unreachable one says so. The list is
        // every peer this profile has ever connected to — a registry, not a
        // presence list — so without this a dead peer read the same as a live
        // one, and could be the default (field report 2026-09-15).
        let target_options: Vec<TargetOption> = known
            .iter()
            .map(|(pid, reachable)| {
                let name = crate::i18n::t(
                    "label.remote_option",
                    &[("name", &crate::views::display_name(peers, pid))],
                );
                TargetOption {
                    value: pid.clone(),
                    label: if *reachable {
                        name
                    } else {
                        format!("{name} · {}", crate::i18n::t("chip.offline", &[])) // i18n-ignore — separator between two catalog strings
                    },
                    selected: *pid == effective_target,
                }
            })
            .collect();

        // The local authz mirror (grants *we* authored as a host) — a secondary
        // positive hint only. It is NOT the client-pull gate (§2.1 caveat).
        let mirror_profile = connected
            .iter()
            .find(|p| p.remote_pid == effective_target)
            .and_then(|p| p.authorized.clone());

        let raw_messages = self.event_log.messages(peers);

        // Ground truth: derive access from what actually happened when we talked
        // to the target. A real 403/401 → Denied (show the affordance); a 2xx →
        // Authorized; no evidence → Unknown (never alarm).
        // A share listing that answered in this session outranks the log, which
        // survives a reload and can still hold a 403 from before a grant.
        let access = match if self.browse.share_root_ok() {
            Some(true)
        } else {
            classify_target_access(&raw_messages, &effective_target)
        } {
            Some(false) => TargetAccess::Denied,
            Some(true) => TargetAccess::Authorized(mirror_profile),
            None => match mirror_profile {
                Some(p) => TargetAccess::Authorized(Some(p)),
                None => TargetAccess::Unknown,
            },
        };

        // Reachability of the effective target — read, never inferred. The
        // kernel read-model is authoritative; the in-memory dial marker speaks
        // only where the kernel is silent. Deliberately NOT folded into
        // `access`: a target can be authorized and offline, or reachable and
        // refused, and collapsing the two loses whichever one the user needs.
        let target_reach = crate::peer_liveness::conn_display(
            crate::peer_liveness::liveness_of(peers, &effective_target),
            dials.hint(&effective_target),
        );

        let events: Vec<EventEntry> = raw_messages
            .into_iter()
            .map(|m| EventEntry {
                category: EventCategory::classify(&m),
                message: m,
            })
            .collect();

        // Reconcile the browse cache with the effective target (a switch clears
        // stale listings), then read the tree rows. Fetches are triggered by the
        // window's event handler (expand / Refresh), never from this read path.
        self.browse.sync_target(&effective_target);
        let tree_rows = self.browse.rows();

        FileTransferOutput {
            peer_id: self.peer_id.clone(),
            has_target: !target_options.is_empty(),
            selected_target: effective_target,
            target_options,
            share_prefix: SHARE_PREFIX.to_string(),
            filename_initial: state.filename.clone(),
            access,
            target_reach,
            tree_rows,
            root_listed: self.browse.root_listed(),
            root_loading: self.browse.root_loading(),
            selected_full_path: self.browse.selected_full_path(),
            selected_pull: self.browse.selected_pull(),
            browse_error: self.browse.error(),
            browse_unreachable: self.browse.unreachable(),
            offers_error: self.browse.offers_error(),
            share_absent: self.browse.share_absent(),
            events,
            // What we serve, read from our OWN tree (not `list_offers`, which is
            // the remote shape and would retry against ourselves). The window
            // subscribes the prefix, which is what makes this readable on the
            // Worker arm at all.
            own_offers: crate::file_offer::read_own_offers(peers, &self.peer_id)
                .into_iter()
                .map(|o| OwnOffer {
                    id: o.id(),
                    name: o.name,
                    size: o.size,
                    source: o.source.map(|s| s.name),
                })
                .collect(),
            kept_files: crate::kept_files::read_kept(peers, &self.peer_id)
                .into_iter()
                .map(|o| OwnOffer {
                    id: o.id(),
                    name: o.name,
                    size: o.size,
                    source: o.source.map(|s| s.name),
                })
                .collect(),
            offer_limit: crate::file_offer::MAX_OFFER_BYTES,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_target_access, decode_listing, pick_target};

    fn known(rows: &[(&str, bool)]) -> Vec<(String, bool)> {
        rows.iter().map(|(p, r)| (p.to_string(), *r)).collect()
    }

    #[test]
    fn with_no_choice_the_default_is_a_device_that_is_reachable() {
        let k = known(&[("LIVE", true), ("DEAD_A", false)]);
        assert_eq!(pick_target("", &k), "LIVE");
        let unsorted = known(&[("DEAD_A", false), ("LIVE", true)]);
        assert_eq!(pick_target("", &unsorted), "LIVE", "reachability decides, not registry order");
        assert_eq!(pick_target("", &known(&[("DEAD_A", false), ("DEAD_B", false)])), "DEAD_A", "none reachable: the first known");
        assert_eq!(pick_target("", &[]), "");
    }

    #[test]
    fn a_chosen_device_is_kept_while_it_is_offline() {
        let k = known(&[("LIVE", true), ("CHOSEN", false)]);
        assert_eq!(pick_target("CHOSEN", &k), "CHOSEN", "a choice is not silently switched to another device");
        assert_eq!(pick_target("FORGOTTEN", &k), "LIVE", "a choice this profile no longer knows falls back");
    }

    #[test]
    fn decode_listing_extracts_children() {
        use entity_ecf::{integer, text, to_ecf, Value};
        // The `TYPE_DIRECTORY` body shape from the local-files handler.
        let data = to_ecf(&Value::Map(vec![
            (text("path"), text("")),
            (
                text("children"),
                Value::Array(vec![
                    Value::Map(vec![
                        (text("name"), text("welcome.txt")),
                        (text("entity_path"), text("local/files/shared/welcome.txt")),
                        (text("entry_type"), text("file")),
                        (text("size"), integer(28)),
                    ]),
                    Value::Map(vec![
                        (text("name"), text("docs")),
                        (text("entity_path"), text("local/files/shared/docs")),
                        (text("entry_type"), text("directory")),
                    ]),
                ]),
            ),
        ]));
        let children = decode_listing(&data);
        assert_eq!(children.len(), 2);

        let file = children.iter().find(|c| c.name == "welcome.txt").unwrap();
        assert!(!file.is_dir);
        assert_eq!(file.relpath, "welcome.txt", "relpath is stripped to the share root");
        assert_eq!(file.full_path, "local/files/shared/welcome.txt");
        assert_eq!(file.size, Some(28));

        let dir = children.iter().find(|c| c.name == "docs").unwrap();
        assert!(dir.is_dir);
        assert_eq!(dir.relpath, "docs");
    }

    #[test]
    fn decode_listing_tolerates_garbage() {
        assert!(decode_listing(b"not cbor").is_empty());
        assert!(decode_listing(&[]).is_empty());
    }

    // The result-line shape from `format_handler_result` via `handle_execute`:
    //   `← entity://{target}/local/files {op} → status={N} type="..." ...`
    fn ok_line(t: &str) -> String {
        format!("← entity://{t}/local/files list → status=200 type=\"dir\" size=12 bytes")
    }
    fn denied_line(t: &str) -> String {
        format!("← entity://{t}/local/files list → status=403 type=\"error\" size=0 bytes")
    }

    #[test]
    fn no_activity_is_unknown() {
        assert_eq!(classify_target_access(&[], "PEER"), None);
        assert_eq!(classify_target_access(&[ok_line("OTHER")], "PEER"), None);
    }

    #[test]
    fn a_200_result_is_allowed() {
        assert_eq!(classify_target_access(&[ok_line("PEER")], "PEER"), Some(true));
    }

    #[test]
    fn a_403_result_is_denied() {
        assert_eq!(classify_target_access(&[denied_line("PEER")], "PEER"), Some(false));
    }

    #[test]
    fn most_recent_result_wins() {
        // Denied earlier, then authorized → newest (allowed) is the truth.
        let msgs = vec![denied_line("PEER"), ok_line("PEER")];
        assert_eq!(classify_target_access(&msgs, "PEER"), Some(true));
        // ...and the reverse: a fresh 403 (e.g. revoked) overrides an old OK.
        let msgs = vec![ok_line("PEER"), denied_line("PEER")];
        assert_eq!(classify_target_access(&msgs, "PEER"), Some(false));
    }

    #[test]
    fn transport_error_is_not_a_denial() {
        // A connection error line carries no `status=` — skip it, don't treat
        // it as "not authorized". With only such a line, access stays unknown.
        let msgs = vec![format!(
            "✗ entity://PEER/local/files list → connection reset by peer"
        )];
        assert_eq!(classify_target_access(&msgs, "PEER"), None);
    }

    #[test]
    fn empty_target_is_unknown() {
        assert_eq!(classify_target_access(&[ok_line("PEER")], ""), None);
    }
}
