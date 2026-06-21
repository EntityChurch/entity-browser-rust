//! Knowledge Base window — browse, read, and edit `knowledge/article`
//! entities in a peer's tree.
//!
//! Architecture:
//!
//! - The **model** (`model.rs`) is long-lived. It holds state +
//!   cache in `Arc<Mutex<Inner>>`, **subscribes** to its data sources
//!   via `PeerContext::subscribe`, and owns its own data lifecycle.
//!   All model methods take `&self` (interior mutability via the
//!   mutex).
//!
//! - This **window** is just a thin controller. It owns the model
//!   directly (no `RefCell`), marshals user `WindowEvent` actions
//!   into model method calls, and asks the model to persist state
//!   after actions that change it.
//!
//! - The **DOM renderer** (`crate::dom::knowledge_base`) reads the
//!   model's pure `render_output()` and constructs DOM. It never
//!   touches Peers.
//!
//! Draft buffers (in-progress edits) live in the DOM input/textarea
//! elements while editing — never in the tree, never in the model.
//! On save, the DOM click handler reads both values via `query_selector`,
//! packs them, and dispatches a single `save` event.

pub mod ingest;
pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use model::KnowledgeBaseModel;

use crate::window_watch::WindowWatch;

/// Separator between title and content in the packed `save` event value.
/// ASCII 0x1f (Unit Separator) — purpose-built for delimiting fields,
/// can't appear in user-typed text.
pub const SAVE_FIELD_SEP: char = '\x1f';

/// Knowledge Base window — peer-bound, thin controller around a
/// long-lived `KnowledgeBaseModel`.
///
/// The window holds the model **directly** (no `RefCell`) — the model
/// uses internal `Arc<Mutex<Inner>>` for its mutable state, so all of
/// its methods take `&self`.
pub struct KnowledgeBaseWindow {
    window_id: WindowId,
    peer_id: String,
    model: KnowledgeBaseModel,
    watch: WindowWatch,
}

impl KnowledgeBaseWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        let model = KnowledgeBaseModel::new(window_id, peer_id.clone());
        Self {
            window_id,
            peer_id,
            model,
            watch: WindowWatch::new(),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Knowledge Base",
            description: "Browse, read, and edit knowledge articles",
            scope: crate::window::WindowScope::Peer,
            create: |id, peer_id, pm| {
                let mut window = KnowledgeBaseWindow::new(id, peer_id.to_string());
                window.model.initialize(pm);
                // Stage C: per-event subscription on the
                // articles prefix. apply_change maintains a local
                // mirror so renders no longer re-scan via
                // tree_listing + per-slug get_entity.
                let inner = window.model.inner_arc();
                let peer_id_for_cb = window.peer_id.clone();
                pm.observe_with_events(
                    &mut window.watch,
                    &window.peer_id,
                    model::articles_prefix(&window.peer_id),
                    move |op| model::apply_change(&inner, &peer_id_for_cb, op),
                );
                // Notify-only watch on the window-state path so the
                // DOM rebuilds after the controller persists view-mode
                // / current_slug changes. No incremental state to
                // maintain here — render_output reads window state via
                // the inner Mutex directly.
                pm.watch_prefix(
                    &mut window.watch,
                    &window.peer_id,
                    crate::app_paths::window_state_path(crate::app_paths::APP_ID, &window.peer_id, window.window_id),
                );
                Box::new(window)
            },
        }
    }
}

impl WindowView for KnowledgeBaseWindow {
    fn title(&self) -> String {
        "Knowledge Base".into()
    }

