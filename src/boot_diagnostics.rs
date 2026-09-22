//! **What this profile believes about routing, written where a dead app can still be read.**
//!
//! The `ecdeos.org` re-key stranded every returning visitor, and the reason it
//! went undiagnosed for days is not that the failure was subtle — it is that
//! nothing anywhere could *state* it. From the outside the profile looked
//! perfect: `boot_load: complete`, frame loop armed, watchdog installed, local
//! content rendering. It had simply stopped being able to see anything new,
//! because every remote path under a retired publisher 404s while the tree keeps
//! serving what it already had.
//!
//! Diagnosing that needs exactly one comparison: **who this profile is pointed
//! at, beside who the domain says publishes it.** The second half is a plain
//! `GET /entity-deployment.json`, which the L1 System Recovery console (the
//! "BIOS" in `index.html`) already does. The first half lives in a CBOR-encoded
//! entity inside the durable tree — which the BIOS deliberately cannot decode,
//! because it must keep working when the peer does not boot at all.
//!
//! So the value is mirrored across the tier boundary, in localStorage, on every
//! boot. This is not a new idea here: [`crate::boot_fast_paint::write_enabled_mirror`]
//! does the same thing for the same reason ("the durable tree config isn't
//! readable before the peer exists"), and self-heals on each boot the same way.
//!
//! ## Why this is a mirror and not a second source of truth (AP17, AP30)
//!
//! A mirror that outlives its source becomes a lie, and a durable record with no
//! way to re-derive it is AP30. Both are answered by the same property: **it is
//! rewritten from the tree on every boot that gets far enough to have an
//! answer.** The re-ask path is "boot again". Nothing reads it to make a
//! decision — no code branches on it — it exists solely to be *reported*, at a
//! tier that cannot reach the original.
//!
//! ## Staleness is the feature, which is why the timestamp is not optional
//!
//! When the app cannot boot, this is the last thing it believed while it could,
//! and that is precisely the interesting fact. It must therefore never be
//! rendered as "what this profile believes" — only as "what it believed at
//! `written_at`, running `build`". A reader who cannot tell those apart will
//! draw a confident wrong conclusion from an old value, which is the failure
//! this module exists to prevent, one level up.

use std::collections::BTreeMap;

/// localStorage key. Read by the recovery console in `index.html`; grep both
/// places together if you rename it.
pub const ROUTING_MIRROR_KEY: &str = "entity_routing_mirror";

/// Serialize the routing facts as the small JSON object the BIOS parses.
///
/// Hand-rolled rather than via `serde_json` so this stays a pure function over
/// strings and can be unit-tested without a browser or a peer. The fields are
/// all peer ids, site ids and a timestamp — public routing information, nothing
/// secret, which is a deliberate constraint on what may ever be added here: the
/// console renders this verbatim and performs no redaction.
pub fn routing_mirror_json(
    home_peer: &str,
    home_site: &str,
    supersessions: &BTreeMap<String, String>,
    build: &str,
    written_at_ms: f64,
) -> String {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let retired: Vec<String> = supersessions
        .iter()
        .map(|(r, n)| format!(r#"{{"retired":"{}","replacement":"{}"}}"#, esc(r), esc(n)))
        .collect();
    format!(
        r#"{{"home_peer":"{}","home_site":"{}","build":"{}","written_at":{},"supersessions":[{}]}}"#,
        esc(home_peer),
        esc(home_site),
        esc(build),
        written_at_ms as u64,
        retired.join(",")
    )
}

/// Write the mirror. Best-effort and silent on failure — a diagnostic that can
/// break a boot is worse than no diagnostic, and this runs on every boot.
#[cfg(target_arch = "wasm32")]
pub fn write_routing_mirror(
    home_peer: &str,
    home_site: &str,
    supersessions: &BTreeMap<String, String>,
) {
    let build = crate::build_id::current().describe();
    let now = web_sys::window()
        .and_then(|w| w.performance().map(|p| p.time_origin() + p.now()))
        .unwrap_or(0.0);
    let json = routing_mirror_json(home_peer, home_site, supersessions, &build, now);
    if let Some(Ok(Some(storage))) = web_sys::window().map(|w| w.local_storage()) {
        let _ = storage.set_item(ROUTING_MIRROR_KEY, &json);
    }
    tracing::info!(
        home_peer = %home_peer,
        retired = supersessions.len(),
        "boot diagnostics: routing mirror written for System Recovery"
    );
}

#[cfg(not(target_arch = "wasm32"))]
pub fn write_routing_mirror(
    _home_peer: &str,
    _home_site: &str,
    _supersessions: &BTreeMap<String, String>,
) {
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    /// The shape the recovery console parses. Asserted as a whole string
    /// because the console's `JSON.parse` is on the other side of a boundary no
    /// compiler checks — this test and the BIOS reader are the contract.
    #[test]
    fn the_mirror_carries_the_routing_facts_and_when_they_were_true() {
        let json = routing_mirror_json(
            "2KNew",
            "demo",
            &map_of(&[("2KOld", "2KNew")]),
            "083443c",
            1_700_000_000_123.0,
        );
        assert_eq!(
            json,
            r#"{"home_peer":"2KNew","home_site":"demo","build":"083443c","written_at":1700000000123,"supersessions":[{"retired":"2KOld","replacement":"2KNew"}]}"#
        );
    }

    /// A profile with nothing retired still writes a well-formed document — the
    /// console must not have to special-case the healthy majority.
    #[test]
    fn an_untouched_profile_still_produces_a_parseable_mirror() {
        let json = routing_mirror_json("2KA", "home", &BTreeMap::new(), "dev", 0.0);
        assert!(json.contains(r#""supersessions":[]"#), "got {json}");
        assert!(json.contains(r#""home_peer":"2KA""#), "got {json}");
    }

    /// Peer ids are base58 and site ids are author-supplied, so the escape is
    /// not decoration: an unescaped quote would produce a document the console
    /// silently fails to parse, and a diagnostic that silently renders nothing
    /// is the exact failure mode this module exists to remove.
    #[test]
    fn a_hostile_site_id_cannot_break_the_document() {
        let json = routing_mirror_json("2KA", "he\"re\\", &BTreeMap::new(), "dev", 0.0);
        assert!(json.contains(r#""home_site":"he\"re\\""#), "got {json}");
    }
}
