//! Cross-impl selection schema — slot writer for the per-panel +
//! app-aggregate selection slots.
//!
//! Mirrors workbench-go's `entitysdk.Selection` (post-Stage-5 cleanup):
//! `{path, type, peer_id, updated_at}` with `paths[]` and
//! `source_window` dropped. Entity type `app/state/selection` per
//! the guide §5 slot table.
//!
//! **§5.4 is five numbered rules, not one schema.** Which line answers each,
//! because being clean on emit is not being conformant and reading it that way
//! cost us two months (AP45): rule 1/2 (emit-side MUST NOT / MUST) — `to_entity`,
//! gated by `the_legacy_gate_is_scoped_to_the_selection_type_not_to_the_field_name`
//! decoding our own output; rule 3 (read-side MUST log a violation) —
//! [`legacy_field_violation`] and [`Selection::decode`]; rule 4 (never re-emit on
//! the round trip) — holds by construction, since a legacy field never reaches
//! [`Selection`], pinned by `a_legacy_field_is_not_re_emitted_on_the_round_trip`;
//! rule 5 (post-publication tolerance) — deferred upstream, nothing owed here.
//!
//! **Two slots, same schema.** A panel that publishes a navigate /
//! select event writes both:
//! - **Per-panel:** `{peer_id}/app/{aid}/workspace/panels/{panel_id}/selection`
//!   — the panel's own slot. Used for restoring panel state across
//!   sessions and as a stable target for panels that want to subscribe
//!   to a specific panel's selection. (Path uses `panels` per
//!   workbench-go's cross-impl rename; Rust-side type
//!   stays `WindowId` for now.)
//! - **App-aggregate:** `{peer_id}/app/{aid}/workspace/selection` —
//!   the "global" selection that other panels co-orient against. We
//!   use a flat app-aggregate (no screen layer) since we're a
//!   single-screen app today; the schema is forward-compatible with
//!   multi-screen.
//!
//! Path helpers live in `crate::app_paths`. Subscribers read via
//! `ctx.store().on_prefix_change_seeded(prefix, …)` (Direct) /
//! `Peers::observe_with_events(…)` (cross-arm normalized).

#![allow(dead_code)]

use entity_entity::Entity;

/// Entity type of both selection slots (guide §5.4).
///
/// A constant because [`legacy_field_violation`] is **scoped to this type**, not
/// to the field names it looks for: `content_type` is a retired spelling here
/// and a *required* one on `app/state/window-index`, which §4.2a calls out by
/// name. A gate written on the field alone would fire on the window index.
pub const SELECTION_TYPE: &str = "app/state/selection";

/// Why a field in a received `app/state/selection` payload is a violation, or
/// `None` if it is merely unknown.
///
/// **The `None` arm is load-bearing.** V7 §2.6 open-types means genuinely
/// unknown fields MUST still be skipped silently — this must stay a small,
/// named set plus the one namespace §5.4 closes, never "anything I don't
/// recognize".
///
/// - The three named spellings are §5.4 **rule 3**, the read-side MUST that
///   makes silent tolerance non-conformant.
/// - Any other `source_*` is a §5.4 **rule 1** violation (emit-side MUST NOT,
///   stated for the whole prefix) that we can see from the read side. Reporting
///   it is a superset of rule 3, and it is the case rule 3's fixed list cannot
///   catch: `source_panel_id` is exactly the emitter bug this exists to surface.
fn legacy_field_violation(name: &str) -> Option<&'static str> {
    match name {
        "source_window" | "source_panel" | "content_type" => {
            Some("§5.4 rule 3 — retired field; the emitter is non-conformant")
        }
        _ if name.starts_with("source_") => {
            Some("§5.4 rule 1 — no `source_*` field may be emitted on this slot")
        }
        _ => None,
    }
}

/// One legacy field found in a received selection payload, and the rule it
/// violates. Carries the rule text so the WARN line names *which* MUST was
/// broken — "retired field" and "no `source_*` at all" are two different
/// messages to the person who has to go fix the emitter (AP40).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyField {
    pub name: String,
    pub rule: &'static str,
}

/// One selection record. Stored as CBOR at the per-panel and
/// app-aggregate slots. Optional fields are omitted from the CBOR map
/// when empty/zero — matches the guide's "absence = unset"
/// convention.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    /// The selected tree path. Empty string is legal but unusual —
    /// typically a clear is represented by removing the slot entity
    /// rather than writing an empty-path Selection.
    pub path: String,
    /// What kind of pointee `path` refers to. "entity" today; future
    /// values like "query-result", "event-log-row".
    pub type_: Option<String>,
    /// Which peer's tree contains `path`. Empty when the host peer.
    pub peer_id: Option<String>,
    /// Epoch milliseconds at write time. Staleness signal for
    /// last-writer tie-breaks on the aggregate slot.
    pub updated_at: u64,
}

