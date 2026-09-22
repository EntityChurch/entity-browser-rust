//! Why a peer could not be reached — the classifier.
//!
//! **Design of record:** `docs/architecture/reviews/DESIGN-REACHABILITY-DIAGNOSIS-AND-THE-RELAY-DECISION.md`.
//!
//! # The problem this exists to fix
//!
//! Before this module, `src/` contained no reference to `srflx`, `relay` or
//! `candidate_type` outside doc comments — nothing in the app had ever looked at
//! what its own ICE agent gathered. So "no reflector configured", "the reflector
//! is down", "both sides are behind restrictive NATs" and "your friend closed
//! their laptop" all failed **identically**, and identically to each other. The
//! user was told a peer is not connected; everything about *why*, and therefore
//! everything they could do about it, was discarded one layer below the surface.
//!
//! That matters more than a usual diagnostics story: the fix for the hardest
//! topologies is a **relay the user points at**, and a user who cannot tell
//! *"this network needs a relay"* from *"my friend is offline"* will never go
//! looking for one. This is what makes a TURN field worth adding.
//!
//! # The observation is exactly the topology
//!
//! An ICE agent's gathered candidate types are a direct read of the network you
//! are on, and we already have them: a gathered candidate arrives as its full
//! SDP line, whose `typ` token *is* the type. Nothing had to be plumbed to learn
//! it — only read.
//!
//! # Two rules that constrain every arm below
//!
//! **Do not cry wolf.** Every WebRTC gate asserts, twice per run on both sides,
//! that *"a header raises no false unreachable note"*. Establishment legitimately
//! takes seconds, and a peer that is merely not connected *yet* must never be
//! reported as unreachable. So the classification is structurally unable to
//! speak before an attempt has actually failed: [`Outcome::InFlight`] maps to
//! [`Reachability::Unknown`], which renders nothing. That is enforced by the
//! match arm, not by a comment at the call site.
//!
//! **Describe our side; infer the pair only from the outcome.** Our agent's
//! candidates say nothing about the far side's. We cannot observe whether the
//! other end is symmetric, and a confident wrong diagnosis is worse than a vague
//! right one — which is why [`Reachability::NoDirectPath`] is worded as
//! *"neither device can be reached directly"* rather than naming a culprit.

/// One ICE candidate type, as the `typ` token spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CandidateKind {
    /// A local interface address (or its mDNS `.local` alias — Chrome and
    /// Firefox both obfuscate by default, and `make e2e-webrtc-lan` proves two
    /// browsers connect on those alone).
    Host,
    /// Server-reflexive: a reflector told us our external address.
    ServerReflexive,
    /// Peer-reflexive. Discovered **during connectivity checks**, not during
    /// gathering, and it is what makes symmetric↔cone work. Treated as
    /// equivalent to [`Self::ServerReflexive`] by [`GatheredTypes::has_reflexive`]
    /// — a classifier reading only the initial gather would under-report.
    PeerReflexive,
    /// A relay is forwarding for us.
    Relay,
}

impl CandidateKind {
    /// Parse the `typ` token out of one SDP candidate line.
    ///
    /// The line looks like
    /// `candidate:0 1 UDP 2122252543 192.168.1.5 60044 typ host generation 0`,
    /// and everything before `typ` is positional fields we do not need. Reading
    /// the token *after* `typ` rather than a fixed index is deliberate: the
    /// prefix has optional components (`tcptype`, `raddr`/`rport` on reflexive
    /// candidates) and a positional read drifts the moment one appears.
    ///
    /// Returns `None` for a line with no `typ` token or an unknown type — an
    /// unrecognised type is **not** guessed at, because every arm downstream is
    /// a claim to a user.
    pub fn from_sdp_line(line: &str) -> Option<Self> {
        let mut parts = line.split_whitespace();
        while let Some(tok) = parts.next() {
            if tok == "typ" {
                return match parts.next()? {
                    "host" => Some(Self::Host),
                    "srflx" => Some(Self::ServerReflexive),
                    "prflx" => Some(Self::PeerReflexive),
                    "relay" => Some(Self::Relay),
                    _ => None,
                };
            }
        }
        None
    }
}

/// What our ICE agent gathered for one negotiation.
///
/// A set rather than a single value: an agent behind a NAT with a reflector
/// gathers host *and* srflx, and the interesting questions are all about which
/// kinds are present together.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GatheredTypes {
    kinds: Vec<CandidateKind>,
}

impl GatheredTypes {
    /// Fold a batch of raw SDP candidate lines into the set of types present.
    /// Unparseable lines are dropped rather than counted as anything.
    pub fn from_sdp_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Self {
        let mut kinds: Vec<CandidateKind> =
            lines.into_iter().filter_map(CandidateKind::from_sdp_line).collect();
        kinds.sort();
        kinds.dedup();
        Self { kinds }
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    pub fn contains(&self, k: CandidateKind) -> bool {
        self.kinds.contains(&k)
    }

    /// Did we learn an address beyond our own LAN? **`prflx` counts** — see
    /// [`CandidateKind::PeerReflexive`].
    pub fn has_reflexive(&self) -> bool {
        self.contains(CandidateKind::ServerReflexive)
            || self.contains(CandidateKind::PeerReflexive)
    }

    /// The types present, sorted and deduped — for logging and for a gate that
    /// wants to assert on what was actually seen.
    pub fn kinds(&self) -> &[CandidateKind] {
        &self.kinds
    }
}

/// How far the establishment attempt got.
///
/// [`Self::InFlight`] is not a failure and must never classify — it is the
/// don't-cry-wolf guard in type form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing has been tried for this peer yet.
    NotAttempted,
    /// A negotiation is running. Candidates may still be gathering.
    InFlight,
    /// A negotiation ran and no path was established.
    Failed,
    /// We have a live path.
    Connected,
}

