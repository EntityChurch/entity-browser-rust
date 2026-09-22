//! The System Monitor's instruments (`DESIGN-2026-09-14-c`).
//!
//! - [`graph`] — rings of samples and the text-mode graphics drawn from them;
//! - [`sampler`] — what this tab counts about itself, dormant unless a monitor
//!   is open;
//! - [`probe`] — what the browser will tell a page (wasm only), each figure
//!   labelled with where it came from;
//! - [`load`] — the one decision the window makes about a row.

pub mod graph;
#[cfg(target_arch = "wasm32")]
pub mod probe;
pub mod sampler;

/// App → host: *here is what I did since my last report* — `busy_ms` of work
/// over `span_ms`, and optionally `instructions` executed and `memory_bytes`
/// held. A **local extension** like `x-files`, sent about once a second; an app
/// that does not send it is shown as *not reporting*, never as idle. The VM
/// pages send it from `tools/run-env/vm-sdk/vm-sdk.js`.
pub const MSG_STATS: &str = "x-stats";

/// A [`BinSource`](crate::content_site::http_poll::BinSource) that tells the
/// sampler how many bytes it fetched, and for which window (BACKLOG B-6: *"you
/// download this stuff from the entity tree, you should track it"*).
///
/// **At the source, not at each caller.** Every foreign byte this app fetches
/// goes through a `BinSource`, so a window that wraps the one it hands to
/// `foreign_cache` / `http_poll` is counted whatever those modules fetch —
/// pointers, catalogs, bundles, page closures — with no call site inside them
/// to remember. A window that does not wrap its source is simply not counted,
/// which the monitor states rather than showing zero. Costs nothing while no
/// monitor is open: the sampler hook returns on a `Cell` read.
pub struct CountingSource<S> {
    inner: S,
    window: Option<crate::window::WindowId>,
}

impl<S> CountingSource<S> {
    pub fn new(inner: S, window: Option<crate::window::WindowId>) -> Self {
        Self { inner, window }
    }
}

impl<S: crate::content_site::http_poll::BinSource> crate::content_site::http_poll::BinSource for CountingSource<S> {
    fn get(
        &self,
        url: String,
        freshness: crate::content_site::http_poll::Freshness,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, crate::content_site::http_poll::PollError>>>> {
        let fut = self.inner.get(url, freshness);
        let window = self.window;
        Box::pin(async move {
            let got = fut.await;
            if let (Some(w), Ok(bytes)) = (window, &got) {
                sampler::note_window_fetch(w, bytes.len() as f64);
            }
            got
        })
    }
}

/// How busy a window's rendering has been over the last few seconds.
///
/// Decided from what the sampler *counted* — render milliseconds per second —
/// and nothing else, so it can say "this window's rendering is busy" and never
/// "this app is using your CPU": an app frame's own script is not render work,
/// and in Chrome it is not even on this thread (`tools/monitor-probe`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Load {
    /// Under 5 ms/s — rebuilding rarely or cheaply.
    Idle,
    /// Under 50 ms/s.
    Active,
    /// 50 ms/s or more: a twentieth of every second spent rebuilding one window.
    Busy,
    /// Fewer than [`LOAD_WINDOW_S`] samples yet — too new to judge.
    Unknown,
}

/// How many seconds of history a [`Load`] is judged over.
pub const LOAD_WINDOW_S: usize = 5;

pub fn load(render_ms_per_s: &[f64]) -> Load {
    if render_ms_per_s.len() < LOAD_WINDOW_S {
        return Load::Unknown;
    }
    let tail = &render_ms_per_s[render_ms_per_s.len() - LOAD_WINDOW_S..];
    let mean = tail.iter().sum::<f64>() / LOAD_WINDOW_S as f64;
    if mean >= 50.0 {
        Load::Busy
    } else if mean >= 5.0 {
        Load::Active
    } else {
        Load::Idle
    }
}

/// How busy an app says it is, from its own reported busy milliseconds per
/// second — the same five-second mean as [`load`], on its own scale: an app is
/// *busy* when it holds most of a core, not a twentieth of a second.
pub fn app_load(busy_ms_per_s: &[f64]) -> Load {
    if busy_ms_per_s.is_empty() {
        return Load::Unknown;
    }
    let tail = &busy_ms_per_s[busy_ms_per_s.len().saturating_sub(LOAD_WINDOW_S)..];
    let mean = tail.iter().sum::<f64>() / tail.len() as f64;
    if mean >= 500.0 {
        Load::Busy
    } else if mean >= 100.0 {
        Load::Active
    } else {
        Load::Idle
    }
}

/// How the tab feels, from how long it was frozen each second — the figure a
/// person notices as taps that do not register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feel {
    /// Under 50 ms of every second frozen.
    Smooth,
    /// Under 250 ms: taps and scrolling lag.
    Sluggish,
    /// A quarter of every second or more: taps can be missed.
    Struggling,
    /// Too new to judge.
    Unknown,
}

pub fn feel(frozen_ms_per_s: &[f64]) -> Feel {
    if frozen_ms_per_s.len() < LOAD_WINDOW_S {
        return Feel::Unknown;
    }
    let tail = &frozen_ms_per_s[frozen_ms_per_s.len() - LOAD_WINDOW_S..];
    let mean = tail.iter().sum::<f64>() / LOAD_WINDOW_S as f64;
    if mean >= 250.0 {
        Feel::Struggling
    } else if mean >= 50.0 {
        Feel::Sluggish
    } else {
        Feel::Smooth
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_is_busy_when_it_holds_most_of_a_core() {
        assert_eq!(app_load(&[]), Load::Unknown, "no report is not idle");
        assert_eq!(app_load(&[900.0]), Load::Busy, "one report is enough to say what it said");
        assert_eq!(app_load(&[20.0; 5]), Load::Idle);
        assert_eq!(app_load(&[200.0; 5]), Load::Active);
        assert_eq!(app_load(&[0.0, 0.0, 0.0, 1000.0, 1000.0, 1000.0, 1000.0]), Load::Busy);
    }

    #[test]
    fn the_tab_feels_how_long_it_was_frozen() {
        assert_eq!(feel(&[900.0; 4]), Feel::Unknown);
        assert_eq!(feel(&[0.0; 5]), Feel::Smooth);
        assert_eq!(feel(&[100.0; 5]), Feel::Sluggish);
        assert_eq!(feel(&[400.0; 5]), Feel::Struggling);
    }

    #[test]
    fn load_needs_a_full_window_of_history_and_judges_its_mean() {
        assert_eq!(load(&[500.0; 4]), Load::Unknown, "too new to judge, however loud");
        assert_eq!(load(&[0.0, 0.0, 0.0, 1.0, 2.0]), Load::Idle);
        assert_eq!(load(&[10.0; 5]), Load::Active);
        assert_eq!(load(&[0.0, 0.0, 0.0, 0.0, 250.0]), Load::Busy, "one bad second in five is 50 ms/s");
        assert_eq!(load(&[900.0, 900.0, 0.0, 0.0, 0.0, 0.0, 0.0]), Load::Idle, "only the last five count");
    }
}