    fn type_name(&self) -> &'static str {
        "Knowledge Base"
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    /// Controller: receive user input, dispatch to the model.
    /// The model performs any I/O. After a state-changing action,
    /// the controller asks the model to persist its state.
    fn handle_action(&mut self, action: &Action, peers: &Peers) {
        let (event, value) = match action {
            Action::WindowEvent {
                window_id,
                event,
                value,
            } if *window_id == self.window_id => (event.clone(), value.clone()),
            _ => return,
        };

        let state_changed = match event.as_str() {
            "show_list" => {
                self.model.show_list();
                true
            }
            "select" => {
                self.model.select_article(value);
                true
            }
            "toggle" => {
                // Flip a docs-tree directory's expand state. Persisting
                // the changed expanded_paths (save_state below) fires
                // the window-state watch → re-render with the new
                // flatten.
                self.model.toggle_expand(&value);
                true
            }
            "edit" => {
                self.model.enter_editor();
                true
            }
            "new" => {
                self.model.enter_new();
                true
            }
            "cancel" => {
                self.model.cancel();
                true
            }
            "save" => {
                let mut parts = value.splitn(2, SAVE_FIELD_SEP);
                let title = parts.next().unwrap_or("").to_string();
                let content = parts.next().unwrap_or("").to_string();
                match self.model.prepare_save(title, content) {
                    Ok((target_slug, article_path, article_entity)) => {
                        // Build the article put_and_wait future from
                        // the peers borrow before we drop it. The
                        // future itself is 'static and can be moved
                        // into spawn_local.
                        let article_future = peers.put_and_wait(
                            &self.peer_id,
                            article_path,
                            article_entity,
                            500,
                        );

                        // Transition view state in memory. Render sees
                        // ViewMode::Reader immediately; the cache
                        // doesn't have the article yet, but no render
                        // can fire until a subscription notifies — and
                        // both relevant subscriptions only fire after
                        // their respective Change events land (the
                        // proxy applies cache updates before firing
                        // notify, so by then the article is in cache).
                        self.model.commit_view_after_save(target_slug);

                        // Build the state put_and_wait future from the
                        // newly-mutated in-memory state, so the state
                        // entity reflects the post-save view mode.
                        let state_path = crate::app_paths::window_state_path(
                            crate::app_paths::APP_ID,
                            &self.peer_id,
                            self.window_id,
                        );
                        let state_entity = self.model.window_state_entity();
                        let state_future = peers.put_and_wait(
                            &self.peer_id,
                            state_path,
                            state_entity,
                            500,
                        );

                        // Sequence the writes: article first, then
                        // state. This ensures that whichever
                        // subscription's notify reaches the dirty flag
                        // first, the article is already cache-resident
                        // (the article put_and_wait completed before
                        // the state put was issued).
                        #[cfg(target_arch = "wasm32")]
                        wasm_bindgen_futures::spawn_local(async move {
                            if let Err(e) = article_future.await {
                                tracing::warn!(error = %e, "knowledge_base: article put_and_wait failed");
                            }
                            if let Err(e) = state_future.await {
                                tracing::warn!(error = %e, "knowledge_base: state put_and_wait failed");
                            }
                        });
                        // Native: tokio::spawn — relies on a current
                        // runtime being active (tests run under
                        // `#[tokio::test]`). On native the only build
                        // is the deprecation stub (EntityApp is
                        // wasm-only), so the sole consumer of this
                        // native arm is the unit-test suite.
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            tokio::spawn(async move {
                                if let Err(e) = article_future.await {
                                    tracing::warn!(error = %e, "knowledge_base: article put_and_wait failed");
                                }
                                if let Err(e) = state_future.await {
                                    tracing::warn!(error = %e, "knowledge_base: state put_and_wait failed");
                                }
                            });
                        }

                        // We handled the state write inside the spawn,
                        // so skip the normal save_state path below.
                        false
                    }
                    Err(reason) => {
                        tracing::warn!(reason = %reason, "knowledge_base save rejected");
                        false
                    }
                }
            }
            "delete" => {
                self.model.delete_current(peers);
                true
            }
            _ => false,
        };

