//! **Preflight** — can this machine reach another machine's peer right now, and
//! if not, *which half is missing*?
//!
//! # Why this exists
//!
//! Every connectivity gate in this repo runs podman containers on one Linux box
//! (`docs/plans/PUNCHLIST-RELEASE.md` §1). The first time the product is run as
//! pitched — two computers, one of them hosting the rendezvous — the person at
//! the second machine has no test harness, no `podman ps`, and no stderr. What
//! they have is a browser tab that either connects or doesn't, and today a
//! *silent* fallback behind every one of the ways it can fail to:
//!
//! - the SPA served from `http://box.lan:8081` is **not a secure context**, so
//!   the browser refuses `RTCPeerConnection` outright and the Worker bootstrap
//!   fails into Direct mode (`main.rs`) — the feature reads as inert;
//! - a connector added but never reloaded leaves the session with **no
//!   establisher** while the registry looks correct [AP22];
//! - a `meet` from a **non-primary** peer succeeds completely and hands the
//!   counterpart an id that can never reach back (establisher is primary-only);
//! - zero reflectors is a legal LAN deployment and *silently* not a WAN one.
//!
//! Each of those is already known here, each is documented, and none of them
//! says anything **on the machine where it is happening**. That is the gap this
//! module closes: one report, listing what was measured and — for anything that
//! is not `Ok` — the next action. A diagnosis that names a fault without naming
//! the remedy is the one-sentence failure this repo keeps re-meeting
//! (`reachability.rs`' whole reason for existing, one layer earlier).
//!
//! # Shape: facts are collected, verdicts are pure
//!
//! [`Facts`] is a plain struct with no browser types in it, and [`assess`] is a
//! pure function over it — so every verdict, every remedy string and the
//! ordering are covered by `make test` on native, where no `RTCPeerConnection`
//! exists. [`collect`] is the thin wasm-only mapping that fills a `Facts` in
//! from the live session. Same native-shadow split as
//! `session_config::WebRtcProvisioning` vs the worker wire types, and for the
//! same reason.
//!
//! # What it deliberately is not
//!
//! **Not a liveness store.** The one connection fact it reports — the node's
//! status — is read from the kernel read-model (`peer_liveness`) at collection
//! time and never cached here. Nothing in this module is written to the tree.
//!
//! **Not a substitute for `reachability`.** That module classifies a
//! negotiation that already ran, from the ICE candidate types it produced. This
//! one runs *before* anything is attempted and can only see configuration and
//! platform. They answer different questions and neither subsumes the other.

/// How much a single check should worry the reader.
///
/// Four levels rather than a boolean because "not configured" and "configured
/// wrong" are different conversations, and because a legal-but-narrow posture
/// (host candidates only) must be sayable without being an error — a gate that
/// cries wolf gets routed around.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Measured, and it is what we want.
    Ok,
    /// Measured, works, and narrows what will work. Never a fault.
    Warn,
    /// Measured, and something the person came here to do cannot happen.
    Fail,
    /// Not measurable from here. **Never** collapsed into `Ok` — an unasked
    /// question and an answered one arriving as the same value is this repo's
    /// most-repeated seam.
    Unknown,
}

impl Level {
    /// Single-column marker. ASCII-safe glyphs so a copy-paste out of a
    /// terminal, an IRC window or a bug report survives.
    pub fn glyph(self) -> &'static str {
        match self {
            Level::Ok => "OK  ",
            Level::Warn => "WARN",
            Level::Fail => "FAIL",
            Level::Unknown => "??  ",
        }
    }

    /// Severity order for [`Report::worst`]. `Unknown` sorts *below* `Fail`:
    /// not knowing is worse than fine and better than broken.
    fn severity(self) -> u8 {
        match self {
            Level::Ok => 0,
            Level::Warn => 1,
            Level::Unknown => 2,
            Level::Fail => 3,
        }
    }
}

/// One measured property, its verdict, and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// Stable token, kebab-case. Greppable in logs and assertable from a gate
    /// without matching on prose — the check text is allowed to be reworded,
    /// the id is not.
    pub id: &'static str,
    pub level: Level,
    /// What was measured, in the reader's terms.
    pub detail: String,
    /// The next action. `None` only when there is nothing to do — every
    /// non-`Ok` check owes one, which [`every_non_ok_check_carries_a_remedy`]
    /// enforces rather than leaving to review.
    pub remedy: Option<String>,
}

/// The node this session is provisioned to rendezvous through, as configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeFacts {
    pub peer_id: String,
    pub addr: String,
    pub label: String,
    /// Reflectors after [`crate::connectors::merge_reflectors`] — the user's
    /// typed ones **and** what the node advertised, which is what the ICE agent
    /// will actually be handed.
    pub reflectors: usize,
    /// A relay (TURN) with credentials, per `IceServer::is_relay`.
    pub relay: bool,
    /// The kernel's liveness word for this node peer, or `None` when nothing
    /// has dialled it this session (which is the normal cold state, not a
    /// fault).
    pub liveness: Option<&'static str>,
}

