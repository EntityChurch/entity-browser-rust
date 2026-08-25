//! Render-ready output for the System Overview window. Pure data — the DOM
//! renderer (`crate::dom::system_overview`) consumes this without touching the
//! model or `Peers`.

/// Backend identity + lifecycle for the status panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendStatusView {
    /// Full Base58 peer id.
    pub peer_id: String,
    /// First 12 chars, for a compact display.
    pub short_id: String,
    /// Lifecycle string from the backend (`running` / `stopped` / …).
    pub status: String,
    /// WS listen / connect address, when running.
    pub ws_addr: Option<String>,
    /// Whether this backend is serving `system/signaling` right now — i.e.
    /// whether browsers on this LAN can rendezvous through it.
    ///
    /// From the IPC poll, which reports the **running** peer rather than the
    /// persisted setting: the handler mounts at build time, so a peer started
    /// before the toggle was flipped is configured to serve and is not serving.
    pub signaling_node: bool,
    /// The internet-reachable address a router is forwarding to this backend.
    /// Only ever an address that works right now — it is withdrawn the moment a
    /// renewal fails, so a surface may render it without qualification.
    pub external_addr: Option<String>,
    /// Whether we are asking a router at all. `true` with `external_addr: None`
    /// is the ordinary case on most routers, not an error.
    pub port_mapping: bool,
    /// Why there is no address, when there is a reason worth showing.
    pub port_mapping_note: Option<String>,
}

/// Everything the System Backend window renders.
#[derive(Clone, Debug)]
pub struct SystemOverviewOutput {
    /// The `system-backend` peer, once polled; `None` if not fetched yet or
    /// (unexpectedly) not provisioned.
    pub backend: Option<BackendStatusView>,
    /// Whether the backend list has been polled at least once — lets the view
    /// distinguish "loading…" from "not provisioned".
    pub fetched: bool,
    /// Whether S is connected to B (from the connections registry).
    pub connected: bool,
    /// Whether the S→B auto-connect is armed and actively dialing (transport not
    /// up yet). When `true` and `!connected`, the link chip reads "Connecting…"
    /// rather than "Offline" — the boot provision→dial→handshake window reads as
    /// progress, not breakage. `false` once connected or after the dial burst is
    /// exhausted (genuine Offline).
    pub dialing: bool,
    /// Tree prefix B shares its files at.
    pub share_prefix: String,
    /// The backend's shared-files directory on disk (behind `share_prefix`);
    /// `None` until fetched. Answers "where do shared files actually live".
    pub share_path: Option<String>,
    /// Current backend log level (`off`/…/`trace`) — the level control's
    /// selected value. Empty until fetched.
    pub log_level: String,
    /// Tailed lines of B's native `tracing` output, oldest first.
    pub log_lines: Vec<String>,
    /// Whether we're in the desktop app (backend visibility requires Tauri).
    /// `false` in a plain browser → the view shows a desktop-only note.
    pub tauri: bool,
    /// Inbound devices connected to B and their authorization status — the
    /// authority surface (moved here from Peer Connections, Direction A). `None`
    /// until a backend is known.
    pub authorizations: Option<AuthorizationsView>,
}

/// The canonical backend's inbound-device authorization surface — a render-ready
/// projection of a `backend_auth::BackendAuthObservation`. Managing *who can do
/// what on the backend* lives here (the backend owns the grant), not in the
/// file-transfer app.
#[derive(Clone, Debug)]
pub struct AuthorizationsView {
    /// The backend these devices are connected to (dispatch target for
    /// refresh/authorize).
    pub backend_pid: String,
    /// The manager peer that dispatches the read/grant (the system peer, which
    /// holds B's manager capability).
    pub manager_pid: String,
    /// Whether a mirror observation exists yet (a check/refresh has run). `false`
    /// renders a "not checked" prompt rather than an empty list.
    pub checked: bool,
    /// A read failure, cleaned for display (`§5` — shown, never swallowed).
    pub error: Option<String>,
    /// Connected devices awaiting authorization — each an actionable row.
    pub pending: Vec<AuthRow>,
    /// Devices already authorized on the backend.
    pub authorized: Vec<AuthRow>,
}

/// One inbound device in an [`AuthorizationsView`].
#[derive(Clone, Debug, Default)]
pub struct AuthRow {
    /// Device id as the backend reports it (identity-hash hex) — the authorize
    /// target key.
    pub peer_id: String,
    /// Short display form (truncated id).
    pub display: String,
    /// The granted profile token (`file-transfer` / `file-transfer-rw` /
    /// `trusted`) for an **authorized** row, read back from the local `authz`
    /// mirror so the operator can see *what* a device was granted — not just
    /// that it was. `None` for a pending row (nothing granted yet) or when the
    /// mirror has no record.
    pub profile: Option<String>,
}

