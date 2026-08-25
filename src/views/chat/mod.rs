//! Chat — the `app/chat` L5 format convention (arch DRAFT
//! `PROPOSAL-APP-CONVENTION-CHAT.md`), the connectivity proof-point app.
//!
//! Shipped so far: the **data layer** (`model`) — message + conversation-genesis
//! entity shapes, the `app/chat/` path convention, and the §1.4 **union read**
//! (`load_messages` merges every participant's messages, `subscription_prefixes`
//! drives one watch per participant); and the **window** — compose, send, live
//! list. Still ahead (the delivery slice): populating other participants' cached
//! copies — subscription over INBOX/RELAY, then the live P2P upgrade over the
//! §6.5 WebRTC floor. Until then the shipped surface binds only the single-peer
//! default self-conversation.

pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use crate::window_watch::WindowWatch;
use model::ChatModel;

pub struct ChatWindow {
    window_id: WindowId,
    peer_id: String,
    // Used only on the WASM render path; native sees it via handle_action/tests.
    model: ChatModel,
    watch: WindowWatch,
}

impl ChatWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        Self {
            window_id,
            peer_id: peer_id.clone(),
            model: ChatModel::new(peer_id),
            watch: WindowWatch::new(),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Chat", // i18n-ignore — identity key; display via window.chat
            description: "Conversational messaging over the app/chat convention", // i18n-ignore — dead_code
            scope: crate::window::WindowScope::Peer,
            create: |id, peer_id, pm| {
                let mut window = ChatWindow::new(id, peer_id.to_string());
                // Subscribe EACH participant's message prefix (§1.4 union), all
                // under the bound peer's own store — ours plus the cached copies
                // delivery lands under each other participant's prefix. Must
                // match exactly what `load_messages` reads, or the Worker arm's
                // subscribed-prefix-only cache drops a participant's messages.
                for prefix in window.model.subscription_prefixes() {
                    pm.watch_prefix(&mut window.watch, &window.peer_id, prefix);
                }
                Box::new(window)
            },
        }
    }
}

impl WindowView for ChatWindow {
    fn title(&self) -> String {
        crate::i18n::window_title("Chat") // i18n-ignore — lookup key, falls back to "Chat"
    }

    fn type_name(&self) -> &'static str {
        "Chat" // i18n-ignore — stable type identifier, not UI text
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, peers: &Peers) {
        // The compose draft is tracked in the DOM atom (`ctx.drafts`), so the
        // only action the window handles is the send itself. The messages
        // subscription re-renders when the write lands — no manual dirty flag.
        if let Action::ChatSend { window_id, body } = action {
            if *window_id == self.window_id {
                self.model.send(peers, body);
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        peers: &Peers,
        ctx: &crate::dom::DomCtx,
    ) {
        let output = self.model.render_output(peers);
        crate::dom::chat::render(container, &output, ctx);
    }
}

#[cfg(test)]
mod window_tests {
    use super::*;

    #[tokio::test]
    async fn chat_send_action_writes_a_message() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let wt = ChatWindow::window_type();
        let mut view = (wt.create)(7, &pid, &peers);

        view.handle_action(
            &Action::ChatSend {
                window_id: 7,
                body: "hello from the window".into(),
            },
            &peers,
        );
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        let prefix = model::conversation_messages_prefix(&pid, model::DEFAULT_CONVERSATION);
        let listed = peers.tree_listing(&pid, &prefix);
        assert_eq!(listed.len(), 1, "the window's ChatSend wrote one message");
    }

    #[tokio::test]
    async fn chat_send_ignores_other_windows() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let wt = ChatWindow::window_type();
        let mut view = (wt.create)(7, &pid, &peers);

        // A ChatSend addressed to a different window_id must be ignored.
        view.handle_action(
            &Action::ChatSend {
                window_id: 999,
                body: "not for us".into(),
            },
            &peers,
        );
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        let prefix = model::conversation_messages_prefix(&pid, model::DEFAULT_CONVERSATION);
        assert!(peers.tree_listing(&pid, &prefix).is_empty());
    }
}