/// Everything [`assess`] is allowed to reason from. No browser types: the
/// collector converts, the model decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    /// `window.isSecureContext`. `None` off the browser.
    pub secure_context: Option<bool>,
    /// The page origin, verbatim — it is half the remedy ("you are on
    /// `http://box.lan:8081`; use localhost or TLS").
    pub origin: String,
    /// Is `RTCPeerConnection` constructible here. `None` off the browser.
    pub rtc_peer_connection: Option<bool>,
    /// `"direct"` or `"worker"` — the arm the *bound* peer runs on.
    pub arm: &'static str,
    /// The peer a `meet` from this surface would hand out.
    pub local_peer_id: String,
    /// Is that peer the primary. The establisher is primary-only, so this
    /// decides whether a missing establisher is a reload or a wrong window.
    pub local_is_primary: bool,
    /// Does the bound peer hold a §6.5 establisher (`Peers::peer_has_webrtc`).
    pub establisher_installed: bool,
    /// The connector this session **booted** with. `None` means no node was
    /// resolved at boot — which is not the same as "no connector is
    /// configured", and the pair below is what tells them apart.
    pub booted_node: Option<NodeFacts>,
    /// Would a reload resolve a *different* provisioning than the one running?
    /// True is the AP22 shape: registry correct, session unaware.
    pub provisioning_drifted: bool,
    /// How many connectors the durable registry holds, regardless of selection.
    pub connectors_configured: usize,
}

impl Facts {
    /// A `Facts` with nothing known — the native/test starting point. Every
    /// field is set explicitly by callers; this only exists so a test can state
    /// the two fields it is about instead of all twelve.
    #[allow(dead_code)]
    pub fn blank() -> Self {
        Self {
            secure_context: None,
            origin: String::new(),
            rtc_peer_connection: None,
            arm: "direct",
            local_peer_id: String::new(),
            local_is_primary: true,
            establisher_installed: false,
            booted_node: None,
            provisioning_drifted: false,
            connectors_configured: 0,
        }
    }
}

/// The assessment: an ordered list of checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    /// The worst level present — the one-word answer.
    pub fn worst(&self) -> Level {
        self.checks
            .iter()
            .map(|c| c.level)
            .max_by_key(|l| l.severity())
            .unwrap_or(Level::Unknown)
    }

    /// One sentence for the top of the report.
    pub fn verdict(&self) -> &'static str {
        match self.worst() {
            // i18n-ignore — dev-facing diagnostic surface (see the module header)
            Level::Ok => "ready — this peer can be reached by another machine's browser",
            Level::Warn => "ready, with limits — read the WARN rows before blaming the network",
            Level::Unknown => "unknown — something below could not be measured from here",
            Level::Fail => "NOT ready — the FAIL rows below are why",
        }
    }

    /// The first thing to fix. Checks are emitted in causal order, so the first
    /// `Fail` is the root and everything after it may be a consequence — which
    /// is why this returns one line and not a list.
    pub fn headline(&self) -> Option<&Check> {
        self.checks.iter().find(|c| c.level == Level::Fail)
    }

    /// Render as fixed-width rows, each tagged with the level of the check it
    /// came from, remedies indented under their check.
    ///
    /// A remedy line carries its **check's** level rather than none, so a
    /// surface that styles by level cannot render a failure's remedy as though
    /// it were unrelated commentary — the two lines are one statement.
    pub fn rows(&self) -> Vec<(Level, String)> {
        let mut out = Vec::new();
        let width = self.checks.iter().map(|c| c.id.len()).max().unwrap_or(0);
        for c in &self.checks {
            out.push((
                c.level,
                format!("{} {:width$}  {}", c.level.glyph(), c.id, c.detail, width = width),
            ));
            if let Some(r) = &c.remedy {
                out.push((c.level, format!("     {:width$}  -> {}", "", r, width = width)));
            }
        }
        out
    }

    /// [`Self::rows`] without the levels — the copy-pasteable block.
    pub fn lines(&self) -> Vec<String> {
        self.rows().into_iter().map(|(_, s)| s).collect()
    }
}

