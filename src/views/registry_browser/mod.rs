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
            // `peer_id \x1f origin`. Split rather than `split_whitespace` so an
            // EMPTY origin survives as an empty second field — that is
            // same-origin, a legitimate and common answer, not a missing one.
            "registry_pin" => {
                let (pid, origin) = value.split_once('\u{1f}').unwrap_or((value.as_str(), ""));
                // The error is carried on the output, so a refusal is on screen
                // rather than a button that appears to do nothing.
                let _ = self.model.pin(pid, origin);
                self.watch.mark_dirty();
            }
            "registry_unpin" => {
                self.model.unpin();
                self.watch.mark_dirty();
            }
            // ⭐ **Which viewer was pressed decides what the press DOES.**
            //
            // Both register the origin the SIGNED binding carried — that is what
            // the resolve established and every viewer needs it. Beyond that
            // they diverge: the Site Browser warms the publisher's manifests
            // (see `open_in_site_browser` for what that does and does not
            // establish — the pages stay unverified, and it says so), and the
            // Feed **adds them to the reader's feed, carrying the resolved
            // name**, which is the only moment that name exists.
            //
            // `value` is the viewer's identity key, not its caption. An
            // unrecognised one registers the route and stops: that half is right
            // for every viewer, and guessing at the other half is how a press
            // does something nobody asked for.
            "registry_open" => {
                let out = self.model.render_output(peers);
                let output::Phase::Done(target) = out.resolved else { return };
                let acted = match value.as_str() {
                    crate::open_target::FEED => self
                        .model
                        .add_to_feed(peers, &target, now_ms_u64())
                        .is_some(),
                    crate::open_target::SITE_BROWSER => {
                        self.model.open_in_site_browser(peers, &target).is_some()
                    }
                    other => {
                        tracing::warn!(
                            viewer = %other,
                            "registry open: an unrecognised viewer — registering the route only"
                        );
                        self.model.register_route_for(peers, &target).is_some()
                    }
                };
                if acted {
                    self.watch.mark_dirty();
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

/// Wall-clock ms for the `since` of a follow this window writes.
///
/// **`Date.now()`, not `performance.now()`** — the same split the Feed window's
/// own `now_ms_u64` carries, and for the same reason: `since` is a timestamp
/// somebody may one day see, and a monotonic clock measured from page load would
/// record every follow as having happened a few seconds after the epoch.
#[cfg(target_arch = "wasm32")]
fn now_ms_u64() -> u64 {
    js_sys::Date::now() as u64
}

/// Natively there is no clock; `0` is an honest *"we do not know when"* rather
/// than a fabricated instant.
#[cfg(not(target_arch = "wasm32"))]
fn now_ms_u64() -> u64 {
    0
}