impl Selection {
    /// Construct an entity-pointer selection. Auto-fills `type` =
    /// `"entity"` and `updated_at` from the current time. Pass
    /// `peer_id = ""` to omit (= host peer).
    pub fn entity(path: impl Into<String>, peer_id: impl Into<String>) -> Self {
        let peer_id = peer_id.into();
        let peer_id = if peer_id.is_empty() { None } else { Some(peer_id) };
        Self {
            path: path.into(),
            type_: Some("entity".into()),
            peer_id,
            updated_at: now_epoch_ms(),
        }
    }

    /// Decode from an entity body. Tolerant of records missing
    /// optional fields. Returns a default `Selection` (empty path,
    /// zero updated_at) when the CBOR shape is unrecognized.
    ///
    /// # Legacy fields are reported, not tolerated silently — §5.4 rule 3
    ///
    /// A retired `source_window` / `source_panel` / `content_type` logs a WARN
    /// and is then dropped. We take the MUST and decline the MAY: rule 3 permits
    /// refusing such an entity outright, but refusing would turn one
    /// non-conformant emitter into a dead co-orientation surface for the reader,
    /// and the reader is not who is wrong. The field never reaches [`Selection`],
    /// so rule 4 (never re-emit on the round trip) holds by construction rather
    /// than by a second rule someone has to remember — AP44.
    ///
    /// **A stated bound: this logs once per decode, not once per offending
    /// entity.** `consume_from_source` decodes on every render pass, so a legacy
    /// entity parked in a slot warns repeatedly. That is deliberate — §5.4's
    /// stated reason for the rule is that *"silent tolerance hides
    /// non-conformant emitters that should be fixed"*, so the noise is the
    /// point, and de-duplicating would need per-call-site state this pure
    /// decoder has no business holding.
    pub fn from_entity(entity: &Entity) -> Self {
        let (sel, violations) = Self::decode(entity);
        for v in &violations {
            tracing::warn!(
                entity_type = %entity.entity_type,
                field = %v.name,
                rule = %v.rule,
                "legacy field in a received selection entity — dropping it, but the \
                 emitter needs fixing"
            );
        }
        sel
    }

    /// The decode itself, with the violations returned rather than logged.
    ///
    /// Split out so a native test can assert **what gets reported**, not merely
    /// that the offending field failed to land in the struct — the second is
    /// also true of doing nothing at all. AP44: the predicate is easy to keep
    /// right and the *wiring* is what decays, so the test has to reach the
    /// wiring, and with no `tracing-subscriber` in this crate's dev-deps a
    /// returned value is how it reaches it.
    pub fn decode(entity: &Entity) -> (Self, Vec<LegacyField>) {
        let mut violations = Vec::new();
        let value: ciborium::Value = match ciborium::from_reader(entity.data.as_slice()) {
            Ok(v) => v,
            Err(_) => return (Self::default(), violations),
        };
        let map = match value.as_map() {
            Some(m) => m,
            None => return (Self::default(), violations),
        };
        let mut sel = Self::default();
        for (k, v) in map {
            match k.as_text() {
                Some("path") => {
                    if let Some(s) = v.as_text() {
                        sel.path = s.to_string();
                    }
                }
                Some("type") => {
                    if let Some(s) = v.as_text() {
                        if !s.is_empty() {
                            sel.type_ = Some(s.to_string());
                        }
                    }
                }
                Some("peer_id") => {
                    if let Some(s) = v.as_text() {
                        if !s.is_empty() {
                            sel.peer_id = Some(s.to_string());
                        }
                    }
                }
                Some("updated_at") => {
                    sel.updated_at = v.as_integer().and_then(|i| u64::try_from(i).ok()).unwrap_or(0);
                }
                // Scoped to the selection type, never to the field name — see
                // `legacy_field_violation`. Anything else falls through to the
                // silent skip V7 open-types requires. The field is *not* kept:
                // that is rule 4 (never re-emit on the round trip) holding by
                // construction.
                Some(name) if entity.entity_type == SELECTION_TYPE => {
                    if let Some(rule) = legacy_field_violation(name) {
                        violations.push(LegacyField {
                            name: name.to_string(),
                            rule,
                        });
                    }
                }
                _ => {}
            }
        }
        (sel, violations)
    }