/// Judge a set of facts. Pure, total, and the only place a verdict is decided.
///
/// **Order is causal, not alphabetical.** A page that is not a secure context
/// makes every WebRTC row below it fail for a reason that is not its own, so
/// the origin is first and the remedies downstream of it say "fix the row
/// above" rather than re-diagnosing. Same discipline as `reachability::classify`
/// checking the gathered-nothing case before the reflector arms.
pub fn assess(f: &Facts) -> Report {
    let mut checks = Vec::new();

    // --- 1. The origin, because everything else is downstream of it ---------
    //
    // `localhost` and `https` are secure contexts; a plain-http LAN or WAN
    // hostname is NOT — the exact shape of "I served the SPA from my desktop
    // and opened it on the laptop, or on my phone".
    //
    // **What that costs is measured, not assumed, and it is less than this row
    // used to claim.** The row asserted that an insecure origin disables
    // WebRTC; driving Firefox 149 at an http LAN origin refutes it —
    // `RTCPeerConnection` constructs and gathers host candidates normally. The
    // engines genuinely differ here, so the honest move is to *grade the origin
    // by the consequence the browser actually exhibits* rather than to predict
    // one. It also removes a self-contradiction: this row used to print
    // "browsers disable WebRTC off a secure origin" directly above a measured
    // `OK webrtc-api`, which is the cry-wolf shape that gets a preflight
    // ignored.
    //
    // What IS uniformly gated, in every engine, and so may be stated from
    // `secure_context` alone: OPFS (`navigator.storage`, hence Worker mode) and
    // `getUserMedia` (`navigator.mediaDevices`, hence the QR scanner). Note the
    // second one is the ironic casualty — the affordance built so nobody has to
    // retype a Base58 peer id is dead on exactly the origin you would want it
    // on. Neither is fatal: Worker mode is opt-in and off by default, and the
    // pairing line can be pasted.
    let insecure = f.secure_context == Some(false);
    checks.push(match (f.secure_context, f.rtc_peer_connection) {
        (Some(true), _) => Check {
            id: "origin",
            level: Level::Ok,
            detail: format!("secure context ({})", f.origin),
            remedy: None,
        },
        // Insecure, and this engine hands us WebRTC anyway. A real limitation,
        // not a blocker for the thing the reader came here to do.
        (Some(false), Some(true)) => Check {
            id: "origin",
            level: Level::Warn,
            detail: format!(
                "not a secure context ({}) — this engine allows WebRTC anyway",
                f.origin
            ),
            remedy: Some(
                "fine for a test on your own network. What IS unavailable here: \
                 Worker/OPFS storage (opt-in, off by default) and the camera, so \
                 QR pairing will not work — paste the pairing line instead. Serve \
                 over https:// in production."
                    .into(),
            ),
        },
        // Insecure, and no WebRTC. Now the origin is the cause and the row is
        // the headline.
        (Some(false), _) => Check {
            id: "origin",
            level: Level::Fail,
            detail: format!("NOT a secure context ({})", f.origin),
            remedy: Some(
                "this engine gates WebRTC on the origin, so browser-to-browser \
                 cannot work here. Open it from http://localhost on this machine, \
                 serve it over https://, or mark this origin trustworthy in the \
                 browser. (A ws:// connection out to another machine is fine from \
                 an http:// page — reaching a desktop peer by address needs none \
                 of this.)"
                    .into(),
            ),
        },
        (None, _) => Check {
            id: "origin",
            level: Level::Unknown,
            detail: "not running in a browser".into(),
            remedy: Some("run this from the browser build; native has no WebRTC.".into()),
        },
    });

    // --- 2. Does the engine actually hand us the API ------------------------
    checks.push(match f.rtc_peer_connection {
        Some(true) => Check {
            id: "webrtc-api",
            level: Level::Ok,
            detail: "RTCPeerConnection is available".into(),
            remedy: None,
        },
        Some(false) => Check {
            id: "webrtc-api",
            level: Level::Fail,
            detail: "RTCPeerConnection is not available in this browser".into(),
            remedy: Some(if insecure {
                // Do not re-diagnose: this is the origin row's consequence, and
                // sending someone to a browser-support page for it wastes the
                // one thing a preflight is for.
                "this follows from the origin above — fix that first.".into()
            } else {
                // Name the half that is lost, not the product. WebRTC is how a
                // peer with NO address is reached; a peer that has one is
                // reached by an ordinary WebSocket and is untouched. Saying
                // "connections are not available here" would send someone to
                // debug a path that works.
                "this engine cannot do WebRTC at all, so a peer with no address \
                 — another browser — cannot be reached from here. A peer that \
                 HAS an address is unaffected: Connect by address still reaches \
                 a desktop install, its shared files, and anything it serves."
                    .to_string()
            }),
        },
        None => Check {
            id: "webrtc-api",
            level: Level::Unknown,
            detail: "not running in a browser".into(),
            remedy: Some(
                "a native peer is reached by its address and needs none of this; \
                 run the preflight in the browser build."
                    .into(),
            ),
        },
    });

    // --- 3. The rendezvous: is one configured, and is it the one we booted --
    //
    // Three distinguishable states, and collapsing any two of them is how a
    // person spends an evening on the wrong half:
    //   nothing configured       -> go add a node
    //   configured, not booted   -> reload (AP22: the registry is right and the
    //                               session has never read it)
    //   booted                   -> say which one, verbatim
    match (&f.booted_node, f.connectors_configured) {
        (None, 0) => checks.push(Check {
            id: "rendezvous",
            level: Level::Fail,
            detail: "no signaling node configured".into(),
            remedy: Some(
                "two browsers that have never met need a third party to swap \
                 offers through. On the machine running Tori: System Overview -> \
                 Rendezvous -> Start, which shows the node's peer id and ws:// \
                 address. Here: `connector add <node-peer-id> <ws://host:port>`, \
                 then `connector use <node-peer-id>`, then reload."
                    .into(),
            ),
        }),
        (None, n) => checks.push(Check {
            id: "rendezvous",
            level: Level::Fail,
            detail: format!("{n} connector(s) configured, none in effect this session"),
            remedy: Some(
                "the node is read once, at boot. Select one with `connector use \
                 <node-peer-id>` and reload the page."
                    .into(),
            ),
        }),
        (Some(n), _) => {
            let label = if n.label.is_empty() {
                String::new()
            } else {
                format!("{} — ", n.label)
            };
            checks.push(Check {
                id: "rendezvous",
                level: Level::Ok,
                detail: format!("{label}{} at {}", n.peer_id, n.addr),
                remedy: None,
            });
        }
    }

    // A drifted provisioning is worth its own row even when the booted one is
    // fine: it is the state where the surface you are editing and the session
    // you are testing disagree, and every symptom of it points at the network.
    if f.provisioning_drifted {
        checks.push(Check {
            id: "rendezvous-drift",
            level: Level::Warn,
            detail: "a reload would rendezvous somewhere else than this session does".into(),
            remedy: Some(
                "reload before testing, or you are testing the previous choice.".into(),
            ),
        });
    }

    // --- 4. The establisher, and WHICH peer holds it ------------------------
    //
    // Primary-only, deliberately. A `meet` from a non-primary peer succeeds
    // completely — discovery is an ordinary call to the node — and hands the
    // counterpart an id with no way to reach back. That failure is silent on
    // both sides, so it gets a row of its own rather than folding into
    // "rendezvous".
    checks.push(if f.establisher_installed {
        Check {
            id: "establisher",
            level: Level::Ok,
            detail: format!("installed on this peer ({})", short(&f.local_peer_id)),
            remedy: None,
        }
    } else if !f.local_is_primary {
        Check {
            id: "establisher",
            level: Level::Fail,
            detail: format!(
                "peer {} is not the primary, and only the primary installs one",
                short(&f.local_peer_id)
            ),
            remedy: Some(
                "meet from a Shell bound to the primary peer. A meet from here \
                 will succeed and hand the other side an id that cannot reach back."
                    .into(),
            ),
        }
    } else if f.booted_node.is_none() {
        Check {
            id: "establisher",
            level: Level::Fail,
            detail: "none installed — no node was resolved at boot".into(),
            remedy: Some("consequence of the rendezvous row above; fix that first.".into()),
        }
    } else {
        // Node resolved, primary peer, still nothing installed: that is not a
        // configuration story, it is the platform refusing. Almost always the
        // origin, which is why the remedy points there rather than guessing.
        Check {
            id: "establisher",
            level: Level::Fail,
            detail: "none installed, though a node WAS resolved at boot".into(),
            remedy: Some(
                "the WebRTC install failed at boot rather than being skipped — \
                 check the origin row, then the console for `webrtc:` lines."
                    .into(),
            ),
        }
    });

    // --- 5. Is the node answering right now --------------------------------
    if let Some(n) = &f.booted_node {
        checks.push(match n.liveness {
            Some("connected") => Check {
                id: "node-link",
                level: Level::Ok,
                detail: "the node is connected".into(),
                remedy: None,
            },
            Some(other) => Check {
                id: "node-link",
                level: Level::Warn,
                detail: format!("the node reads {other}"),
                remedy: Some(format!(
                    "`connector check` dials it. If it stays down, confirm {} is \
                     reachable from THIS machine — same LAN, right port, and the \
                     host's firewall open.",
                    n.addr
                )),
            },
            None => Check {
                id: "node-link",
                level: Level::Unknown,
                detail: "nothing has dialled the node this session".into(),
                // Not a fault: we only dial on demand. Saying so beats an
                // ambiguous dash, and `check` is one word away.
                remedy: Some("`connector check` asks it, and reports what it says.".into()),
            },
        });

        // --- 6. How far the media can travel -------------------------------
        //
        // Zero reflectors is a correct and proven LAN posture (host candidates
        // carry the media, `make e2e-webrtc-lan`) and is silently not a WAN
        // one. That is exactly a limit, not a fault.
        checks.push(if n.reflectors == 0 {
            Check {
                id: "reflectors",
                level: Level::Warn,
                detail: "none — host candidates only".into(),
                remedy: Some(
                    "correct for two machines on one network. Across networks each \
                     side needs to learn its public address: put a `stun:` URI on \
                     the connector row."
                        .into(),
                ),
            }
        } else {
            Check {
                id: "reflectors",
                level: Level::Ok,
                detail: format!("{} configured", n.reflectors),
                remedy: None,
            }
        });

        // A relay is rented infrastructure and the overwhelming majority of
        // runs neither have nor need one, so its absence is stated, never
        // warned about.
        checks.push(Check {
            id: "relay",
            level: Level::Ok,
            detail: if n.relay {
                "configured (a symmetric-NAT pair can still connect)".into()
            } else {
                "none — a pair behind two restrictive NATs will not connect".to_string()
            },
            remedy: None,
        });
    }

    // --- 7. Who this peer is, last: the line you read out loud -------------
    checks.push(Check {
        id: "this-peer",
        level: Level::Ok,
        detail: format!(
            "{} on the {} arm{}",
            f.local_peer_id,
            f.arm,
            if f.local_is_primary { "" } else { " (not primary)" }
        ),
        remedy: None,
    });

    Report { checks }
}

