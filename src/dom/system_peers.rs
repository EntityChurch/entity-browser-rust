//! System-peers section renderer — the posture line + the system peer identity
//! cards (in-app + native). Consumed by the merged System Overview window
//! (`dom::system_overview::render`), which supplies the window header and, below
//! these cards, the native peer's live detail (status, device auth, logs,
//! share). There is no drill-in: after the merge (S2 — one window, one job) the
//! detail lives in the same window, right underneath.

use web_sys::Element;

use crate::dom::components;
use crate::dom::util;
use crate::views::system_peers::output::{SystemPeersOutput, SystemPeerCard};

/// Append the posture line + system-peer identity cards into `parent`. No header
/// of its own — the window titles itself "System Overview" once. The native
/// peer's identity card is deliberately terse (id + role·runtime·storage); its
/// live status/listen/share is the Status section that follows, so we don't
/// repeat it here.
pub fn render_system_peers(parent: &Element, output: &SystemPeersOutput) {
    // Posture line — the deployment's cold-boot facts in one glanceable row.
    let posture = util::create_element("p");
    posture
        .set_attribute("style", "color:var(--text-dim, #888);font-size:13px;margin:0 0 12px")
        .ok();
    let peers_txt = if output.user_peers == 0 {
        format!("{} peer(s)", output.total_peers)
    } else {
        format!("{} peer(s), {} yours", output.total_peers, output.user_peers)
    };
    let create_txt = if output.peer_creation { "on" } else { "off" };
    util::set_text(
        &posture,
        &format!(
            "Startup: {}  ·  Peer creation: {}  ·  {}",
            output.startup, create_txt, peers_txt
        ),
    );
    util::append(parent, &posture);

    // The in-app system peer — always present.
    let in_app = components::card(&crate::i18n::t("syspeers.system_peer", &[]));
    append_peer_facts(&in_app, &output.in_app);
    util::append(parent, &in_app);

    // The System backend — desktop only. Identity only; the detail is below.
    if let Some(card) = &output.native {
        let native = components::card(&crate::i18n::t("syspeers.system_backend", &[]));
        append_peer_facts(&native, card);
        util::append(parent, &native);
    }
}

/// Render a peer's identity + role·runtime·storage facts into a card, reusing
/// the Peers-table badge/chip vocabulary so the facts read identically here.
fn append_peer_facts(card: &Element, peer: &SystemPeerCard) {
    let row = util::create_element_with_class("div", "sysov-peer-line");

    let id = util::create_element_with_class("span", "sysov-peer-id");
    util::set_text(&id, &format!("{} {}", peer.descriptor.glyph(), peer.short_id));
    id.set_attribute("title", &peer.full_id).ok();
    util::append(&row, &id);

    // System/User badge + runtime chip + storage chip (same classes as Peers).
    let badge = util::create_element_with_class("span", "peer-badge system");
    util::set_text(&badge, "System");
    util::append(&row, &badge);

    let runtime = util::create_element_with_class("span", "peer-chip");
    util::set_text(&runtime, peer.descriptor.runtime.label());
    util::append(&row, &runtime);

    let storage = util::create_element_with_class("span", "peer-chip");
    util::set_text(&storage, peer.descriptor.storage.label());
    util::append(&row, &storage);

    util::append(card, &row);
}
