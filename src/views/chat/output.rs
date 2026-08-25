//! Chat window render output — the model→DOM handoff shape (pure data, no
//! `web_sys`, so `render_output` stays native-testable).

/// Everything the DOM layer needs to paint one frame of the Chat window.
pub struct ChatOutput {
    /// The active conversation id (the immutable genesis content-hash; the
    /// slice-2 default is the local `self` conversation).
    pub conversation_id: String,
    /// Messages oldest-first.
    pub messages: Vec<ChatMessageView>,
    /// Whether a real (multi-peer) conversation is bound. When false the window
    /// is on the default single-peer `self` scratch and offers the start-a-chat
    /// picker.
    pub bound: bool,
    /// Connected peers you can start a 1:1 chat with — populated only while
    /// unbound. Empty + unbound ⇒ show the "connect a peer first" hint.
    pub startable: Vec<StartablePeer>,
    /// Can we actually reach the people in this conversation? One row per
    /// **other** participant (never ourselves), in the model's participant
    /// order. **Empty while unbound** — the default self-conversation has no
    /// remote, so there is nothing for a connection status to be *about*, and a
    /// chip there would invent a relationship the user never created.
    ///
    /// The status is the kernel read-model resolved through
    /// [`conn_display`](crate::peer_liveness::conn_display) — the same
    /// vocabulary and the same authority Peer Connections renders (S4: one
    /// status, one home). Chat does **not** derive liveness of its own; a
    /// second opinion here would be the `connection_health` mirror disease in a
    /// new window.
    pub reachability: Vec<ParticipantReach>,
    /// The likely *reason* nothing here is reachable: this window's own peer
    /// installs no §6.5 WebRTC establisher, so a counterpart met through
    /// rendezvous can find us but has no way to reach back (AP22 — discovery
    /// and reachability are independent, and only one of them fails loudly).
    ///
    /// Set only when it is both **true and relevant**: the conversation is
    /// bound, we have no establisher, and no participant is currently
    /// reachable. An establisher is irrelevant to a peer we already reach over
    /// a WebSocket, and a standing warning next to a working conversation is
    /// the kind users learn to ignore.
    pub no_establisher: bool,
    /// *Why* nothing here is reachable, when our own ICE agent's gathered
    /// candidates can say (`crate::reachability`). `None` renders nothing.
    ///
    /// Subordinate to the state above, never a replacement for it: the chips
    /// keep saying **whether**, this adds **why**, and it is the difference
    /// between "that peer is not connected" and "this network needs a relay".
    ///
    /// Same relevance rule as [`Self::no_establisher`] — set only when the
    /// conversation is bound and nothing is currently reachable. It is also
    /// naturally exclusive with it: no establisher means no negotiation ever
    /// ran, so there is no verdict to carry. That is asserted rather than
    /// assumed (`no_establisher_and_a_network_verdict_are_never_both_shown`).
    pub reachability_advice: Option<crate::reachability::Reachability>,
}

/// One other participant's reachability, as the header paints it.
pub struct ParticipantReach {
    #[allow(dead_code)] // the renderer paints `label`; the id is for tests/logs
    pub peer_id: String,
    /// Display label (`views::display_name`) — never the raw id.
    pub label: String,
    /// Kernel-authoritative status, dial-hint layered in only where the kernel
    /// is silent. `Unknown` renders as a quiet dash, never a guess.
    pub status: crate::peer_liveness::ConnDisplay,
}

/// A connected peer offered in the start-a-chat picker.
pub struct StartablePeer {
    pub peer_id: String,
    pub name: String,
}

/// One message as the view needs it — the model's [`ChatMessage`] projected
/// with a `mine` flag so the renderer can style own-vs-other without re-deriving
/// authorship.
///
/// [`ChatMessage`]: super::model::ChatMessage
pub struct ChatMessageView {
    /// The authoritative author peer-id (the path authority, not the body field).
    pub author: String,
    /// Human-friendly byline: the author's label if known, else a short id
    /// (`views::display_name`). What the renderer paints — never the raw id.
    pub author_label: String,
    pub body: String,
    /// Author send time, projected for the renderer. Not shown yet — the DOM
    /// renderer paints author + body only; a timestamp / relative-time display
    /// is the same §4.3 display-ordering debt noted in the handoff (`sent_at` is
    /// a display heuristic, never a causal authority). Carried now so that pass
    /// needs no model change.
    #[allow(dead_code)]
    pub sent_at: u64,
    pub mine: bool,
}