/// First 12 characters, the abbreviation used in logs throughout the app.
fn short(peer_id: &str) -> &str {
    &peer_id[..12.min(peer_id.len())]
}

// ---------------------------------------------------------------------------
// The collector — the only browser-aware code here.
// ---------------------------------------------------------------------------

/// Gather live [`Facts`] from this session.
///
/// `bound_peer_id` is the peer whose id a `meet` from the calling surface would
/// hand out — the Shell's bound peer, not the primary — because the
/// establisher check is about *that* peer.
///
/// The native arm exists so the calling surface is one code path on both
/// targets and can be driven by a native test. It reports the browser facts as
/// `Unknown` rather than as `false`, which is the honest answer: a native build
/// has no origin and no `RTCPeerConnection`, and saying "no" would read as a
/// browser that refused.
#[cfg(not(target_arch = "wasm32"))]
pub fn collect(peers: &crate::peers::Peers, bound_peer_id: &str) -> Facts {
    let system_pid = peers.system_peer_id().to_string();
    Facts {
        secure_context: None,
        origin: "native build (no page)".into(),
        rtc_peer_connection: None,
        arm: peers.arm_of(bound_peer_id),
        local_peer_id: bound_peer_id.to_string(),
        local_is_primary: bound_peer_id == peers.primary_peer_id(),
        establisher_installed: peers.peer_has_webrtc(bound_peer_id),
        booted_node: None,
        provisioning_drifted: false,
        connectors_configured: crate::connectors::read_connectors(peers, &system_pid).len(),
    }
}

