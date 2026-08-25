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
