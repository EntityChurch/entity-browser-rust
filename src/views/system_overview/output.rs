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

/// The *Problems* section — Entity Doctor's checks, rendered inside System
/// Overview rather than in a window of their own (`doctor.rs`, "Where this
/// appears").
///
/// `ran` is not decoration and is not the same as `findings.is_empty()`: an
/// empty list before the first run and an empty list after a clean run are the
/// same value and opposite facts. The renderer shows *"Checking…"* for one and
/// *"No problems found"* for the other, and it can only tell them apart from
/// this flag.
#[derive(Debug, Clone, Default)]
pub struct HealthView {
    /// Whether the checks have completed at least once this window-open.
    pub ran: bool,
    /// Whether a run is in flight **right now**. Orthogonal to `ran`: on a
    /// re-check both are true, and the renderer keeps showing the last answer
    /// while saying it is looking again.
    pub checking: bool,
    /// Every finding, including the clear ones — the renderer decides what to
    /// show. Handing it only the problems would make "quiet when healthy" a
    /// property of the model, where nothing can see it change.
    pub findings: Vec<crate::doctor::Finding>,
    /// What the last applied remedy reported, if one was applied this session.
    pub remedy_message: Option<String>,
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
    /// Whether another device on this network can load the app from this
    /// desktop, and whether it arrives already knowing the rendezvous.
    pub app_server: AppServerView,
}

/// The "serve the app from here" row.
///
/// Three states, deliberately not two. Collapsing `Serving` and
/// `ServingUnprovisioned` into one "on" would promise *type a URL and you are
/// done* while delivering *type a URL, then add a connector by hand* — which is
/// the same shape of half-truth as a rendezvous row that reports the persisted
/// setting instead of what is mounted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AppServerView {
    /// Not serving. Another device cannot load the app from here at all.
    #[default]
    Off,
    /// Serving, and a visitor arrives provisioned with this desktop's
    /// rendezvous — the whole flow is "type this URL".
    Serving { url: String, node_peer_id: String },
    /// Serving, but this desktop is not a rendezvous, so a visitor gets the app
    /// and still has to be told how to reach anybody. The remedy is the
    /// Rendezvous row directly above, which is why they sit together.
    ServingUnprovisioned { url: String },
}

impl AppServerView {
    /// Grade the IPC report into the three states.
    ///
    /// A **pure function over plain arguments, not over `AppServerInfo`** — the
    /// IPC type is `wasm32`-only, and taking it here would put this classifier
    /// out of reach of `make test`, which is where the "serving but
    /// unprovisioned" distinction is actually checked. Same native-shadow split
    /// as `WebRtcProvisioning` against the worker wire types.
    pub fn grade(serving: bool, url: Option<&str>, node_peer_id: Option<&str>) -> Self {
        match (serving, url, node_peer_id) {
            (true, Some(url), Some(node)) if !url.is_empty() && !node.is_empty() => {
                Self::Serving { url: url.to_string(), node_peer_id: node.to_string() }
            }
            (true, Some(url), _) if !url.is_empty() => {
                Self::ServingUnprovisioned { url: url.to_string() }
            }
            // Serving with no URL is not a state the backend can produce — the
            // address is read back from the bound socket. If it ever were, the
            // honest answer is Off rather than a row with nothing to type.
            _ => Self::Off,
        }
    }

    pub fn is_serving(&self) -> bool {
        !matches!(self, Self::Off)
    }

    /// The URL to hand a person, when there is one.
    pub fn url(&self) -> Option<&str> {
        match self {
            Self::Off => None,
            Self::Serving { url, .. } | Self::ServingUnprovisioned { url } => Some(url),
        }
    }
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

/// One step in *"how do I get another device connected to this one"*, in the
/// order to try them.
///
/// **The two are not the same tier, and rendering them as a flat pile of
/// strings is what made this card unreadable.** Opening the served URL is the
/// WHOLE flow — the browser arrives provisioned, nothing typed, no reload. The
/// pairing command is the FALLBACK, for a device that did not come from this
/// desktop's URL: it loaded the app from somewhere else, or it is already
/// running. Presenting them as two equivalent options asks the operator to
/// work out which one they are in, which is the question this card exists to
/// answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectStep {
    /// Open this URL on the other device.
    ///
    /// `provisioned` is false when the app server is up and this desktop is
    /// **not** a rendezvous — the URL still hands over a working app, and its
    /// visitor still cannot reach anybody. A surface must not present that as
    /// the finished flow; it is the same distinction [`AppServerView`]'s three
    /// states exist for, carried one layer out.
    OpenUrl { url: String, provisioned: bool },
    /// Paste this into the other machine's Shell, then reload that page.
    PasteCommand { scope: PairScope, command: String },
}

/// What to carry to the other device, best first.
///
/// Pure and separated from the renderer for [`pairing_commands`]' reason: what
/// can be wrong here is the **order** and whether a step appears at all, and
/// neither is observable from a native test while it lives inside
/// `create_element` calls.
///
/// Empty means there is nothing to carry — nothing is served and no rendezvous
/// is running — and the renderer omits the card rather than showing an empty
/// one. The remedy is the switches directly above it, which are already on
/// screen.
pub fn connect_steps(b: &BackendStatusView, s: &AppServerView) -> Vec<ConnectStep> {
    let mut out = Vec::new();
    // The URL first, always: it is the only step that needs nothing typed on
    // the far end, so an operator who reads no further has read the best one.
    if let Some(url) = s.url() {
        out.push(ConnectStep::OpenUrl {
            url: url.to_string(),
            provisioned: matches!(s, AppServerView::Serving { .. }),
        });
    }
    out.extend(
        pairing_commands(b)
            .into_iter()
            .map(|(scope, command)| ConnectStep::PasteCommand { scope, command }),
    );
    out
}