/// Say — on the page, at boot — when this origin is one the browser will not
/// let us do peer-to-peer on.
///
/// **This is the single change most likely to save the first real two-machine
/// run.** Serving the app off a desktop and opening it on a laptop or a phone
/// over `http://box.lan:8081` is the obvious way to do it, and it costs you
/// things that fail silently: OPFS (so the Worker bootstrap falls back to
/// Direct) and `getUserMedia` (so the QR scanner is simply absent). All of that
/// gets logged; none of it reaches a person who is not in devtools.
///
/// **There are THREE uniform losses, not two — the service worker is the one
/// this enumeration missed (audited 2026-09-02).** Registration is restricted
/// to secure contexts in every engine, so `'serviceWorker' in navigator` is
/// simply false here and `index.html`'s registration block is skipped without
/// an error. The consequence is **no offline shell**: a phone paired to a
/// desktop over `http://` cannot open the app at all when that desktop sleeps,
/// even though its own tree is sitting in IndexedDB, which does work on an
/// insecure origin. It also means none of the service-worker heal path — the
/// hotfix route, the kill switch, the recovery console's registration
/// enumeration — has anything to act on at this origin. That is the *safe*
/// direction for staleness (no cache, so no stale cache) and a real loss of
/// availability, and those are different facts.
///
/// **The banner names all three as of 2026-09-03.** It named two for a day,
/// deliberately: the `readiness.insecure_origin` string is translated into 30
/// locales and the i18n parity check is by KEY, not by content, so editing the
/// English value silently leaves 30 stale translations and no gate would say
/// so. It was recorded as owed rather than half-done, and paid in the release's
/// translation pass — the English and all 30 overlays moved in one commit,
/// which is the only shape that debt has.
///
/// **It does NOT categorically cost you WebRTC, and this banner used to say it
/// did.** Measured against Firefox 149 at an http LAN origin,
/// `RTCPeerConnection` constructs and gathers host candidates normally. Engines
/// differ, so the banner states the two losses that are uniform and defers the
/// engine-specific question to [`assess`], which measures it.
///
/// A banner rather than a refusal, deliberately: the rest of the app works
/// perfectly on an insecure origin (the tree, the site browser, publishing),
/// and refusing to boot over a limitation would be a bigger lie than saying
/// nothing. Dismissible, idempotent, no Rust `Closure` (D12) — the same
/// primitive the storage banners use.
#[cfg(target_arch = "wasm32")]
pub fn warn_if_insecure_origin() {
    let Some(win) = web_sys::window() else { return };
    if win.is_secure_context() {
        return;
    }
    let origin = win.location().origin().unwrap_or_default();
    tracing::warn!(
        %origin,
        "insecure origin — OPFS (Worker storage), getUserMedia (QR scanning) and \
         the SERVICE WORKER (so no offline shell: this origin cannot open the app \
         when the machine serving it is unreachable) are unavailable here. WebRTC \
         availability is engine-specific and is measured by the `net` preflight. \
         Use https:// in production."
    );
    crate::storage_durability::inject_banner_with_id(
        "insecure-origin-banner",
        &crate::i18n::t("readiness.insecure_origin", &[("origin", &origin)]),
        "#4a1e1e",
        "#a23a3a",
    );
}

