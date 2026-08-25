//! Chat window render output — the model→DOM handoff shape (pure data, no
//! `web_sys`, so `render_output` stays native-testable).

/// Everything the DOM layer needs to paint one frame of the Chat window.
pub struct ChatOutput {
    /// The active conversation id (the immutable genesis content-hash; the
    /// slice-2 default is the local `self` conversation).
    pub conversation_id: String,
    /// Messages oldest-first.
    pub messages: Vec<ChatMessageView>,
}

/// One message as the view needs it — the model's [`ChatMessage`] projected
/// with a `mine` flag so the renderer can style own-vs-other without re-deriving
/// authorship.
///
/// [`ChatMessage`]: super::model::ChatMessage
pub struct ChatMessageView {
    pub author: String,
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
