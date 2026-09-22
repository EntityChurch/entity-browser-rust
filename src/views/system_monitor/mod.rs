//! System Monitor window — what this tab can know about its own load, shown
//! live and labelled with where each figure came from (`DESIGN-2026-09-14-c`).
//!
//! Opening one takes a [`MonitorHold`], which wakes the sampler's hooks; closing
//! the last one drops it and the hooks go back to a single `Cell` read. The
//! window repaints once per second, when the sampler rolls — never per frame —
//! so its own cost is one small rebuild a second, and it lists itself among the
//! windows so that cost is visible.
//!
//! **No persisted window state**: nothing here is a fact about the profile.

pub mod output;

#[cfg(target_arch = "wasm32")]
use std::cell::{Cell, RefCell};
#[cfg(target_arch = "wasm32")]
use std::rc::Rc;

#[allow(unused_imports)]
use crate::action::Action;
use crate::monitor::sampler::{self, MonitorHold};
#[allow(unused_imports)]
use crate::peers::Peers;
#[allow(unused_imports)]
use crate::window::{WindowId, WindowType, WindowView};
use crate::window_watch::WindowWatch;

pub const TYPE_NAME: &str = "System Monitor"; // i18n-ignore — identity key; display via window.system_monitor

/// Show or hide *What your browser tells this tab*.
pub const ABOUT_EVENT: &str = "monitor_about";

/// How often the browser probes (memory, storage) are re-read. Storage
/// estimates are async and slow to change; memory figures are rounded anyway.
#[cfg(target_arch = "wasm32")]
const PROBE_EVERY_MS: f64 = 5_000.0;

pub struct SystemMonitorWindow {
    peer_id: String,
    watch: WindowWatch,
    _hold: MonitorHold,
    /// A button toggle rather than a `<details>`: the pane rebuilds every
    /// second, and a `<details>` would snap shut on each rebuild.
    about_open: std::cell::Cell<bool>,
    #[cfg(target_arch = "wasm32")]
    probes: Rc<RefCell<output::Probes>>,
    #[cfg(target_arch = "wasm32")]
    last_probe_ms: Cell<f64>,
    #[cfg(target_arch = "wasm32")]
    seen_rolls: Cell<u64>,
    #[cfg(target_arch = "wasm32")]
    pressure: Rc<RefCell<Option<String>>>,
    #[cfg(target_arch = "wasm32")]
    _pressure_watch: Option<crate::monitor::probe::PressureWatch>,
}

impl SystemMonitorWindow {
    pub fn new(peer_id: String) -> Self {
        let watch = WindowWatch::new();
        #[cfg(target_arch = "wasm32")]
        let pressure = Rc::new(RefCell::new(None));
        #[cfg(target_arch = "wasm32")]
        let pressure_watch = crate::monitor::probe::watch_pressure(pressure.clone(), watch.flag());
        Self {
            peer_id,
            _hold: sampler::hold(),
            about_open: std::cell::Cell::new(false),
            #[cfg(target_arch = "wasm32")]
            probes: Rc::new(RefCell::new(output::Probes {
                facts: Some(crate::monitor::probe::engine_facts()),
                ..Default::default()
            })),
            #[cfg(target_arch = "wasm32")]
            last_probe_ms: Cell::new(f64::NEG_INFINITY),
            #[cfg(target_arch = "wasm32")]
            seen_rolls: Cell::new(0),
            #[cfg(target_arch = "wasm32")]
            pressure,
            #[cfg(target_arch = "wasm32")]
            _pressure_watch: pressure_watch,
            watch,
        }
    }

    pub fn window_type() -> WindowType {
        WindowType {
            name: TYPE_NAME,
            description: "Live CPU, memory and storage figures for this tab, and what the browser will not tell it", // i18n-ignore — dead_code
            scope: crate::window::WindowScope::System,
            create: |_id, peer_id, _pm| Box::new(SystemMonitorWindow::new(peer_id.to_string())),
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn refresh_probes(&self) {
        let now = js_sys::Date::now();
        if now - self.last_probe_ms.get() < PROBE_EVERY_MS {
            return;
        }
        self.last_probe_ms.set(now);
        {
            let mut p = self.probes.borrow_mut();
            p.wasm_memory = crate::monitor::probe::wasm_memory_bytes();
            p.js_heap = crate::monitor::probe::js_heap();
        }
        let probes = self.probes.clone();
        let dirty = self.watch.flag();
        wasm_bindgen_futures::spawn_local(async move {
            let answer = crate::views::storage::fetch_origin_estimate().await.map(|est| output::StorageFigures {
                usage: est.usage_bytes,
                quota: est.quota_bytes,
                persisted: est.persisted,
            });
            probes.borrow_mut().storage = Some(answer);
            dirty.mark();
        });
    }
}

impl WindowView for SystemMonitorWindow {
    fn title(&self) -> String {
        crate::i18n::window_title(TYPE_NAME)
    }

    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn peer_id(&self) -> &str {
        &self.peer_id
    }

    fn watch(&self) -> &WindowWatch {
        &self.watch
    }

    fn handle_action(&mut self, action: &Action, _peers: &Peers) {
        if let Action::WindowEvent { event, .. } = action {
            if event == ABOUT_EVENT {
                self.about_open.set(!self.about_open.get());
            }
        }
        self.watch.mark_dirty();
    }

    /// Every frame: roll the sampler when a second has passed, and repaint only
    /// when the history moved. The sampler is shared, so with two monitors open
    /// the first to tick rolls it — each repaints on `rolls` changing, not on
    /// its own roll.
    fn tick(&mut self, _peers: &Peers) {
        #[cfg(target_arch = "wasm32")]
        {
            sampler::roll(js_sys::Date::now());
            let rolls = sampler::rolls();
            if rolls != self.seen_rolls.get() {
                self.seen_rolls.set(rolls);
                self.refresh_probes();
                self.watch.mark_dirty();
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn render_dom(&self, container: &web_sys::Element, _peers: &Peers, ctx: &crate::dom::DomCtx) {
        let mut probes = self.probes.borrow().clone();
        probes.pressure = self.pressure.borrow().clone();
        let out = output::MonitorOutput { history: sampler::history(), probes, own_window: ctx.window_id, about_open: self.about_open.get() };
        crate::dom::system_monitor::render(container, &out, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_monitor_holds_the_sampler_open_for_exactly_its_lifetime() {
        assert!(!sampler::active());
        let w = SystemMonitorWindow::new("p".into());
        assert!(sampler::active());
        assert_eq!(w.type_name(), "System Monitor");
        drop(w);
        assert!(!sampler::active(), "closing the monitor must put the hooks back to sleep");
    }
}