/// **No `RTCPeerConnection` — say so at boot, on the screen.**
///
/// This is the second half of the rule the insecure-origin banner exists for,
/// and it was learned the hard way: the `webrtc-api` row has been in the `net`
/// preflight for weeks, correctly reporting `FAIL` on the desktop, and it took
/// three sessions to find because **nobody runs a preflight on a device that
/// appears to be working**. The desktop's WebView shipped with WebKitGTK's
/// `enable-webrtc` off (its default), so `RTCPeerConnection` did not exist —
/// and the visible half of the product kept working, because `meet` is an
/// ordinary WebSocket call to the rendezvous node. Two devices found each
/// other, showed each other's ids, and could never connect, in either
/// direction, with no error naming the cause.
///
/// A precondition that fails silently on a user's machine owes a **surface**,
/// not a row in a report they have no reason to open.
///
/// **The banner names the half that is lost, and its first wording did not.**
/// It said devices could still be found but "nothing will connect, in either
/// direction", which is false: WebRTC is how a peer with *no address* is
/// reached, and a peer that has one is reached by an ordinary WebSocket. On a
/// desktop with no WebRTC, connecting to another install by address, browsing
/// and moving files through its shared folder, and everything served over that
/// connection all keep working. A banner that overstates the damage costs the
/// reader the paths that are fine — the cry-wolf shape, arriving as pessimism
/// instead of as a false alarm.
///
/// It is deliberately not gated on being a desktop: a browser with WebRTC
/// disabled by policy or by the user is the same product, equally limited, and
/// equally entitled to be told.
#[cfg(target_arch = "wasm32")]
pub fn warn_if_no_webrtc_api() {
    let Some(win) = web_sys::window() else { return };
    // Feature-detect on the global rather than constructing one — the same
    // reason `collect` does: a construction attempt throws in one engine and
    // returns a crippled object in another, and the question here is only
    // whether the API exists.
    let present =
        js_sys::Reflect::get(win.as_ref(), &wasm_bindgen::JsValue::from_str("RTCPeerConnection"))
            .map(|v| v.is_function())
            .unwrap_or(false);
    if present {
        return;
    }
    tracing::error!(
        "no RTCPeerConnection — this session cannot establish a §6.5 connection in either \
         direction, so no peer that lacks a dialable address (i.e. another browser) is \
         reachable from here. Rung 1/2 of the §10.3 ladder is UNAFFECTED: a peer with a \
         published address is still reached over WebSocket, which is why file transfer to \
         a desktop's share keeps working. Meeting a device also still works (an ordinary \
         WebSocket call to the rendezvous), which is why this presents as 'we found each \
         other and nothing connects'. On the Linux desktop the cause is that WebKitGTK is \
         built without the WebRTC bindings."
    );
    crate::storage_durability::inject_banner_with_id(
        "no-webrtc-banner",
        &crate::i18n::t("readiness.no_webrtc_api", &[]),
        "#4a1e1e",
        "#a23a3a",
    );
}

