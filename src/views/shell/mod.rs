//! Entity shell window — 10th window, leading-edge feature surface.

pub mod binding;
pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use model::ShellModel;

use crate::window_watch::WindowWatch;

pub struct ShellWindow {
    window_id: WindowId,
    peer_id: String,
    model: ShellModel,
    watch: WindowWatch,
}

impl ShellWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        let model = ShellModel::new(window_id, peer_id.clone());
        Self {
            window_id,
            peer_id,
            model,
            watch: WindowWatch::new(),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Shell",
            description: "Entity shell — verbs, paths, exec",
            scope: crate::window::WindowScope::Peer,
            create: |id, peer_id, pm| {
                let mut window = ShellWindow::new(id, peer_id.to_string());
                window.model.initialize(pm);
                // Subscribe to the window's own state path so external
                // writes (e.g. async `exec` completing) re-render.
                let state = crate::app_paths::window_state_path(
                    crate::app_paths::APP_ID,
                    &window.peer_id,
                    window.window_id,
                );
                pm.watch_prefix(&mut window.watch, &window.peer_id, state);
                Box::new(window)
            },
        }
    }
}

impl ShellWindow {
    /// Install a `tail <prefix>` subscription on this window's
    /// `WindowWatch`. The subscription's lifetime is tied to the
    /// watch — closing the window drops it. Each `ChangeOp` event
    /// appends a Listing (Put) or Error (Remove) row to the model's
    /// scrollback and marks the watch dirty so the next render
    /// rebuilds.
    fn install_tail(&mut self, prefix: &str, peers: &Peers) {
        // Tail subscribes against the **bound peer**'s tree — the
        // prefix's first segment is part of the address, not a peer
        // selector. (To tail another peer's tree, open a shell on
        // that peer.)
        let pid = self.peer_id.clone();

        // Soft-cancellation: register an active flag and capture a
        // clone into the callback. `untail` flips the flag → callback
        // no-ops on subsequent events. The underlying subscription
        // handle stays parked on the watch until the window closes —
        // wasted SDK-side dispatch but no plumbing through Peers to
        // get the handle back.
        let active = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        self.model.tails.lock().unwrap().push(
            crate::views::shell::model::TailEntry {
                prefix: prefix.to_string(),
                active: active.clone(),
            },
        );

        let scrollback = self.model.inner.clone();
        peers.observe_with_events(
            &mut self.watch,
            &pid,
            prefix.to_string(),
            move |op| {
                if !active.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                use crate::views::shell::output::ScrollbackEntry;
                let entry = match op {
                    crate::peers::ChangeOp::Put { path } => {
                        ScrollbackEntry::Listing(format!("+ {}", path))
                    }
                    crate::peers::ChangeOp::Remove { path } => {
                        ScrollbackEntry::ErrorText(format!("- {}", path))
                    }
                    crate::peers::ChangeOp::Resync => {
                        ScrollbackEntry::Info("(tail resync — events dropped)".into())
                    }
                };
                scrollback.lock().unwrap().push(entry);
            },
        );
    }
}

impl WindowView for ShellWindow {
    fn title(&self) -> String {
        "Shell".into()
    }

    fn type_name(&self) -> &'static str {
        "Shell"
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, peers: &Peers) {
        let dirty = match action {
            Action::ShellSubmit { window_id, line } if *window_id == self.window_id => {
                self.model
                    .handle_submit(line, peers, self.window_id, self.watch.flag());
                true
            }
            Action::ShellHistoryPrev { window_id, current } if *window_id == self.window_id => {
                self.model.history_prev(current)
            }
            Action::ShellHistoryNext { window_id, current } if *window_id == self.window_id => {
                self.model.history_next(current)
            }
            Action::ShellClear(id) if *id == self.window_id => {
                self.model.clear();
                true
            }
            Action::ShellTabComplete { window_id, partial } if *window_id == self.window_id => {
                self.model.complete(partial, peers)
            }
            Action::ShellTail { window_id, prefix } if *window_id == self.window_id => {
                self.install_tail(prefix, peers);
                // Subscription handle parked on the watch; no model
                // state changed yet (events arrive async). The seed
                // emits initial entries which DO flip the dirty flag
                // via the callback, so the rebuild lands naturally.
                false
            }
            Action::WindowEvent { window_id, event, value } if *window_id == self.window_id => {
                match event.as_str() {
                    // Per-keystroke draft tracking so the model has
                    // the user's in-progress text on hand when an
                    // unrelated rebuild fires (exec completion,
                    // scrollback append, etc.). Intentionally returns
                    // `false` so each character doesn't write the
                    // shell state entity — that would thrash the tree
                    // and re-dirty the watch on every keystroke,
                    // causing a rebuild storm. Refresh loses the
                    // typed-but-unsubmitted text; the shell convention.
                    "set_draft" => {
                        self.model.set_draft(value);
                        false
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        if dirty {
            self.model.save_state(peers);
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        _peers: &Peers,
        ctx: &crate::dom::DomCtx,
    ) {
        // Forward any follow-up actions a verb produced
        // (`peer create`, `peer delete`, …) into the renderer's
        // shared queue. Two-frame propagation; cheap.
        let pending = self.model.drain_pending_actions();
        if !pending.is_empty() {
            ctx.actions.borrow_mut().extend(pending);
        }
        let output = self.model.render_output();
        crate::dom::shell::render(container, &output, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_peers() -> Peers {
        Peers::new_direct()
    }

    async fn flush_writes() {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    #[tokio::test]
    async fn factory_writes_initial_state() {
        let peers = make_peers();
        let wt = ShellWindow::window_type();
        let _view = (wt.create)(3, peers.primary_peer_id(), &peers);
        flush_writes().await;

        let path = crate::app_paths::window_state_path(
            crate::app_paths::APP_ID,
            peers.primary_peer_id(),
            3,
        );
        let entity = peers
            .get_entity(peers.primary_peer_id(), &path)
            .expect("initial shell state should land in the tree");
        assert_eq!(entity.entity_type, "app/state/shell");
    }

    #[tokio::test]
    async fn shell_submit_records_history_and_persists() {
        let peers = make_peers();
        let pid = peers.primary_peer_id().to_string();
        let wt = ShellWindow::window_type();
        let mut view = (wt.create)(5, &pid, &peers);
        flush_writes().await;

        view.handle_action(
            &Action::ShellSubmit {
                window_id: 5,
                line: "pwd".into(),
            },
            &peers,
        );
        flush_writes().await;

        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, &pid, 5);
        let entity = peers.get_entity(&pid, &path).unwrap();
        let state = model::ShellState::from_entity(&entity);
        assert_eq!(state.history, vec!["pwd"]);
    }

    #[tokio::test]
    async fn shell_clear_wipes_persisted_scrollback() {
        let peers = make_peers();
        let pid = peers.primary_peer_id().to_string();
        let wt = ShellWindow::window_type();
        let mut view = (wt.create)(9, &pid, &peers);
        flush_writes().await;

        view.handle_action(
            &Action::ShellSubmit {
                window_id: 9,
                line: "foo".into(),
            },
            &peers,
        );
        flush_writes().await;
        view.handle_action(&Action::ShellClear(9), &peers);
        flush_writes().await;

        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, &pid, 9);
        let entity = peers.get_entity(&pid, &path).unwrap();
        let state = model::ShellState::from_entity(&entity);
        assert!(state.scrollback.is_empty());
    }
}