/// Which address a pairing line carries — the label the row renders, and the
/// only thing that distinguishes two otherwise identical commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairScope {
    /// The listener's own address. Works for another machine on this network.
    Lan,
    /// The address a router is forwarding right now. Works from anywhere —
    /// including, on most but not all routers, from this network.
    Internet,
}

/// The pairing lines this backend can offer: the literal command to run on the
/// **other** machine, per address that works from where that machine is.
///
/// Pure, and separated from the renderer deliberately. What can be wrong here is
/// the *composition* — the verb, the argument order, which address goes with
/// which label, and whether a row appears at all — and none of that is
/// observable from a native test while it lives inside `create_element` calls.
/// The DOM half is then a loop over this.
///
/// Three rules, each of which is a way to publish an instruction that fails:
///
/// - **No rendezvous, no rows.** A pairing command for a node that is off tells
///   someone to join something that will refuse them.
/// - **No address, no rows.** A stopped listener has nothing to dial.
/// - **The LAN line comes first, and the internet line only when a door is
///   actually open.** `external_addr` is `Some` for exactly *a mapping is live*
///   — never for "asked" and never for "refused" — so this cannot advertise a
///   door that is not there. When the two addresses are identical there is one
///   row, because two rows carrying the same command is not a choice.
pub fn pairing_commands(b: &BackendStatusView) -> Vec<(PairScope, String)> {
    if !b.signaling_node {
        return Vec::new();
    }
    let Some(lan) = b.ws_addr.as_deref().filter(|a| !a.is_empty()) else {
        return Vec::new();
    };
    // i18n-ignore — shell syntax, not prose: this is the literal line the
    // `connector add` verb parses, and translating it produces a command that
    // does not run.
    let cmd = |addr: &str| format!("connector add {} {}", b.peer_id, addr); // i18n-ignore
    let mut out = vec![(PairScope::Lan, cmd(lan))];
    if let Some(ext) = b.external_addr.as_deref().filter(|a| !a.is_empty() && *a != lan) {
        out.push((PairScope::Internet, cmd(ext)));
    }
    out
}

#[cfg(test)]
mod pairing_tests {
    use super::*;

    fn backend() -> BackendStatusView {
        BackendStatusView {
            peer_id: "2KNodeSevenFullBase58Id".into(),
            short_id: "2KNodeSevenF".into(),
            status: "running".into(),
            ws_addr: Some("ws://192.168.1.10:4041".into()),
            signaling_node: true,
            external_addr: None,
            port_mapping: false,
            port_mapping_note: None,
        }
    }

    /// The line must be exactly what the verb parses. Asserted as a whole
    /// string, not by `contains`: the argument ORDER is the part a reader cannot
    /// check and the part that silently produces `no connector with peer-id
    /// ws://…` on the other machine.
    #[test]
    fn the_lan_line_is_the_command_the_connector_verb_accepts() {
        let rows = pairing_commands(&backend());
        assert_eq!(rows.len(), 1, "no door is open, so there is one line");
        assert_eq!(rows[0].0, PairScope::Lan);
        assert_eq!(
            rows[0].1,
            "connector add 2KNodeSevenFullBase58Id ws://192.168.1.10:4041"
        );
    }

    /// A forwarded address is an ADDITIONAL line, and the LAN one stays — a
    /// mapped address is not guaranteed to hairpin back onto its own network,
    /// so replacing the LAN line would strand the machine in the next room.
    #[test]
    fn an_open_door_adds_a_second_line_and_does_not_replace_the_first() {
        let b = BackendStatusView {
            external_addr: Some("ws://203.0.113.7:4041".into()),
            ..backend()
        };
        let rows = pairing_commands(&b);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, PairScope::Lan);
        assert_eq!(rows[1].0, PairScope::Internet);
        assert!(rows[1].1.ends_with("ws://203.0.113.7:4041"), "{}", rows[1].1);

        // Same address twice is one line: two rows carrying an identical
        // command is not a choice, it is a rendering bug wearing one.
        let same = BackendStatusView {
            external_addr: Some("ws://192.168.1.10:4041".into()),
            ..backend()
        };
        assert_eq!(pairing_commands(&same).len(), 1);
    }

    /// Nothing to join, nothing to print. Both halves, because they fail
    /// differently: a stopped node refuses the caller, a stopped listener has no
    /// address to give — and either way an instruction that cannot work is worse
    /// than an absent row.
    #[test]
    fn a_node_that_is_off_or_not_listening_offers_no_pairing_line() {
        let off = BackendStatusView { signaling_node: false, ..backend() };
        assert!(pairing_commands(&off).is_empty(), "the rendezvous is off");

        let silent = BackendStatusView { ws_addr: None, ..backend() };
        assert!(pairing_commands(&silent).is_empty(), "nothing is listening");

        let blank = BackendStatusView { ws_addr: Some(String::new()), ..backend() };
        assert!(pairing_commands(&blank).is_empty(), "an empty address is not an address");
    }
}
