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

pub mod delivery;
pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use crate::window_watch::WindowWatch;
use delivery::ChatDelivery;
use model::ChatModel;

pub struct ChatWindow {
    window_id: WindowId,
    peer_id: String,
    // Used only on the WASM render path; native sees it via handle_action/tests.
    model: ChatModel,
    watch: WindowWatch,
    /// Cross-peer delivery for a bound conversation. `None` while the window
    /// shows the default single-peer `self` conversation; `Some` once bound to a
    /// 1:1 with another peer (drives subscribe/fetch/cache; `tick` pumps it).
    delivery: Option<ChatDelivery>,
}

impl ChatWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        Self {
            window_id,
            peer_id: peer_id.clone(),
            model: ChatModel::new(peer_id),
            watch: WindowWatch::new(),
            delivery: None,
        }
    }

    /// Rebind the window to the well-known 1:1 conversation with `other`: swap
    /// the model to the two-participant union, subscribe the window's watch to
    /// both participants' prefixes (so render reads them and wakes on a delivered
    /// write), and install a fresh [`ChatDelivery`]. Returns the participant list
    /// so the caller can kick off delivery. Shared by the wasm action path and
    /// the awaitable test path.
    fn bind_one_to_one(&mut self, peers: &Peers, other: &str) -> Vec<String> {
        let conversation_id = model::wellknown_one_to_one_id(&self.peer_id, other);
        let participants = vec![self.peer_id.clone(), other.to_string()];
        self.model = ChatModel::with_conversation(
            self.peer_id.clone(),
            conversation_id.clone(),
            participants.clone(),
        );
        for prefix in self.model.subscription_prefixes() {
            peers.watch_prefix(&mut self.watch, &self.peer_id, prefix);
        }
        self.delivery = Some(ChatDelivery::new(
            self.peer_id.clone(),
            conversation_id,
            participants.clone(),
        ));
        self.watch.mark_dirty();
        participants
    }

    /// Bind + await delivery subscription — the awaitable form used by tests and
    /// any non-frame-loop caller. (The live window binds via `ChatStartWith`,
    /// which spawns the subscribe off the frame loop.)
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub async fn bind_and_subscribe(&mut self, peers: &Peers, other: &str) {
        self.bind_one_to_one(peers, other);
        if let Some(delivery) = &self.delivery {
            delivery.subscribe(peers).await;
        }
    }

    /// The current render projection (message list + `mine` flags + conversation
    /// id) — what the window paints. Pure data, so it is what the native
    /// full-flow test asserts on; `render_dom` projects the same thing to DOM.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub fn render_output(&self, peers: &Peers) -> output::ChatOutput {
        self.model.render_output(peers)
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

    /// A bound conversation's remote participants. Empty while the window shows
    /// the default self-conversation (no remote, nothing to maintain) — binding
    /// a 1:1 is the deliberate user act that makes the relationship worth
    /// keeping alive across a drop.
    fn maintained_remotes(&self) -> Vec<String> {
        match &self.delivery {
            Some(d) => d.remotes().to_vec(),
            None => Vec::new(),
        }
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, peers: &Peers) {
        match action {
            // The compose draft is tracked in the DOM atom (`ctx.drafts`), so the
            // send just authors into the tree; the messages subscription
            // re-renders when the write lands — no manual dirty flag.
            Action::ChatSend { window_id, body } if *window_id == self.window_id => {
                self.model.send(peers, body);
            }
            // Bind a 1:1 with the picked peer and start pulling their messages.
            Action::ChatStartWith { window_id, peer_id } if *window_id == self.window_id => {
                self.bind_one_to_one(peers, peer_id);
                // Start reactive delivery off the frame loop (Direct arm; can't
                // await here). `tick` pumps the pipeline each frame — and its
                // poll is what carries delivery on the Worker arm, where
                // subscribe is unavailable.
                #[cfg(target_arch = "wasm32")]
                if let Some(delivery) = &self.delivery {
                    delivery.start(peers);
                }
            }
            _ => {}
        }
    }

    fn tick(&mut self, peers: &Peers) {
        // Drain the delivery pipeline: spawn fetches for newly-notified messages,
        // cache completed ones into our store (which wakes render via the watch).
        if let Some(delivery) = &mut self.delivery {
            delivery.pump(peers);
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        peers: &Peers,
        ctx: &crate::dom::DomCtx,
    ) {
        let output = self.render_output(peers);
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

    /// The window only offers a remote to `maintain-peer` once the user has
    /// deliberately bound a conversation. An unbound window shows the default
    /// self-conversation — maintaining anything there would either be a
    /// self-maintain (which the handler rejects) or a connection kept alive
    /// that nobody asked for.
    #[tokio::test]
    async fn maintained_remotes_is_empty_until_a_conversation_is_bound() {
        let peers = Peers::new_direct();
        let pid = peers.primary_peer_id().to_string();
        let wt = ChatWindow::window_type();
        let view = (wt.create)(7, &pid, &peers);

        assert!(
            view.maintained_remotes().is_empty(),
            "an unbound chat window has no remote to keep connected"
        );

        // Bind a 1:1 — now the other participant is what must survive a drop.
        let other = "2KotherPeerIdForTheBoundConversation".to_string();
        let mut win = ChatWindow::new(7, pid.clone());
        win.bind_and_subscribe(&peers, &other).await;
        assert_eq!(
            win.maintained_remotes(),
            vec![other],
            "a bound 1:1 offers exactly the other participant — never ourselves"
        );
        assert!(
            !win.maintained_remotes().contains(&pid),
            "our own peer must never be offered: maintain-peer rejects self-maintain"
        );
    }
}
