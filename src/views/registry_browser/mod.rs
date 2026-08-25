//! Registry Browser window — which registries am I pointed at, what do they
//! carry, and what was checked.
//!
//! **System-scoped**, like the connector registry it sits beside: a registry pin
//! is deployment infrastructure, read from the durable `SessionConfig`, not a
//! property of whichever peer a window happens to be bound to. The shell's
//! `connector` verb once managed a second registry by using the *bound* peer;
//! that split is the thing this scope choice avoids repeating.
//!
//! It is a **general-purpose window**: it works wherever it is opened, and shows
//! less where less is configured (no pin → it says so and fails closed, rather
//! than rendering a blank panel that reads as an empty registry).

pub mod model;
pub mod output;

#[allow(unused_imports)]
use crate::action::Action;
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};

use crate::window_watch::WindowWatch;
use model::RegistryBrowserModel;

pub struct RegistryBrowserWindow {
    window_id: WindowId,
    peer_id: String,
    #[allow(dead_code)]
    model: RegistryBrowserModel,
    watch: WindowWatch,
}

impl RegistryBrowserWindow {
    pub fn new(window_id: WindowId, peer_id: String) -> Self {
        Self {
            window_id,
            peer_id,
            model: RegistryBrowserModel::new(window_id),
            watch: WindowWatch::new(),
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: "Registry Browser", // i18n-ignore — identity key; display via window.registry_browser
            description: "Pinned name registries, what they carry, and what a resolve checked", // i18n-ignore — dead_code
            scope: crate::window::WindowScope::System,
            create: |id, peer_id, _pm| {
                Box::new(RegistryBrowserWindow::new(id, peer_id.to_string()))
            },
        }
    }
}

impl WindowView for RegistryBrowserWindow {
    fn title(&self) -> String {
        crate::i18n::window_title("Registry Browser") // i18n-ignore — lookup key
    }

    fn type_name(&self) -> &'static str {
        "Registry Browser" // i18n-ignore — stable type identifier, not UI text
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    /// **The change flag is consumed here, not diffed.**
    ///
    /// Every result this window shows lands in a spawned task, between frames,
    /// touching no tree path — so no subscription fires and there is nothing for
    /// `WindowWatch` to notice. A surface that instead compared its own output
    /// around the pump would find it equal *exactly when* something had just
    /// changed, which is AP21 and is what froze a running `meet` on "Searching…".
    fn tick(&mut self, _peers: &Peers) {
        if self.model.take_changed() {
            self.watch.mark_dirty();
        }
    }

    fn handle_action(&mut self, action: &Action, peers: &Peers) {
        let Action::WindowEvent { window_id, event, value } = action else { return };
        if *window_id != self.window_id {
            return;
        }
        match event.as_str() {
            "registry_browse" => self.model.browse(),
            "registry_resolve" => self.model.resolve(value),
            // Opening registers the origin the SIGNED binding carried, then hands
            // the peer to a Site Browser. See `open_in_site_browser` for what that
            // does and does not establish — the pages themselves stay unverified,
            // and the Site Browser says so.
            "registry_open" => {
                let out = self.model.render_output(peers);
                if let output::Phase::Done(target) = out.resolved {
                    if self.model.open_in_site_browser(peers, &target).is_some() {
                        self.watch.mark_dirty();
                    }
                }
            }
            _ => {}
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(
        &self,
        container: &web_sys::Element,
        peers: &Peers,
        ctx: &crate::dom::util::DomCtx,
    ) {
        let output = self.model.render_output(peers);
        crate::dom::registry_browser::render(container, &output, ctx, self.window_id);
    }
}