/// What we observed about one attempt to reach one peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub gathered: GatheredTypes,
    /// Whether this session was configured with **any** reflector at all. Comes
    /// from the provisioning the establisher was built with, not from the
    /// candidates — it is the difference between "we asked and got nothing" and
    /// "we never asked", which is the whole of `NoReflector` vs
    /// `ReflectorUnreachable`.
    pub reflectors_configured: bool,
    pub outcome: Outcome,
    /// Did the SDP exchange complete — did the far side answer at all?
    ///
    /// **The fact this module shipped without, and the one that made its worst
    /// arm wrong.** Every verdict below except [`Reachability::NoCounterpart`]
    /// is a claim about *the network between two peers*, and that network is
    /// only exercised once both peers are talking. Reproduced in a real browser
    /// (2026-08-22): a chat opened against a peer that was simply not running
    /// gathered `[Host]`, failed, and was told *"no reflector is set up, so this
    /// app can only reach devices on your local network"* — false (both were on
    /// one machine), unactionable (a reflector fixes nothing here), and pointing
    /// at the one thing the user could go waste money on.
    ///
    /// `None` = **not measurable**, never `false`: a carrier error or a policy
    /// refusal ends the negotiation before this can be known, and reading
    /// absence as "they did not answer" would blame a peer for our own refusal.
    /// The classifier treats `None` as *carry on* — today's behaviour exactly.
    pub sdp_exchange_complete: Option<bool>,
}

/// The verdict. Only three arms ever reach a user; the rest render nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reachability {
    /// Say nothing. Not attempted, in flight, connected, or a failure we cannot
    /// honestly explain.
    Unknown,
    /// A live path exists.
    Connected,
    /// The far side never answered. **Not a network verdict** — the SDP
    /// exchange never completed, so nothing about the path between the two
    /// peers was ever tried, and every other arm here would be a guess about a
    /// network that was not exercised. Actionable, and by the other person:
    /// they have to be online, in this app, through the same rendezvous.
    NoCounterpart,
    /// Host candidates only, and no reflector was ever configured — we can only
    /// reach peers on this LAN. Actionable: add a reflector, or pick a node that
    /// advertises one.
    NoReflector,
    /// A reflector *was* configured and we still gathered nothing beyond host —
    /// it did not answer. Actionable: fix the URL, or the operator is down.
    ReflectorUnreachable,
    /// We know our external address and still could not nominate a pair. This is
    /// the arm that earns the feature: it is the only honest way to say *"neither
    /// of you can be reached directly"*, and it is what a relay fixes.
    NoDirectPath,
}

impl Reachability {
    /// Whether this verdict should be shown at all. `Unknown` and `Connected`
    /// render nothing — the existing connection state already says everything
    /// true about them.
    pub fn is_advisory(self) -> bool {
        matches!(
            self,
            Self::NoCounterpart
                | Self::NoReflector
                | Self::ReflectorUnreachable
                | Self::NoDirectPath
        )
    }

    /// A stable, non-user-facing token — for logs and for a gate that wants to
    /// assert the classification without depending on translated prose.
    pub fn as_token(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Connected => "connected",
            Self::NoCounterpart => "no-counterpart",
            Self::NoReflector => "no-reflector",
            Self::ReflectorUnreachable => "reflector-unreachable",
            Self::NoDirectPath => "no-direct-path",
        }
    }
}