    /// Encode to an entity body. Optional fields omitted when
    /// empty/zero; `updated_at` always present.
    pub fn to_entity(&self) -> Entity {
        let mut pairs: Vec<(entity_ecf::Value, entity_ecf::Value)> = Vec::new();
        pairs.push((
            entity_ecf::Value::Text("path".into()),
            entity_ecf::text(&self.path),
        ));
        if let Some(ref t) = self.type_ {
            pairs.push((
                entity_ecf::Value::Text("type".into()),
                entity_ecf::text(t),
            ));
        }
        if let Some(ref p) = self.peer_id {
            pairs.push((
                entity_ecf::Value::Text("peer_id".into()),
                entity_ecf::text(p),
            ));
        }
        pairs.push((
            entity_ecf::Value::Text("updated_at".into()),
            entity_ecf::Value::Integer(self.updated_at.into()),
        ));
        let data = entity_ecf::to_ecf(&entity_ecf::Value::Map(pairs));
        Entity::new(SELECTION_TYPE, data).unwrap()
    }
}

/// Write an entity-pointer Selection to both the per-panel and the
/// app-aggregate selection slots. Used by Entity Tree on Navigate and
/// by the shell on `cd`; any future producer panel calls this too.
pub fn publish_entity_selection(
    peers: &crate::peers::Peers,
    peer_id: &str,
    window_id: crate::window::WindowId,
    path: &str,
) {
    let sel = Selection::entity(path, peer_id);
    let panel_path = crate::app_paths::panel_selection_path(
        crate::app_paths::APP_ID,
        peer_id,
        window_id,
    );
    let app_path =
        crate::app_paths::app_selection_path(crate::app_paths::APP_ID, peer_id);
    peers.dispatch_write(peer_id, panel_path, sel.to_entity());
    peers.dispatch_write(peer_id, app_path, sel.to_entity());
}

/// Remove both selection slots — used when the producer panel has
/// "no current selection" (Entity Tree on NavigateUp past the peer
/// root).
pub fn clear_entity_selection(
    peers: &crate::peers::Peers,
    peer_id: &str,
    window_id: crate::window::WindowId,
) {
    let panel_path = crate::app_paths::panel_selection_path(
        crate::app_paths::APP_ID,
        peer_id,
        window_id,
    );
    let app_path =
        crate::app_paths::app_selection_path(crate::app_paths::APP_ID, peer_id);
    peers.dispatch_remove(peer_id, panel_path);
    peers.dispatch_remove(peer_id, app_path);
}

/// Remove only the per-panel selection slot for `window_id`. Used at
/// window-close time — the app-aggregate slot stays put because another
/// open panel may have published there (single-valued slot, last-write
/// wins). Clearing it on every close would race other panels still
/// holding a current selection.
///
/// See the D9 memory-accounting audit §2.B for the rationale.
pub fn clear_panel_selection_on_close(
    peers: &crate::peers::Peers,
    peer_id: &str,
    window_id: crate::window::WindowId,
) {
    let panel_path = crate::app_paths::panel_selection_path(
        crate::app_paths::APP_ID,
        peer_id,
        window_id,
    );
    peers.dispatch_remove(peer_id, panel_path);
}

#[cfg(target_arch = "wasm32")]
fn now_epoch_ms() -> u64 {
    js_sys::Date::now() as u64
}