#[cfg(test)]
mod connect_tests {
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

    fn serving() -> AppServerView {
        AppServerView::Serving {
            url: "http://192.168.1.10:8081".into(),
            node_peer_id: "2KNodeSevenFullBase58Id".into(),
        }
    }

    /// **The order is the decision.** The URL needs nothing typed on the far
    /// end; the command needs a paste and a reload. An operator who reads only
    /// the first row must have read the better one, so this asserts position
    /// and not merely membership.
    #[test]
    fn the_url_comes_before_the_command_it_replaces() {
        let steps = connect_steps(&backend(), &serving());
        assert_eq!(steps.len(), 2);
        assert_eq!(
            steps[0],
            ConnectStep::OpenUrl {
                url: "http://192.168.1.10:8081".into(),
                provisioned: true,
            },
        );
        assert!(matches!(
            steps[1],
            ConnectStep::PasteCommand { scope: PairScope::Lan, .. }
        ));
    }

    /// Serving without a rendezvous still yields a URL — and one that must not
    /// claim to be the finished flow. The visitor gets a working app and no way
    /// to reach anybody, which is exactly what `provisioned: false` is for.
    ///
    /// There is no pairing line in this state either, and that is not an
    /// oversight: `pairing_commands` refuses to print a command for a node that
    /// is off, because it would tell someone to join something that will refuse
    /// them.
    #[test]
    fn serving_without_a_rendezvous_offers_the_url_unprovisioned_and_no_command() {
        let b = BackendStatusView { signaling_node: false, ..backend() };
        let s = AppServerView::ServingUnprovisioned { url: "http://192.168.1.10:8081".into() };
        let steps = connect_steps(&b, &s);
        assert_eq!(
            steps,
            vec![ConnectStep::OpenUrl {
                url: "http://192.168.1.10:8081".into(),
                provisioned: false,
            }],
        );
    }

    /// A rendezvous with no app server is the *other* half — the manual path,
    /// which is what you want against a desktop that is not handing out the app
    /// (or a device that is already running one).
    #[test]
    fn a_rendezvous_alone_offers_the_manual_path() {
        let steps = connect_steps(&backend(), &AppServerView::Off);
        assert_eq!(steps.len(), 1);
        assert!(matches!(
            steps[0],
            ConnectStep::PasteCommand { scope: PairScope::Lan, .. }
        ));
    }

    /// Both open doors produce both lines, still behind the URL. The internet
    /// line only ever appears when a mapping is live (`pairing_commands`), so
    /// this cannot advertise a door that is not there.
    #[test]
    fn an_open_router_adds_a_third_step_and_the_url_still_leads() {
        let b = BackendStatusView {
            external_addr: Some("ws://203.0.113.7:4041".into()),
            ..backend()
        };
        let steps = connect_steps(&b, &serving());
        assert_eq!(steps.len(), 3);
        assert!(matches!(steps[0], ConnectStep::OpenUrl { .. }));
        assert!(matches!(
            steps[1],
            ConnectStep::PasteCommand { scope: PairScope::Lan, .. }
        ));
        assert!(matches!(
            steps[2],
            ConnectStep::PasteCommand { scope: PairScope::Internet, .. }
        ));
    }

    /// Nothing served and nothing to join: no steps, so the renderer omits the
    /// card entirely. An empty "Connect another device" card is a question with
    /// no answer, and the remedy — the switches — is already on screen above it.
    #[test]
    fn nothing_running_yields_no_steps_at_all() {
        let off = BackendStatusView { signaling_node: false, ..backend() };
        assert!(connect_steps(&off, &AppServerView::Off).is_empty());
    }
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

    /// The distinction the whole three-state enum exists for: serving *with* a
    /// rendezvous is "type this URL and you are done"; serving *without* one
    /// hands over a working app that still cannot reach anybody. A two-state
    /// "on/off" would promise the first while delivering the second.
    #[test]
    fn serving_without_a_rendezvous_is_its_own_state_not_just_on() {
        assert_eq!(
            AppServerView::grade(true, Some("http://192.168.1.9:8081"), Some("2KaNODE")),
            AppServerView::Serving {
                url: "http://192.168.1.9:8081".into(),
                node_peer_id: "2KaNODE".into(),
            },
        );
        assert_eq!(
            AppServerView::grade(true, Some("http://192.168.1.9:8081"), None),
            AppServerView::ServingUnprovisioned { url: "http://192.168.1.9:8081".into() },
            "a visitor gets the app but must still add a connector by hand",
        );
        // An empty node-id is the same absence as a missing one — it arrives
        // over IPC, where "" and null are not reliably distinct.
        assert_eq!(
            AppServerView::grade(true, Some("http://x:8081"), Some("")),
            AppServerView::ServingUnprovisioned { url: "http://x:8081".into() },
        );
    }

    /// Serving with nothing to type is not a state the backend can produce (the
    /// URL is read back from the bound socket). If it ever were, the row must
    /// say Off rather than claim a service with no address — the same rule as
    /// the pairing row, which renders nothing when nothing is listening.
    #[test]
    fn serving_with_no_url_is_reported_as_off() {
        assert_eq!(AppServerView::grade(true, None, Some("2KaNODE")), AppServerView::Off);
        assert_eq!(AppServerView::grade(true, Some(""), Some("2KaNODE")), AppServerView::Off);
        assert_eq!(AppServerView::grade(false, Some("http://x:8081"), Some("n")), AppServerView::Off);
        assert!(!AppServerView::Off.is_serving());
        assert!(AppServerView::grade(true, Some("http://x:8081"), None).is_serving());
    }
}