/// Classify one observation. Pure — the table in the design doc *is* this
/// function, and its tests.
pub fn classify(obs: &Observation) -> Reachability {
    match obs.outcome {
        // The don't-cry-wolf guard, structural. Establishment takes seconds and
        // a peer that is not connected YET is not a peer that cannot be reached.
        Outcome::NotAttempted | Outcome::InFlight => Reachability::Unknown,
        Outcome::Connected => Reachability::Connected,
        Outcome::Failed => {
            if obs.sdp_exchange_complete == Some(false) {
                // **Nobody answered, so there is no network fact here to
                // report.** Every network arm below reasons about the path
                // *between two peers*; that path is only exercised once both are
                // talking, and an exchange that never completed did not exercise
                // it. The cheaper, truer claim has to win, or a richer-sounding
                // one is made about a network nothing touched. This is the arm
                // the operator's report earned — "no reflector is set up" said
                // about a peer on the same machine that was not running.
                //
                // `== Some(false)`, deliberately, so `None` (not measurable)
                // falls through to exactly today's behaviour rather than
                // becoming a silent accusation.
                //
                // ⭐ **It sits above the empty-gather arm as of 2026-09-16, and
                // that is a REVERSAL of an ordering this module used to assert
                // on purpose** ("an agent that gathered nothing is about US, so
                // it outranks a statement about them"). The reversal is not a
                // change of principle — it is that the earlier case analysis was
                // missing the case that produces almost all of these.
                //
                // `AUDIT-2026-09-15-a`: ~100 consecutive negotiations, every one
                // `role=answerer, sdp_exchange=INCOMPLETE, bucket=0 msg(s),
                // candidates posted=0/fed=0`. An **answerer with no offer to
                // answer never calls `create_answer`, so gathering never
                // starts** — the empty gather is a *consequence* of the
                // counterpart's absence, not an independent fault of ours, and
                // classifying it as *"we cannot say"* withheld the one true,
                // actionable sentence for the whole life of that session. The
                // audit recorded the storm as "never reaching an honest terminal
                // *I cannot reach them*"; this arm is that terminal.
                //
                // **What makes the reversal safe is that `Some(false)` already
                // excludes the paths where our own machinery broke.** Only
                // `WebRtcError::Timeout` carries `answered`, and `Timeout` means
                // the negotiation loop ran to its deadline — a `create_offer` /
                // `create_answer` that threw is a substrate error, reported as
                // `None`. So `Some(false)` is a fact about the *pair* reaching a
                // deadline with neither side closing the exchange, which an
                // empty local gather cannot contradict.
                //
                // The empty-gather arm keeps its precedence over every arm that
                // makes a claim about the **network**, which is what it was
                // earned against — see below.
                Reachability::NoCounterpart
            } else if obs.gathered.is_empty() {
                // Not even a host candidate, and the exchange did not tell us
                // nobody was there. That is not a topology fact about the
                // network between two peers — it is an agent that never
                // gathered — so we have nothing honest to say. **This arm has to
                // come above every network arm below it**: it was written under
                // the `reflectors_configured` check at first, which classified a
                // never-started agent as `ReflectorUnreachable` and would have
                // sent the user to fix a reflector that was never asked.
                // `gathering_nothing_at_all_is_not_a_topology_claim` caught it.
                Reachability::Unknown
            } else if obs.gathered.contains(CandidateKind::Relay) {
                // A relay gathered and it still failed. Do NOT say "this network
                // needs a relay" — they have one. The likeliest remaining cause
                // is that the far side is genuinely not there, which is exactly
                // what the existing "not connected" already says, so add
                // nothing. §3.3: never promise a relay will fix it.
                Reachability::Unknown
            } else if obs.gathered.has_reflexive() {
                Reachability::NoDirectPath
            } else if obs.reflectors_configured {
                // We asked a reflector and learned nothing beyond our own LAN.
                Reachability::ReflectorUnreachable
            } else {
                Reachability::NoReflector
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

/// Last-known verdict per remote peer.
///
/// # This is `dial_markers.rs`'s shape, and that is not a free choice
///
/// Connection **liveness** is kernel-owned and subscribed (`system/peer/status`,
/// the `peer_liveness` read-model). This repo has already deleted one app-tier
/// liveness mirror that guessed from connect *attempts* and lied "Connected"
/// straight through a mid-session drop, and the standing rule earned from it is:
/// **never write a fourth parallel liveness store.**
///
/// This is not liveness. It is a local, in-flight, diagnostic fact about **our
/// own ICE agent** — meaningless on another peer, meaningless across a reload,
/// and never an input to whether we are connected. `conn_display` and every
/// existing surface keep resolving connection state through the kernel status,
/// unchanged; this only ever *adds a sentence* to a state something else
/// decided. That is precisely the exception `dial_markers.rs` already occupies,
/// so it lives here: in memory, dropped on reload without ceremony.
///
/// **If a future reader finds this in the tree, that is the bug.**
mod store {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Bumped on every change to the map — **this is the module's dirty
    /// signal, and it is not optional.**
    ///
    /// A verdict is written from inside the establisher's async negotiation,
    /// which runs off the frame loop: no tree write happens, so no
    /// `WindowWatch` fires, and a surface reading this map can go arbitrarily
    /// long showing nothing while the answer sits right here. That is the
    /// repo's standing rule — *every render input must carry its own dirty
    /// signal* — and it is the exact shape that froze the theme dropdown on a
    /// stale list.
    ///
    /// It was also not hypothetical here: the first `make e2e-webrtc-nat` run
    /// against this feature showed ~30 completed failed negotiations per side
    /// and the note rendered on **one** browser, because only that one happened
    /// to be repainted by something unrelated. Same counter/compare shape as
    /// `i18n::locale_generation`, consumed once per frame by `dom::mod`.
    static GENERATION: AtomicU64 = AtomicU64::new(0);

    pub(super) fn generation() -> u64 {
        GENERATION.load(Ordering::Relaxed)
    }

    thread_local! {
        /// Keyed by remote peer-id (Base58, the app's key space — the kernel's
        /// identity-hash hex is a different one; see `MODEL-REMOTE-PEER-FACTS`).
        static VERDICTS: RefCell<HashMap<String, Reachability>> =
            RefCell::new(HashMap::new());
    }

    /// Drop any sentence we were adding about `peer_id` — what a working
    /// connection earns. Separate from `set` because "store nothing" and
    /// "remove what is stored" are different acts, and only one of them is what
    /// [`Record::Keep`] must NOT do.
    pub(super) fn forget(peer_id: &str) {
        VERDICTS.with(|m| {
            let mut m = m.borrow_mut();
            if m.remove(peer_id).is_some() {
                GENERATION.fetch_add(1, Ordering::Relaxed);
            }
        });
    }

    pub(super) fn set(peer_id: &str, v: Reachability) {
        VERDICTS.with(|m| {
            let mut m = m.borrow_mut();
            // **Only advice is stored**, and that is a boundary, not a
            // micro-optimisation. Keeping a `Connected` row here would make this
            // map a second opinion about whether a peer is reachable — which is
            // the `connection_health` mirror disease this module's header
            // promises not to reintroduce. Liveness has exactly one home (the
            // kernel's `system/peer/status`); this holds only the *sentence we
            // are allowed to add* to it, and "nothing to add" is an absent row.
            let before = m.get(peer_id).copied();
            if v.is_advisory() {
                m.insert(peer_id.to_string(), v);
            } else {
                m.remove(peer_id);
            }
            // Signal only on a real change: a failing peer is re-negotiated
            // every few seconds and re-recording the same verdict must not
            // force a full-window rebuild at that cadence.
            if before != m.get(peer_id).copied() {
                GENERATION.fetch_add(1, Ordering::Relaxed);
            }
        });
    }

    thread_local! {
        /// Peers that have completed an SDP exchange with us at least once this
        /// session — i.e. peers we have *heard from*.
        ///
        /// **Sticky, and that is the point.** A peer that is present but
        /// unreachable produces a MIXTURE: some negotiations complete the
        /// exchange and fail at ICE, others never correlate at all (a busy
        /// bucket, a retry storm, a window that closed first). Classifying the
        /// most recent one alone makes the verdict flap between *"the network
        /// could not carry it"* and *"nobody was there"* on a coin toss — and
        /// the second is strictly less true once you have heard from them.
        ///
        /// Measured in `make e2e-webrtc-nat`, which is exactly that topology:
        /// B's FIRST negotiation was `sdp_exchange=complete, fed=3` and a later
        /// one `INCOMPLETE, fed=0`, so a per-negotiation reading told one
        /// browser the reflector was missing and the other that its perfectly
        /// live counterpart had not answered.
        ///
        /// Known limit, recorded rather than papered over: a peer that answered
        /// and then genuinely left keeps the network verdict for the rest of the
        /// session. That is the pre-existing behaviour, the chip already says
        /// *not connected*, and the alternative — expiring the fact — would put
        /// this map back in the business of tracking liveness.
        static ANSWERED_EVER: RefCell<std::collections::HashSet<String>> =
            RefCell::new(std::collections::HashSet::new());
    }

    /// Record whether `peer_id` answered in *this* negotiation, and return
    /// whether it has **ever** answered.
    pub(super) fn note_exchange(peer_id: &str, answered_now: bool) -> bool {
        ANSWERED_EVER.with(|s| {
            let mut s = s.borrow_mut();
            if answered_now {
                s.insert(peer_id.to_string());
            }
            s.contains(peer_id)
        })
    }

    pub(super) fn get(peer_id: &str) -> Reachability {
        VERDICTS.with(|m| m.borrow().get(peer_id).copied().unwrap_or(Reachability::Unknown))
    }

    #[cfg(test)]
    pub(super) fn clear() {
        VERDICTS.with(|m| m.borrow_mut().clear());
        // Both maps, or a test that recorded an answered exchange leaks that
        // fact into the next one and the `NoCounterpart` arm becomes
        // unreachable — silently, and only in whichever test runs second.
        ANSWERED_EVER.with(|s| s.borrow_mut().clear());
    }
}

/// What we last learned about reaching `peer_id`. [`Reachability::Unknown`] when
/// we have learned nothing — which is the default and renders nothing.
pub fn verdict_for(peer_id: &str) -> Reachability {
    store::get(peer_id)
}

/// Monotonic counter, bumped whenever any verdict changes. Read once per frame
/// by the DOM loop, which forces a rebuild when it moves.
///
/// **Required, not an optimisation.** See [`store`]'s `GENERATION`: verdicts are
/// written off the frame loop from inside an async negotiation, so no
/// `WindowWatch` fires and a surface that reads this map has no other way to
/// learn it changed.
pub fn generation() -> u64 {
    store::generation()
}

/// What one classified observation does to the stored verdict.
///
/// **Three outcomes, not two, and the third is the one a tidy version drops:**
/// an observation that *established nothing* must not overwrite one that did.
/// That is AP30's corollary (a) — *an errored round-trip is not an answer; keep
/// what you have* — arriving in the reachability store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Record {
    /// An earned claim about the path between us and them.
    Replace(Reachability),
    /// A working connection retires whatever sentence we were adding.
    Clear,
    /// This negotiation established nothing about that path, so it is not
    /// evidence in either direction.
    Keep,
}

/// How a classified verdict meets the memory of previous negotiations.
///
/// **Pure, and separate from `classify`, because the two reason over different
/// spans.** `classify` is a table over ONE observation and stays that way; this
/// is the only place cross-negotiation memory (`store::ANSWERED_EVER`) is
/// applied.
///
/// The arm that earns the function is `NoCounterpart` + `answered_before`.
/// Suppressing *"nobody was there"* about a peer we have heard from is right —
/// it is what the NAT rig bought — but the previous spelling suppressed it by
/// telling `classify` the exchange had completed, which **promoted the
/// observation into the network arms** and made a claim about a path nothing
/// touched. Measured 2026-09-15 in `make e2e-webrtc-stall`: while A's main
/// thread was frozen, B's retry negotiations classified `no-reflector` and B's
/// Chat header read *"No reflector is set up, so this app can only reach devices
/// on your local network"* — beside a conversation that was delivering in both
/// directions. There is a third answer and it is *we have nothing to say*.
///
/// ⭐ The passing runs of that gate differed only in `gathered = []`, which hits
/// `classify`'s empty-gather arm first. **The assertion's PASS/FAIL was a coin
/// toss on whether a doomed retry happened to gather a host candidate** — a
/// product that was right by fallback, not by rule.
pub fn record_for(verdict: Reachability, answered_before: bool) -> Record {
    match verdict {
        Reachability::Connected => Record::Clear,
        // Established nothing: an agent that never gathered, or a relay that
        // gathered and failed, are both statements about *us*.
        Reachability::Unknown => Record::Keep,
        // "Nobody answered" about a peer we have heard from is not a fact about
        // them — but it is not a fact about the network either.
        Reachability::NoCounterpart if answered_before => Record::Keep,
        v => Record::Replace(v),
    }
}

/// Record one finished negotiation.
///
/// `reflectors_configured` is the session's provisioning, not a property of the
/// candidates: it is the whole difference between *"we asked a reflector and got
/// nothing"* and *"we never asked"*, which are different sentences with different
/// fixes.
pub fn record_negotiation(
    peer_id: &str,
    local_candidates: &[String],
    reflectors_configured: bool,
    established: bool,
    sdp_exchange_complete: Option<bool>,
) {
    // **"Have they ever answered" is a fact about the peer, not about this
    // negotiation** — see `store::ANSWERED_EVER`. It is applied in `record_for`
    // rather than folded into the observation: this negotiation's exchange
    // either completed or it did not, and overwriting that answer is what let a
    // peer we had merely *heard from* be reported as a network topology.
    let ever_answered =
        store::note_exchange(peer_id, established || sdp_exchange_complete == Some(true));
    let obs = Observation {
        gathered: GatheredTypes::from_sdp_lines(local_candidates.iter().map(|s| s.as_str())),
        reflectors_configured,
        outcome: if established { Outcome::Connected } else { Outcome::Failed },
        sdp_exchange_complete,
    };
    let verdict = classify(&obs);
    let record = record_for(verdict, ever_answered);
    tracing::debug!(
        peer = %peer_id,
        gathered = ?obs.gathered.kinds(),
        reflectors_configured,
        established,
        sdp_exchange_complete = ?sdp_exchange_complete,
        ever_answered,
        verdict = verdict.as_token(),
        record = ?record,
        "reachability: negotiation classified"
    );
    match record {
        Record::Replace(v) => store::set(peer_id, v),
        Record::Clear => store::forget(peer_id),
        Record::Keep => {}
    }
}

/// Should a conversation show a diagnosis, and which?
///
/// Pure, and separate from the window that calls it, because the *relevance*
/// rule is the half most likely to be got wrong quietly: a correct verdict
/// rendered at the wrong moment is exactly the false unreachable note that four
/// assertions in every WebRTC gate exist to catch.
///
/// Two guards, and neither is redundant:
/// - **`bound` / `nothing_reachable`** — a diagnosis beside a working
///   conversation is a notice users learn to ignore, and an unbound window has
///   no remote to diagnose.
/// - **`is_advisory`** — carried by the verdict itself, so a peer still
///   negotiating (`Unknown`) cannot speak no matter what the window thinks.
///
/// Returns the **first** advisory verdict among the participants. A 1:1 chat has
/// one; for a group, one clear sentence beats a list nobody reads, and the
/// verdicts in a group are usually the same network fact seen twice.
pub fn advice_for_conversation(
    bound: bool,
    nothing_reachable: bool,
    participant_verdicts: impl IntoIterator<Item = Reachability>,
) -> Option<Reachability> {
    if !bound || !nothing_reachable {
        return None;
    }
    participant_verdicts.into_iter().find(|v| v.is_advisory())
}

/// The [`entity_wasm_worker_proxy::IceObserver`] that feeds [`record_negotiation`].
///
/// A ZST holding only the session's reflector count, so it is trivially
/// `Send + Sync` as the trait requires while the store it writes to is a
/// `thread_local!` — sound because the browser arm is single-threaded and the
/// establisher only ever calls this from the negotiation it runs on that thread.
#[cfg(target_arch = "wasm32")]
pub struct EstablisherObserver {
    reflectors_configured: bool,
}

#[cfg(target_arch = "wasm32")]
impl EstablisherObserver {
    /// `reflectors_configured` is fixed for the establisher's lifetime because
    /// `ice_servers` is: the session is handed the same list on every open, and
    /// changing reflectors means re-provisioning, which rebuilds the establisher.
    pub fn new(reflectors_configured: bool) -> Self {
        Self { reflectors_configured }
    }
}

#[cfg(target_arch = "wasm32")]
impl entity_wasm_worker_proxy::IceObserver for EstablisherObserver {
    fn negotiation_finished(&self, report: entity_wasm_worker_proxy::NegotiationReport<'_>) {
        record_negotiation(
            report.peer_id,
            report.local_candidates,
            self.reflectors_configured,
            report.established,
            report.sdp_exchange_complete,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real lines. The first is copied from `make e2e-webrtc-lan`'s own output
    /// (mDNS-obfuscated host, which is what a real browser sends and what every
    /// gate before that one was NOT exercising); the rest carry the optional
    /// components a positional parser would trip over.
    const LAN_HOST: &str =
        "candidate:0 1 UDP 2122252543 19ca5c40-0000-0000-0000-11061fb4056b.local 60044 typ host";
    const RAW_HOST: &str = "candidate:1 1 udp 2122260223 192.168.1.5 51234 typ host generation 0";
    const SRFLX: &str =
        "candidate:2 1 udp 1686052607 203.0.113.9 51234 typ srflx raddr 192.168.1.5 rport 51234";
    const RELAY: &str =
        "candidate:3 1 udp 41885439 198.51.100.7 60000 typ relay raddr 203.0.113.9 rport 51234";
    const PRFLX: &str = "candidate:4 1 udp 1853824767 203.0.113.9 51999 typ prflx";

    #[test]
    fn the_typ_token_is_read_by_name_not_by_position() {
        assert_eq!(CandidateKind::from_sdp_line(LAN_HOST), Some(CandidateKind::Host));
        assert_eq!(CandidateKind::from_sdp_line(RAW_HOST), Some(CandidateKind::Host));
        // `raddr`/`rport` sit BEFORE nothing and AFTER `typ`, and a tcptype or a
        // generation can appear too — the whole reason this reads the token
        // after `typ` rather than a fixed index.
        assert_eq!(
            CandidateKind::from_sdp_line(SRFLX),
            Some(CandidateKind::ServerReflexive)
        );
        assert_eq!(CandidateKind::from_sdp_line(RELAY), Some(CandidateKind::Relay));
        assert_eq!(CandidateKind::from_sdp_line(PRFLX), Some(CandidateKind::PeerReflexive));

        // An unknown type is NOT guessed at — every arm downstream is a claim
        // made to a user.
        assert_eq!(
            CandidateKind::from_sdp_line("candidate:9 1 udp 1 1.2.3.4 1 typ moonbeam"),
            None
        );
        assert_eq!(CandidateKind::from_sdp_line("a=end-of-candidates"), None);
        assert_eq!(CandidateKind::from_sdp_line(""), None);
        // `typ` as the last token: must not panic reaching for what follows.
        assert_eq!(CandidateKind::from_sdp_line("candidate:9 1 udp typ"), None);
    }

    #[test]
    fn gathering_folds_to_the_set_of_types_present() {
        let g = GatheredTypes::from_sdp_lines([RAW_HOST, SRFLX, RAW_HOST]);
        assert_eq!(
            g.kinds(),
            &[CandidateKind::Host, CandidateKind::ServerReflexive],
            "duplicates collapse and order is stable"
        );
        assert!(g.has_reflexive());
        assert!(!g.contains(CandidateKind::Relay));

        // prflx counts as reflexive — it is what makes symmetric↔cone work, and
        // it appears during checks rather than during gathering, so a classifier
        // that ignored it would under-report exactly the row that works.
        let p = GatheredTypes::from_sdp_lines([RAW_HOST, PRFLX]);
        assert!(p.has_reflexive(), "prflx must count as reflexive");

        assert!(GatheredTypes::from_sdp_lines(["a=end-of-candidates"]).is_empty());
    }

    /// The design's table, arm by arm.
    #[test]
    fn the_classification_table_holds() {
        let host_only = GatheredTypes::from_sdp_lines([RAW_HOST]);
        let with_srflx = GatheredTypes::from_sdp_lines([RAW_HOST, SRFLX]);
        let with_relay = GatheredTypes::from_sdp_lines([RAW_HOST, SRFLX, RELAY]);

        // Every row here is a genuine ICE failure: the far side answered, so a
        // claim about the network between the two is legitimate. That is what
        // `sdp_exchange_complete: Some(true)` is asserting, and it is a
        // PRECONDITION of the whole table rather than a detail of one row.
        //
        // host only, nobody configured a reflector → they can add one.
        assert_eq!(
            classify(&Observation {
                gathered: host_only.clone(),
                reflectors_configured: false,
                outcome: Outcome::Failed,
                sdp_exchange_complete: Some(true),
            }),
            Reachability::NoReflector
        );

        // host only, a reflector WAS configured → it did not answer. This is a
        // different sentence and a different fix from the arm above, and telling
        // them apart is half the point of the module.
        assert_eq!(
            classify(&Observation {
                gathered: host_only,
                reflectors_configured: true,
                outcome: Outcome::Failed,
                sdp_exchange_complete: Some(true),
            }),
            Reachability::ReflectorUnreachable
        );

        // We know our external address and still nominated nothing → a relay is
        // the answer. The arm that earns the feature.
        assert_eq!(
            classify(&Observation {
                gathered: with_srflx,
                reflectors_configured: true,
                outcome: Outcome::Failed,
                sdp_exchange_complete: Some(true),
            }),
            Reachability::NoDirectPath
        );

        // A relay gathered and it STILL failed: say nothing. "This network needs
        // a relay" would be false — they have one — and the likeliest cause is
        // that the far side is simply not there, which the existing state
        // already says.
        assert_eq!(
            classify(&Observation {
                gathered: with_relay,
                reflectors_configured: true,
                outcome: Outcome::Failed,
                sdp_exchange_complete: Some(true),
            }),
            Reachability::Unknown
        );
    }

    /// **The don't-cry-wolf guard, and it is the regression gate for the whole
    /// feature.** Every WebRTC gate asserts twice per run, on both sides, that a
    /// header raises no false unreachable note. Establishment takes seconds; a
    /// peer that is not connected *yet* must not be called unreachable.
    #[test]
    fn nothing_in_flight_or_unattempted_ever_classifies() {
        for outcome in [Outcome::NotAttempted, Outcome::InFlight] {
            for reflectors_configured in [false, true] {
                // Over every exchange state too: `NoCounterpart` is the newest
                // arm and it must be as unable to speak mid-establishment as
                // every other one.
                for sdp_exchange_complete in [None, Some(false), Some(true)] {
                for gathered in [
                    GatheredTypes::default(),
                    GatheredTypes::from_sdp_lines([RAW_HOST]),
                    GatheredTypes::from_sdp_lines([RAW_HOST, SRFLX]),
                ] {
                    let v = classify(&Observation {
                        gathered: gathered.clone(),
                        reflectors_configured,
                        outcome,
                        sdp_exchange_complete,
                    });
                    assert_eq!(
                        v,
                        Reachability::Unknown,
                        "{outcome:?} with {gathered:?} must stay silent"
                    );
                    assert!(!v.is_advisory(), "and must render nothing");
                }
                }
            }
        }
    }

    /// A successful connection never carries advice, whatever it gathered — the
    /// four "raises no false unreachable note" assertions in the green gates are
    /// exactly this case.
    #[test]
    fn a_connected_peer_never_carries_advice() {
        for gathered in [
            GatheredTypes::default(),
            GatheredTypes::from_sdp_lines([RAW_HOST]),
            GatheredTypes::from_sdp_lines([RAW_HOST, SRFLX, RELAY]),
        ] {
            let v = classify(&Observation {
                gathered,
                reflectors_configured: true,
                outcome: Outcome::Connected,
                sdp_exchange_complete: Some(true),
            });
            assert_eq!(v, Reachability::Connected);
            assert!(!v.is_advisory());
        }
    }

    /// **The operator's bug, as a table.** A chat opened against a peer that was
    /// not running was told *"no reflector is set up, so this app can only reach
    /// devices on your local network"* — on one machine, where a reflector fixes
    /// nothing and no reflector was the problem. Measured in a real browser
    /// against the desktop's own served app, 2026-08-22.
    ///
    /// The property is *an unanswered exchange makes no claim about the
    /// network*, so it is asserted over every network shape rather than the one
    /// that was reported: no gather is a topology claim if nobody was there to
    /// have a topology with.
    #[test]
    fn an_exchange_nobody_answered_is_not_a_claim_about_the_network() {
        for gathered in [
            GatheredTypes::from_sdp_lines([RAW_HOST]),
            GatheredTypes::from_sdp_lines([RAW_HOST, SRFLX]),
            GatheredTypes::from_sdp_lines([RAW_HOST, SRFLX, RELAY]),
        ] {
            for reflectors_configured in [false, true] {
                assert_eq!(
                    classify(&Observation {
                        gathered: gathered.clone(),
                        reflectors_configured,
                        outcome: Outcome::Failed,
                        sdp_exchange_complete: Some(false),
                    }),
                    Reachability::NoCounterpart,
                    "{gathered:?} with reflectors={reflectors_configured} — nobody answered, so \
                     none of the network arms may speak"
                );
            }
        }

        // ⭐ **REVERSED 2026-09-16, and the reversal is the subject of
        // `an_answerer_with_nothing_to_answer_is_told_nobody_was_there`.** This
        // assertion used to demand `Unknown` here, on the reasoning that "an
        // agent that gathered nothing is about US, so it outranks a statement
        // about them". `AUDIT-2026-09-15-a` is the case that analysis was
        // missing: an answerer with no offer to answer never gathers at all, so
        // the empty gather IS the statement about them. Ordering, asserted
        // rather than commented — in the other direction now.
        assert_eq!(
            classify(&Observation {
                gathered: GatheredTypes::default(),
                reflectors_configured: false,
                outcome: Outcome::Failed,
                sdp_exchange_complete: Some(false),
            }),
            Reachability::NoCounterpart
        );

        // **`None` is not `false`.** A carrier error or a policy refusal cannot
        // say whether the far side answered, and reading that silence as "they
        // did not" would blame a peer for our own refusal. It falls through to
        // exactly the pre-existing behaviour — which is also what keeps the NAT
        // gate's `NoReflector` expectation intact on any path that cannot
        // measure the exchange.
        assert_eq!(
            classify(&Observation {
                gathered: GatheredTypes::from_sdp_lines([RAW_HOST]),
                reflectors_configured: false,
                outcome: Outcome::Failed,
                sdp_exchange_complete: None,
            }),
            Reachability::NoReflector
        );
    }

    // -----------------------------------------------------------------------
    // The store and the relevance rule
    // -----------------------------------------------------------------------

    /// Recording a failed negotiation makes the verdict readable by peer-id —
    /// the whole point of the store, since the surface that renders it is a
    /// different window from the one that establishes.
    #[test]
    fn a_recorded_failure_is_readable_by_peer_id() {
        store::clear();
        assert_eq!(verdict_for("2KAlice"), Reachability::Unknown, "nothing known yet");

        record_negotiation("2KAlice", &[RAW_HOST.into(), SRFLX.into()], true, false, Some(true));
        assert_eq!(verdict_for("2KAlice"), Reachability::NoDirectPath);
        // Keyed, not global: one peer's verdict must not answer for another.
        assert_eq!(verdict_for("2KBob"), Reachability::Unknown);
    }

    /// **A peer that HAS answered must not later be called absent — the NAT rig
    /// found this, and it is why "have they answered" is sticky.**
    ///
    /// A present-but-unreachable peer produces a mixture: some negotiations
    /// complete the SDP exchange and fail at ICE, others never correlate (a full
    /// bucket, a retry storm, a window that closed first). Measured in
    /// `make e2e-webrtc-nat` — B's first negotiation `sdp_exchange=complete,
    /// fed=3`, a later one `INCOMPLETE, fed=0` — so classifying the most recent
    /// one alone told one browser its reflector was missing and the other that
    /// its perfectly live counterpart had never answered. Same topology, two
    /// different sentences, decided by which negotiation ran last.
    #[test]
    fn a_counterpart_that_answered_once_is_not_called_absent_by_a_later_miss() {
        store::clear();
        let host_only = [RAW_HOST.to_string()];

        // 1. They answered, and ICE failed: the network verdict is legitimate.
        record_negotiation("2KAlice", &host_only, false, false, Some(true));
        assert_eq!(verdict_for("2KAlice"), Reachability::NoReflector);

        // 2. A later negotiation never correlates. Same peer, same topology —
        //    it must not un-say what we already know about them.
        record_negotiation("2KAlice", &host_only, false, false, Some(false));
        assert_eq!(
            verdict_for("2KAlice"),
            Reachability::NoReflector,
            "a miss after a hit is a miss, not evidence that nobody is there"
        );

        // 3. And the stickiness is PER PEER, not global — otherwise one working
        //    counterpart would silence the arm for every other peer.
        record_negotiation("2KBob", &host_only, false, false, Some(false));
        assert_eq!(verdict_for("2KBob"), Reachability::NoCounterpart);
    }

    /// **The success half of the observer contract, and it is the cry-wolf
    /// failure arriving late.** A consumer told only about failures keeps
    /// showing "this network needs a relay" over a connection that is now
    /// working — which is worse than never having said it, because the user
    /// went and bought a relay.
    #[test]
    fn a_later_success_clears_the_advice_it_was_showing() {
        store::clear();
        record_negotiation("2KAlice", &[RAW_HOST.into(), SRFLX.into()], true, false, Some(true));
        assert!(verdict_for("2KAlice").is_advisory(), "precondition: advice is showing");

        record_negotiation("2KAlice", &[RAW_HOST.into(), SRFLX.into()], true, true, Some(true));
        assert!(
            !verdict_for("2KAlice").is_advisory(),
            "a working connection must carry no leftover diagnosis"
        );
        // And specifically `Unknown`, not `Connected`: a `Connected` row here
        // would make this store a second opinion about liveness, which is the
        // one thing the module header promises it is not. The kernel says
        // whether; this only ever says why-not.
        assert_eq!(
            verdict_for("2KAlice"),
            Reachability::Unknown,
            "this store holds advice, never connection state"
        );
    }

    /// The relevance rule: a correct verdict rendered at the wrong moment is
    /// exactly the false unreachable note the WebRTC gates assert against.
    #[test]
    fn advice_is_withheld_unless_it_is_both_true_and_relevant() {
        let advisory = [Reachability::NoDirectPath];

        assert_eq!(
            advice_for_conversation(true, true, advisory),
            Some(Reachability::NoDirectPath),
            "bound, nothing reachable, and a real verdict — say it"
        );
        assert_eq!(
            advice_for_conversation(false, true, advisory),
            None,
            "an unbound window has no remote to diagnose"
        );
        assert_eq!(
            advice_for_conversation(true, false, advisory),
            None,
            "something IS reachable — a diagnosis beside a working conversation \
             is the notice users learn to ignore"
        );
        // The guard that lives in the verdict rather than in the window: a peer
        // still negotiating cannot speak however the window is configured.
        assert_eq!(
            advice_for_conversation(true, true, [Reachability::Unknown]),
            None
        );
        assert_eq!(
            advice_for_conversation(true, true, [Reachability::Connected]),
            None
        );
        assert_eq!(advice_for_conversation(true, true, []), None);
        // First advisory wins, and a silent verdict never masks a real one.
        assert_eq!(
            advice_for_conversation(
                true,
                true,
                [Reachability::Unknown, Reachability::NoReflector]
            ),
            Some(Reachability::NoReflector)
        );
    }

    /// ⭐ **The stall bug, as a table.** Two browsers chat; one's main thread is
    /// frozen for 40 s; the side still running retries §6.5 at a peer it cannot
    /// momentarily hear, and every retry fails `sdp_exchange=INCOMPLETE,
    /// candidates posted=3/fed=0`. Measured in `make e2e-webrtc-stall`
    /// (2026-09-15, kernel `86e313b`): the header read *"No reflector is set
    /// up…"* **while messages were crossing in both directions.**
    ///
    /// The shape is a peer that is `Connected`, then misses. Asserted over every
    /// gathered set, because the run that passed differed from the run that
    /// failed only in `gathered = []` — the empty-gather arm catching it first.
    /// A property that holds only for one candidate set is the fallback, not the
    /// rule.
    #[test]
    fn a_miss_after_a_success_says_nothing_about_the_network() {
        for gathered in [
            vec![],
            vec![RAW_HOST.to_string()],
            vec![RAW_HOST.to_string(), SRFLX.to_string()],
        ] {
            for reflectors_configured in [false, true] {
                store::clear();
                // 1. They answered and we connected: no sentence is owed.
                record_negotiation("2KAlice", &gathered, reflectors_configured, true, Some(true));
                assert_eq!(verdict_for("2KAlice"), Reachability::Unknown);

                // 2. A retry while they are frozen. Nothing correlated, so this
                //    negotiation exercised no path between the two of us.
                record_negotiation("2KAlice", &gathered, reflectors_configured, false, Some(false));
                assert_eq!(
                    verdict_for("2KAlice"),
                    Reachability::Unknown,
                    "{gathered:?} reflectors={reflectors_configured}: a peer we are talking to \
                     must not acquire a network diagnosis from a retry that correlated nothing"
                );
                assert!(!verdict_for("2KAlice").is_advisory());
            }
        }
    }

    /// The three outcomes, and the count — so a seventh `Reachability` variant
    /// has to be given a rule instead of falling into `Replace` by default.
    ///
    /// **`Replace` may only ever carry an advisory verdict**: a non-advisory one
    /// reaching the store would put connection state in a map whose header
    /// promises it holds none.
    #[test]
    fn every_verdict_has_a_rule_for_what_it_does_to_the_store() {
        let all = [
            Reachability::Unknown,
            Reachability::Connected,
            Reachability::NoCounterpart,
            Reachability::NoReflector,
            Reachability::ReflectorUnreachable,
            Reachability::NoDirectPath,
        ];
        assert_eq!(all.len(), 6, "a new variant needs a row here, not a default");

        for v in all {
            for answered_before in [false, true] {
                if let Record::Replace(stored) = record_for(v, answered_before) {
                    assert!(
                        stored.is_advisory(),
                        "{v:?} would store a non-advisory verdict"
                    );
                }
            }
        }
        // The one arm that reads `answered_before` — and it is the fix.
        assert_eq!(record_for(Reachability::NoCounterpart, false),
                   Record::Replace(Reachability::NoCounterpart));
        assert_eq!(record_for(Reachability::NoCounterpart, true), Record::Keep);
        assert_eq!(record_for(Reachability::Connected, false), Record::Clear);
        assert_eq!(record_for(Reachability::Unknown, true), Record::Keep);
        // And a network claim is unaffected by having heard from them: that is
        // the NAT rig's verdict and it must still be written.
        assert_eq!(record_for(Reachability::NoReflector, true),
                   Record::Replace(Reachability::NoReflector));
    }

    /// An observation that established nothing must not erase one that did —
    /// including an ICE agent that never gathered, which is a statement about
    /// **us** and therefore no evidence against a finding about the path.
    #[test]
    fn an_agent_that_never_started_does_not_un_say_a_real_finding() {
        store::clear();
        let host_only = [RAW_HOST.to_string()];
        record_negotiation("2KAlice", &host_only, false, false, Some(true));
        assert_eq!(verdict_for("2KAlice"), Reachability::NoReflector, "precondition");

        record_negotiation("2KAlice", &[], false, false, Some(true));
        assert_eq!(
            verdict_for("2KAlice"),
            Reachability::NoReflector,
            "an agent that gathered nothing is not evidence about their network"
        );
    }

    /// An agent that gathered **nothing**, in a negotiation that cannot say
    /// whether the far side was there, failed for a reason that is not about the
    /// network between two peers — so we have nothing honest to say. Notably
    /// this is NOT `NoReflector`: recommending a reflector to someone whose ICE
    /// agent never started would send them to fix the wrong thing.
    ///
    /// `Some(true)` and `None` are both here **because they are the two inputs
    /// this arm still owns** after `NoCounterpart` was lifted above it
    /// (2026-09-16). Running only the `Some(true)` row would leave the arm's
    /// precedence over `reflectors_configured` — the thing it was earned for —
    /// resting on one input.
    #[test]
    fn gathering_nothing_at_all_is_not_a_topology_claim() {
        for reflectors_configured in [false, true] {
            for sdp_exchange_complete in [Some(true), None] {
                assert_eq!(
                    classify(&Observation {
                        gathered: GatheredTypes::default(),
                        reflectors_configured,
                        outcome: Outcome::Failed,
                        sdp_exchange_complete,
                    }),
                    Reachability::Unknown,
                    "reflectors={reflectors_configured} exchange={sdp_exchange_complete:?}"
                );
            }
        }
    }

    /// **THE STORM'S MISSING TERMINAL — `AUDIT-2026-09-15-a`, closed 2026-09-16.**
    ///
    /// A real session ran ~100 consecutive negotiations, every one
    /// `role=answerer, sdp_exchange=INCOMPLETE, bucket=0 msg(s), candidates
    /// posted=0/fed=0`, and the app said **nothing at all** — for the whole life
    /// of the session. Not a wrong sentence: no sentence. The audit recorded it
    /// as *"never reaching an honest terminal `I cannot reach them`"*.
    ///
    /// The cause was an ordering, and the ordering was decided by a case
    /// analysis missing its own main case. `classify` asked *"did our agent
    /// gather anything?"* first and answered `Unknown` when it had not, on the
    /// reasoning that an agent which never gathered is a fault of **ours** and
    /// must not be reported as a fault of theirs. True — and **an answerer with
    /// no offer to answer never calls `create_answer`, so gathering never
    /// starts**. In that shape the empty gather is the *consequence* of the
    /// counterpart's absence, and treating it as an independent fault about us
    /// swallowed the one claim that was both true and actionable.
    ///
    /// What makes the reordering safe rather than a coin flip is that
    /// `Some(false)` cannot be produced by our own machinery failing: only
    /// `WebRtcError::Timeout` carries `answered`, and a `Timeout` means the
    /// negotiation loop ran to its deadline. A `create_offer`/`create_answer`
    /// that threw reports `None`, which still falls through to `Unknown`.
    #[test]
    fn an_answerer_with_nothing_to_answer_is_told_nobody_was_there() {
        // The capture's exact shape: nothing gathered, nobody answered.
        let storm = Observation {
            gathered: GatheredTypes::default(),
            reflectors_configured: false,
            outcome: Outcome::Failed,
            sdp_exchange_complete: Some(false),
        };
        assert_eq!(
            classify(&storm),
            Reachability::NoCounterpart,
            "the ~100-negotiation storm must reach a terminal, not silence"
        );
        assert!(
            classify(&storm).is_advisory(),
            "a terminal that renders nothing is the silence this test exists to end"
        );
        // And it must not have become a claim about the network on the way —
        // the empty-gather arm's own reason for existing.
        for reflectors_configured in [false, true] {
            let v = classify(&Observation { reflectors_configured, ..storm.clone() });
            assert_ne!(v, Reachability::NoReflector, "reflectors={reflectors_configured}");
            assert_ne!(v, Reachability::ReflectorUnreachable, "reflectors={reflectors_configured}");
            assert_ne!(v, Reachability::NoDirectPath, "reflectors={reflectors_configured}");
        }
    }

    /// The suppression still holds over the new ordering: a peer we have
    /// **heard from** must not be told *"that device didn't answer"* just
    /// because one later negotiation timed out with nothing gathered. That is
    /// `record_for`'s `Keep`, and lifting `NoCounterpart` above the empty-gather
    /// arm makes strictly more observations reach it — so the guard now carries
    /// strictly more weight than it did when it was written.
    #[test]
    fn a_peer_we_have_heard_from_is_still_not_called_absent_by_an_empty_negotiation() {
        assert_eq!(record_for(Reachability::NoCounterpart, true), Record::Keep);
        assert_eq!(
            record_for(Reachability::NoCounterpart, false),
            Record::Replace(Reachability::NoCounterpart)
        );
    }
}