#[cfg(not(target_arch = "wasm32"))]
fn now_epoch_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_constructor_fills_defaults() {
        let s = Selection::entity("/p/foo", "");
        assert_eq!(s.path, "/p/foo");
        assert_eq!(s.type_.as_deref(), Some("entity"));
        assert!(s.peer_id.is_none()); // empty string => omitted
        assert!(s.updated_at > 0);
    }

    #[test]
    fn entity_constructor_keeps_peer_id_when_set() {
        let s = Selection::entity("/p/foo", "peerXYZ");
        assert_eq!(s.peer_id.as_deref(), Some("peerXYZ"));
    }

    #[test]
    fn round_trip_full() {
        let original = Selection {
            path: "/p/docs/arch".into(),
            type_: Some("entity".into()),
            peer_id: Some("p".into()),
            updated_at: 1_700_000_000_000,
        };
        let entity = original.to_entity();
        assert_eq!(entity.entity_type, "app/state/selection");
        let decoded = Selection::from_entity(&entity);
        assert_eq!(decoded, original);
    }

    #[test]
    fn round_trip_minimal() {
        let original = Selection {
            path: "/p/x".into(),
            type_: None,
            peer_id: None,
            updated_at: 42,
        };
        let entity = original.to_entity();
        let decoded = Selection::from_entity(&entity);
        assert_eq!(decoded, original);
    }

    fn selection_entity(pairs: Vec<(&str, entity_ecf::Value)>) -> Entity {
        typed_entity(SELECTION_TYPE, pairs)
    }

    fn typed_entity(ty: &str, pairs: Vec<(&str, entity_ecf::Value)>) -> Entity {
        let pairs: Vec<(entity_ecf::Value, entity_ecf::Value)> = pairs
            .into_iter()
            .map(|(k, v)| (entity_ecf::Value::Text(k.into()), v))
            .collect();
        Entity::new(ty, entity_ecf::to_ecf(&entity_ecf::Value::Map(pairs))).unwrap()
    }

    /// V7 §2.6 open-types: a field we simply do not know is skipped **without a
    /// word**. This is the half §5.4 rule 3 must not swallow — the previous
    /// single test covered this and the legacy case together, which is why the
    /// silence read as intentional for two months.
    #[test]
    fn from_entity_skips_genuinely_unknown_fields_silently() {
        let entity = selection_entity(vec![
            ("path", entity_ecf::text("/p/y")),
            // `paths[]` is in §5.4's own schema and we do not consume it;
            // `screen_id` is a plausible future addition. Neither is legacy.
            (
                "paths",
                entity_ecf::Value::Array(vec![entity_ecf::text("/p/y"), entity_ecf::text("/p/z")]),
            ),
            ("screen_id", entity_ecf::text("main")),
        ]);
        let (decoded, violations) = Selection::decode(&entity);
        assert_eq!(decoded.path, "/p/y");
        assert!(
            violations.is_empty(),
            "an unknown field is not a violation — reporting it would make the \
             WARN worthless as a signal: {violations:?}"
        );
    }

    /// §5.4 rule 3: the three retired spellings are **reported**, not tolerated
    /// silently. Asserts the report, not only that the field failed to land —
    /// doing nothing at all also satisfies the second.
    #[test]
    fn from_entity_reports_every_retired_field_rather_than_tolerating_it() {
        for field in ["source_window", "source_panel", "content_type"] {
            let entity = selection_entity(vec![
                ("path", entity_ecf::text("/p/y")),
                (field, entity_ecf::text("whatever")),
            ]);
            let (decoded, violations) = Selection::decode(&entity);
            assert_eq!(
                violations.len(),
                1,
                "{field} is retired by §5.4 and silent tolerance is NON-CONFORMANT"
            );
            assert_eq!(violations[0].name, field);
            assert!(violations[0].rule.contains("rule 3"));
            assert_eq!(decoded.path, "/p/y", "the rest of the record still decodes");
        }
    }

    /// §5.4 rule 1 closes the whole `source_*` prefix, so a spelling outside
    /// rule 3's fixed list is still a violation we can see from the read side.
    /// This is the case a three-name allowlist cannot catch.
    #[test]
    fn an_unlisted_source_field_is_still_reported_under_rule_one() {
        let entity = selection_entity(vec![("source_panel_id", entity_ecf::text("7"))]);
        let (_, violations) = Selection::decode(&entity);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].rule.contains("rule 1"));
    }

    /// **The trap §4.2a names by hand.** `content_type` is retired on
    /// `app/state/selection` and *required* on `app/state/window-index`. The
    /// gate is scoped to the type, so our own ruled index schema does not trip
    /// it — if this ever fails, the gate was written on the field name.
    #[test]
    fn the_legacy_gate_is_scoped_to_the_selection_type_not_to_the_field_name() {
        let index_row = typed_entity(
            crate::window_index::INDEX_TYPE,
            vec![("content_type", entity_ecf::text("Shell"))],
        );
        let (_, violations) = Selection::decode(&index_row);
        assert!(
            violations.is_empty(),
            "`content_type` is required on the window index; only the selection \
             slot retired it: {violations:?}"
        );

        // And the round trip our own encoder produces is clean, which is the
        // emit-side MUST (rule 1/2) restated as a test.
        let (_, own) = Selection::decode(&Selection::entity("/p/x", "peer").to_entity());
        assert!(own.is_empty(), "we emit a conformant payload: {own:?}");
    }

    /// §5.4 rule 4 — a reader that loads a legacy entity and re-publishes MUST
    /// NOT re-emit the legacy field. Holds by construction here (the field never
    /// reaches `Selection`), and this pins that it stays that way.
    #[test]
    fn a_legacy_field_is_not_re_emitted_on_the_round_trip() {
        let entity = selection_entity(vec![
            ("path", entity_ecf::text("/p/y")),
            ("source_window", entity_ecf::Value::Integer(7.into())),
        ]);
        let re_emitted = Selection::from_entity(&entity).to_entity();
        let (_, violations) = Selection::decode(&re_emitted);
        assert!(
            violations.is_empty(),
            "we re-emitted a field §5.4 retired: {violations:?}"
        );
    }
}