        if state_changed {
            self.model.save_state(peers);
        }
    }

    /// View: read from the model and hand the output to the renderer.
    /// The model reads article list / current article fresh from the
    /// tree via `peers`. The renderer itself takes only the output.
    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        peers: &Peers,
        ctx: &crate::dom::DomCtx,
    ) {
        let output = self.model.render_output(peers);
        crate::dom::knowledge_base::render(container, &output, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::{article_path, articles_prefix, decode_article, encode_article};
    use output::ViewMode;

    fn pm() -> Peers {
        Peers::new_direct()
    }

    fn dispatch(window: &mut KnowledgeBaseWindow, peers: &Peers, event: &str, value: &str) {
        window.handle_action(
            &Action::WindowEvent {
                window_id: window.window_id,
                event: event.into(),
                value: value.into(),
            },
            peers,
        );
    }

    /// Flush pending spawned tasks. `dispatch_write` fires-and-forgets
    /// through the peer's execute pipeline; a brief sleep lets the full
    /// chain complete before the assertion reads the tree.
    async fn flush_writes() {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    #[tokio::test]
    async fn factory_initializes_model() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let mut window = KnowledgeBaseWindow::new(7, pid.clone());
        window.model.initialize(&pm);
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let path = crate::app_paths::window_state_path(crate::app_paths::APP_ID, &pid, 7);
        assert!(pm.get_entity(&pid, &path).is_some());
        assert_eq!(window.model.state_snapshot().view_mode, ViewMode::List);
    }

    #[tokio::test]
    async fn save_action_creates_article() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let mut window = KnowledgeBaseWindow::new(10, pid.clone());
        window.model.initialize(&pm);

        dispatch(&mut window, &pm, "new", "");
        let packed = format!("First Article{}# Hello\n\nThe body.", SAVE_FIELD_SEP);
        dispatch(&mut window, &pm, "save", &packed);
        flush_writes().await;

        let path = article_path(&pid, "first-article");
        let entity = pm.get_entity(&pid, &path).expect("article exists");
        let (title, content) = decode_article(&entity);
        assert_eq!(title, "First Article");
        assert_eq!(content, "# Hello\n\nThe body.");

        assert_eq!(window.model.state_snapshot().view_mode, ViewMode::Reader);
    }

    #[test]
    fn save_with_empty_title_does_not_write() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let mut window = KnowledgeBaseWindow::new(11, pid.clone());
        window.model.initialize(&pm);

        dispatch(&mut window, &pm, "new", "");
        let packed = format!("   {}body", SAVE_FIELD_SEP);
        dispatch(&mut window, &pm, "save", &packed);

        let prefix = articles_prefix(&pid);
        assert!(pm.tree_listing(&pid, &prefix).is_empty());
        assert_eq!(window.model.state_snapshot().view_mode, ViewMode::New);
    }

    #[tokio::test]
    async fn edit_then_save_updates_in_place() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let mut window = KnowledgeBaseWindow::new(12, pid.clone());
        window.model.initialize(&pm);

        dispatch(&mut window, &pm, "new", "");
        dispatch(&mut window, &pm, "save", &format!("Test{}original", SAVE_FIELD_SEP));
        flush_writes().await;
        dispatch(&mut window, &pm, "edit", "");
        dispatch(&mut window, &pm, "save", &format!("Test{}updated", SAVE_FIELD_SEP));
        flush_writes().await;

        let path = article_path(&pid, "test");
        let (_, content) = decode_article(&pm.get_entity(&pid, &path).unwrap());
        assert_eq!(content, "updated");

        let prefix = articles_prefix(&pid);
        assert_eq!(pm.tree_listing(&pid, &prefix).len(), 1);
    }

    #[tokio::test]
    async fn delete_removes_article() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let mut window = KnowledgeBaseWindow::new(13, pid.clone());
        window.model.initialize(&pm);

        dispatch(&mut window, &pm, "new", "");
        dispatch(&mut window, &pm, "save", &format!("Doomed{}body", SAVE_FIELD_SEP));
        flush_writes().await;
        dispatch(&mut window, &pm, "delete", "");

        let prefix = articles_prefix(&pid);
        assert!(pm.tree_listing(&pid, &prefix).is_empty());
        assert_eq!(window.model.state_snapshot().view_mode, ViewMode::List);
    }

    #[test]
    fn select_loads_current_article() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let ctx = pm.test_seed_ctx(&pid);
        ctx.store().put(&article_path(&pid, "preexisting"), encode_article("Pre", "existing"))
            .ok();

        let mut window = KnowledgeBaseWindow::new(15, pid.clone());
        window.model.initialize(&pm);

        dispatch(&mut window, &pm, "select", "preexisting");

        assert_eq!(window.model.state_snapshot().view_mode, ViewMode::Reader);
        let detail = window.model.current_article_snapshot(&pm).unwrap();
        assert_eq!(detail.title, "Pre");
    }

    #[test]
    fn cancel_new_does_not_create_article() {
        let pm = pm();
        let pid = pm.primary_peer_id().to_string();
        let mut window = KnowledgeBaseWindow::new(14, pid.clone());
        window.model.initialize(&pm);

        dispatch(&mut window, &pm, "new", "");
        dispatch(&mut window, &pm, "cancel", "");

        let prefix = articles_prefix(&pid);
        assert!(pm.tree_listing(&pid, &prefix).is_empty());
        assert_eq!(window.model.state_snapshot().view_mode, ViewMode::List);
    }
}
