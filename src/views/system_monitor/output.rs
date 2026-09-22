//! What the System Monitor renderer draws — plain data, no DOM.
//!
//! Every figure carries where it came from ([`Source`]), because the window's
//! second job is to teach what a browser tab can and cannot know about itself.

use crate::monitor::sampler::History;

/// Where a figure came from. Rendered beside the figure, in words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The browser reported it (a web API).
    Browser,
    /// This app counted it itself.
    Counted,
    /// An app running in a window said so about itself (`x-stats`).
    App,
    /// The browser does not expose it here.
    Unavailable,
}

/// The browser engine family, detected once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Chromium,
    Firefox,
    WebKit,
    Unknown,
}

/// Facts about the substrate, read once when the monitor opens.
#[derive(Debug, Clone, PartialEq)]
pub struct EngineFacts {
    pub engine: Engine,
    /// `navigator.hardwareConcurrency` (Safari clamps it).
    pub cores: Option<f64>,
    /// `navigator.deviceMemory`, a bucket in GB (Chromium only).
    pub device_memory_gb: Option<f64>,
    pub cross_origin_isolated: bool,
    /// The smallest non-zero step `performance.now()` took, measured here.
    pub timer_resolution_ms: Option<f64>,
    /// Long Animation Frames are observable (Chromium).
    pub long_frames: bool,
    /// `PressureObserver` exists (Chromium).
    pub pressure_api: bool,
}

/// `performance.memory` (Chromium, rounded, per renderer process).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JsHeap {
    pub used: f64,
    pub limit: f64,
}

/// The origin's storage, as `navigator.storage` reports it (imprecise by design).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StorageFigures {
    pub usage: f64,
    pub quota: f64,
    pub persisted: Option<bool>,
}

/// Everything the probes found, refreshed every few seconds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Probes {
    pub facts: Option<EngineFacts>,
    /// This app's own WebAssembly memory, in bytes — exact.
    pub wasm_memory: Option<f64>,
    pub js_heap: Option<JsHeap>,
    /// `None` while the first probe is out; then the figures, or why there are
    /// none — never "loading" forever (field report 2026-09-14).
    pub storage: Option<Result<StorageFigures, crate::views::storage::output::EstimateUnavailable>>,
    /// The latest `PressureObserver` "cpu" state, if one arrived.
    pub pressure: Option<String>,
}

/// What the *This tab* pane says about frozen time drawing does not explain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OtherWork {
    /// Too little to mention.
    Quiet,
    /// Firefox, with apps running in windows: they share this thread, so they
    /// are the likely cause. The names are the windows' apps.
    FirefoxApps(Vec<String>),
    /// Something other than drawing — the page cannot say what.
    Unexplained,
}

/// Frozen milliseconds per second that drawing does not explain before the pane
/// says anything about them — a few ms of GC every second is not news.
pub const OTHER_WORK_NOTE_MS: f64 = 50.0;

/// Decide what to say about frozen time drawing does not explain. Pure, so the
/// one inference the window makes about apps it cannot see is a native test.
/// It names apps only where the substrate makes them the cause (Firefox runs
/// app frames on this thread, measured); in Chrome their work is in another
/// process and cannot freeze this tab, so naming them would be a false lead.
pub fn other_work(engine: Option<Engine>, other_ms: f64, running_apps: &[String]) -> OtherWork {
    if !(other_ms >= OTHER_WORK_NOTE_MS) {
        return OtherWork::Quiet;
    }
    match engine {
        Some(Engine::Firefox) if !running_apps.is_empty() => OtherWork::FirefoxApps(running_apps.to_vec()),
        _ => OtherWork::Unexplained,
    }
}

#[derive(Debug, Clone)]
pub struct MonitorOutput {
    pub history: History,
    pub probes: Probes,
    /// The monitor's own window id — its row says so.
    pub own_window: crate::window::WindowId,
    pub about_open: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apps_are_named_as_the_cause_only_where_they_share_the_thread() {
        let apps = vec!["ASCII Aquarium".to_string(), "CMatrix".to_string()];
        assert_eq!(other_work(Some(Engine::Firefox), 10.0, &apps), OtherWork::Quiet, "a little GC is not news");
        assert_eq!(other_work(Some(Engine::Firefox), f64::NAN, &apps), OtherWork::Quiet);
        assert_eq!(other_work(Some(Engine::Firefox), 400.0, &apps), OtherWork::FirefoxApps(apps.clone()));
        assert_eq!(other_work(Some(Engine::Chromium), 400.0, &apps), OtherWork::Unexplained,
            "Chrome runs these apps in another process: they cannot be what froze this tab");
        assert_eq!(other_work(Some(Engine::Firefox), 400.0, &[]), OtherWork::Unexplained, "no app to blame");
        assert_eq!(other_work(None, 400.0, &apps), OtherWork::Unexplained);
    }
}