#[cfg(target_arch = "wasm32")]
pub fn collect(peers: &crate::peers::Peers, bound_peer_id: &str) -> Facts {
    let win = web_sys::window();
    let secure_context = win.as_ref().map(|w| w.is_secure_context());
    let origin = win
        .as_ref()
        .and_then(|w| w.location().href().ok())
        .unwrap_or_default();
    // Feature-detect by looking the constructor up on the global rather than
    // constructing one: a construction attempt on a page that forbids it throws
    // in one engine and returns a crippled object in another, and we only want
    // to know whether the API is there.
    let rtc_peer_connection = win.as_ref().map(|w| {
        js_sys::Reflect::get(w.as_ref(), &wasm_bindgen::JsValue::from_str("RTCPeerConnection"))
            .map(|v| v.is_function())
            .unwrap_or(false)
    });

    let system_pid = peers.system_peer_id().to_string();
    let booted = crate::connectors::booted_snapshot();
    let rows = crate::connectors::read_connectors(peers, &system_pid);

    let booted_node = booted.as_ref().map(|p| {
        // Match the booted node against the registry row so the label and the
        // relay flag come from the row the user edits, while the id/addr and
        // reflector COUNT come from what the session actually resolved. A row
        // edited since boot must not make the session look freshly configured.
        let row = rows.iter().find(|r| r.node_peer_id == p.node_peer_id);
        NodeFacts {
            peer_id: p.node_peer_id.clone(),
            addr: p.node_addr.clone(),
            label: row.map(|r| r.label.clone()).unwrap_or_default(),
            // URLs, not entries. `parse_ice_urls` packs every reflector into
            // ONE credential-free `IceServer`, so an entry count here is 0-or-1
            // by construction and would read "1 configured" for a list of six —
            // the same distinction `e2e-webrtc-advertised` asserts on `ice_urls`
            // rather than `ice_servers`. The relay is excluded because it is its
            // own row and is not a reflector (`IceServer::is_relay`, the one
            // discriminator).
            reflectors: p.ice_servers.iter().filter(|s| !s.is_relay()).map(|s| s.urls.len()).sum(),
            relay: p.has_relay(),
            liveness: match crate::peer_liveness::liveness_of(peers, &p.node_peer_id) {
                crate::peer_liveness::LiveStatus::Connected => Some("connected"),
                crate::peer_liveness::LiveStatus::Suspect => Some("suspect"),
                crate::peer_liveness::LiveStatus::Disconnected => Some("disconnected"),
                crate::peer_liveness::LiveStatus::Unknown => None,
            },
        }
    });

    let would_use =
        crate::connectors::resolve_provisioning_quietly(&crate::app::webrtc_url_query())
            .map(|(p, _)| p);

    Facts {
        secure_context,
        origin,
        rtc_peer_connection,
        arm: peers.arm_of(bound_peer_id),
        local_peer_id: bound_peer_id.to_string(),
        local_is_primary: bound_peer_id == peers.primary_peer_id(),
        establisher_installed: peers.peer_has_webrtc(bound_peer_id),
        provisioning_drifted: crate::connectors::provisioning_drifted(
            booted.as_ref(),
            would_use.as_ref(),
        ),
        connectors_configured: rows.len(),
        booted_node,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> NodeFacts {
        NodeFacts {
            peer_id: "2KNodeSeven".into(),
            addr: "ws://192.168.1.10:4041".into(),
            label: "my box".into(),
            reflectors: 0,
            relay: false,
            liveness: Some("connected"),
        }
    }

    fn healthy() -> Facts {
        Facts {
            secure_context: Some(true),
            origin: "http://localhost:8081/".into(),
            rtc_peer_connection: Some(true),
            arm: "direct",
            local_peer_id: "2KLocalPeerAbcdef".into(),
            local_is_primary: true,
            establisher_installed: true,
            booted_node: Some(node()),
            provisioning_drifted: false,
            connectors_configured: 1,
        }
    }

    fn find<'a>(r: &'a Report, id: &str) -> &'a Check {
        r.checks
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("no `{id}` check in {:?}", r.checks.iter().map(|c| c.id).collect::<Vec<_>>()))
    }

    /// The whole surface is only worth something if a reader can act on it, and
    /// "act on it" means the row that is not `Ok` says what to do. Enforced
    /// here rather than left to review, because the failure mode of a
    /// diagnostic is silently becoming a list of nouns.
    #[test]
    fn every_non_ok_check_carries_a_remedy() {
        // Sweep the interesting corners rather than one fixture: a remedy added
        // for the happy-path variant of a check says nothing about the arm that
        // actually fires.
        let mut cases = vec![healthy(), Facts::blank()];
        let mut insecure = healthy();
        insecure.secure_context = Some(false);
        insecure.rtc_peer_connection = Some(false);
        cases.push(insecure);
        // ...and the arm that measures differently: insecure, WebRTC present.
        // A `Warn` owes a remedy exactly as a `Fail` does, and this arm was
        // added after the old one, so sweeping only the old one would leave the
        // new row's remedy unchecked.
        let mut insecure_with_rtc = healthy();
        insecure_with_rtc.secure_context = Some(false);
        insecure_with_rtc.rtc_peer_connection = Some(true);
        cases.push(insecure_with_rtc);
        let mut unbooted = healthy();
        unbooted.booted_node = None;
        unbooted.establisher_installed = false;
        cases.push(unbooted.clone());
        unbooted.connectors_configured = 0;
        cases.push(unbooted);
        let mut secondary = healthy();
        secondary.local_is_primary = false;
        secondary.establisher_installed = false;
        cases.push(secondary);
        let mut down = healthy();
        down.booted_node = Some(NodeFacts { liveness: Some("disconnected"), ..node() });
        cases.push(down);
        let mut drift = healthy();
        drift.provisioning_drifted = true;
        cases.push(drift);

        for f in cases {
            for c in assess(&f).checks {
                if c.level != Level::Ok {
                    assert!(
                        c.remedy.is_some(),
                        "check `{}` is {:?} and tells the reader nothing to do",
                        c.id,
                        c.level
                    );
                }
            }
        }
    }

    /// The happy path is `Ok` everywhere except the one row that is honestly a
    /// limit — a preflight that cannot go green is a preflight nobody reads.
    #[test]
    fn a_healthy_lan_session_is_ready_with_exactly_one_limit() {
        let r = assess(&healthy());
        assert_eq!(find(&r, "origin").level, Level::Ok);
        assert_eq!(find(&r, "establisher").level, Level::Ok);
        // Zero reflectors on a LAN is the proven posture (`e2e-webrtc-lan`), so
        // it must not read as a fault — and must not read as unqualified `Ok`
        // either, because it is exactly what stops working between houses.
        assert_eq!(find(&r, "reflectors").level, Level::Warn);
        assert_eq!(r.worst(), Level::Warn);
        assert!(r.headline().is_none(), "nothing is broken, so nothing is the headline");

        let with_stun = Facts {
            booted_node: Some(NodeFacts { reflectors: 1, ..node() }),
            ..healthy()
        };
        let r = assess(&with_stun);
        assert_eq!(r.worst(), Level::Ok, "a reflector removes the last limit: {:?}", r.lines());
    }

    /// The origin is the root cause the whole module exists for, and the two
    /// rows it poisons must point back at it instead of re-diagnosing.
    #[test]
    fn an_insecure_origin_is_the_headline_and_the_api_row_defers_to_it() {
        let f = Facts {
            secure_context: Some(false),
            origin: "http://desk.lan:8081/".into(),
            rtc_peer_connection: Some(false),
            establisher_installed: false,
            ..healthy()
        };
        let r = assess(&f);
        let head = r.headline().expect("an insecure origin is a failure");
        assert_eq!(head.id, "origin", "the FIRST failure must be the cause, not a consequence");
        assert!(
            head.detail.contains("desk.lan"),
            "the origin itself is half the remedy: {}",
            head.detail
        );
        let api = find(&r, "webrtc-api");
        assert!(
            api.remedy.as_deref().unwrap().contains("origin above"),
            "the consequence must defer upward, not send the reader to a browser \
             support page: {:?}",
            api.remedy
        );
    }

    /// **The engine, not the spec, decides what an insecure origin costs.**
    ///
    /// This row used to assert that an insecure origin disables WebRTC and to
    /// grade itself `Fail` on that assertion. Measured against Firefox 149 at
    /// `http://192.168.68.55:8099/`: `RTCPeerConnection` constructs and gathers
    /// host candidates (2, mDNS-obfuscated) — so the assertion was false, and
    /// the report printed it directly above its own measured `OK webrtc-api`.
    ///
    /// A preflight that contradicts itself on adjacent lines is worse than one
    /// that says less: the reader cannot tell which line to believe, so they
    /// believe neither. Grade by the consequence the browser exhibits.
    #[test]
    fn an_insecure_origin_that_still_has_webrtc_warns_instead_of_failing() {
        let f = Facts {
            secure_context: Some(false),
            origin: "http://192.168.68.55:8099/".into(),
            rtc_peer_connection: Some(true),
            ..healthy()
        };
        let r = assess(&f);
        let origin = find(&r, "origin");
        assert_eq!(
            origin.level,
            Level::Warn,
            "an origin whose only measured cost is opt-in storage is a limit, \
             not a fault: {:?}",
            origin
        );
        assert!(
            r.headline().is_none(),
            "nothing here stops the reader connecting, so nothing should be \
             headlined as a failure: {:?}",
            r.lines()
        );
        // The remedy must name what is ACTUALLY gone. Both of these are
        // uniformly secure-context-gated in every engine, so they are sayable
        // from `secure_context` alone — unlike WebRTC, which is not.
        let remedy = origin.remedy.as_deref().unwrap_or_default();
        assert!(
            remedy.contains("QR"),
            "the camera is the ironic casualty — the affordance that exists so \
             nobody retypes a peer id is the one that dies here: {remedy}"
        );
        assert!(
            remedy.contains("https://"),
            "production is the place this genuinely must be fixed: {remedy}"
        );
    }

    /// The general form of the bug above, swept rather than fixtured: **the
    /// report may never diagnose a cause whose effect it just measured as
    /// absent.** If `webrtc-api` came back `Ok`, no row may be `Fail` on the
    /// grounds that WebRTC is unavailable.
    #[test]
    fn no_row_fails_for_a_consequence_the_report_measured_as_fine() {
        let mut cases = vec![healthy()];
        for rtc in [Some(true), Some(false)] {
            for secure in [Some(true), Some(false)] {
                cases.push(Facts { secure_context: secure, rtc_peer_connection: rtc, ..healthy() });
            }
        }
        for f in cases {
            let r = assess(&f);
            if find(&r, "webrtc-api").level != Level::Ok {
                continue;
            }
            let origin = find(&r, "origin");
            assert_ne!(
                origin.level,
                Level::Fail,
                "webrtc-api measured OK, so the origin row must not fail the \
                 session over WebRTC. facts: secure={:?} rtc={:?} row={:?}",
                f.secure_context,
                f.rtc_peer_connection,
                origin
            );
        }
    }

    /// AP22, as a row: the registry is correct and the session never read it.
    /// "Nothing configured" and "configured, not in effect" send a person to
    /// two different places, so they must not render the same.
    #[test]
    fn a_configured_but_unbooted_node_says_reload_not_add() {
        let f = Facts {
            booted_node: None,
            establisher_installed: false,
            connectors_configured: 2,
            ..healthy()
        };
        let r = assess(&f);
        let rz = find(&r, "rendezvous");
        assert_eq!(rz.level, Level::Fail);
        assert!(rz.detail.contains('2'), "say how many are configured: {}", rz.detail);
        assert!(
            rz.remedy.as_deref().unwrap().contains("reload"),
            "the action is a reload: {:?}",
            rz.remedy
        );

        let none = Facts { connectors_configured: 0, ..f };
        let r = assess(&none);
        let rz = find(&r, "rendezvous");
        assert!(
            rz.remedy.as_deref().unwrap().contains("connector add"),
            "with nothing configured the action is to add one: {:?}",
            rz.remedy
        );
    }

    /// The silent one. A meet from a non-primary peer succeeds and hands over
    /// an unreachable id; the establisher row is the only place that says so
    /// before it happens.
    #[test]
    fn a_non_primary_peer_is_told_its_meet_would_hand_out_a_dead_id() {
        let f = Facts {
            local_is_primary: false,
            establisher_installed: false,
            ..healthy()
        };
        let report = assess(&f);
        let est = find(&report, "establisher");
        assert_eq!(est.level, Level::Fail);
        assert!(
            est.detail.contains("not the primary"),
            "name the actual cause: {}",
            est.detail
        );
        assert!(est.remedy.as_deref().unwrap().contains("cannot reach back"));
    }

    /// A node that was never dialled is `Unknown`, never `Ok` and never a
    /// failure — the cold state of a working session. Collapsing it into either
    /// is the seam this repo keeps meeting.
    #[test]
    fn an_undialled_node_is_unknown_rather_than_ok_or_broken() {
        let f = Facts {
            booted_node: Some(NodeFacts { liveness: None, ..node() }),
            ..healthy()
        };
        let r = assess(&f);
        assert_eq!(find(&r, "node-link").level, Level::Unknown);
        assert!(r.headline().is_none(), "not knowing is not a failure");
        assert_eq!(r.worst(), Level::Unknown, "but it does outrank a mere limit");
    }

    /// The report is read by a person on another machine, over a chat window.
    /// Every check must appear in the rendered text, and a remedy must appear
    /// under the check it belongs to.
    #[test]
    fn the_rendered_lines_carry_every_check_and_its_remedy() {
        let r = assess(&healthy());
        let text = r.lines().join("\n");
        for c in &r.checks {
            assert!(text.contains(c.id), "`{}` is missing from the render", c.id);
        }
        assert!(text.contains("host candidates only"));
        assert!(text.contains("-> correct for two machines"), "{text}");
    }
}
