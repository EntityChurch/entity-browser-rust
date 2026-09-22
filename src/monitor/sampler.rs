//! The System Monitor's sampler — what this tab can count about itself, one
//! second at a time.
//!
//! **Dormant unless a monitor is open.** Every hook starts with a thread-local
//! check of how many [`MonitorHold`]s exist; with none, a hook returns before it
//! touches anything, so a closed monitor costs one `Cell` read per frame. The
//! hold is RAII and owned by the window — the `RetryHolder` shape — so there is
//! no "stop sampling" call for a close path to forget.
//!
//! **The hooks sit on paths every member already takes** (AP44): the rAF
//! closure calls [`note_frame`] for every frame, the DOM renderer hands
//! [`note_sections`] the per-window rebuild timings it already measured (and used
//! to throw away), and the app hands [`note_open_windows`] the window list once
//! per frame. No window has to announce itself to be counted.
//!
//! What is counted, stated so a reader does not take it for more:
//!
//! - **frame work** — milliseconds per second spent inside `EntityApp::frame()`
//!   (every window's tick, action handling, and the DOM rebuild). It is **not**
//!   the main thread's whole load: event handlers and `spawn_local` futures run
//!   outside it, and in Firefox an app frame's script runs on this thread too
//!   (measured, `tools/monitor-probe`).
//! - **frame gaps** — the time between rAF callbacks. A gap far over one frame
//!   means *something* held the thread — our work, an app frame in Firefox, GC —
//!   and this sampler cannot say which. That is the honest limit of a web page.
//! - **frozen time** — how much of each second the thread was held past one
//!   frame, split into the part spent in `frame()` (this app drawing its
//!   windows) and **everything else** (garbage collection, background tasks,
//!   and in Firefox every app running in a window). The split is arithmetic on
//!   what was counted, never a claim about what the "else" was.
//! - **per-window render** — milliseconds per second each window spent
//!   rebuilding its DOM section.
//! - **per-app reports** — what an app running in a window says about itself
//!   (`x-stats`): its busy milliseconds and, for a VM, instructions executed and
//!   the memory it holds. Labelled *reported by the app*: an app can go quiet
//!   or be wrong, and most apps do not report at all.
//! - **per-app data** — bytes the host handed an app, and how many of those it
//!   first had to download.

use std::cell::RefCell;
use std::collections::BTreeMap;

use super::graph::Ring;
use crate::window::WindowId;

/// A rAF gap over this is a **stall**: three frames at 60 Hz lost.
pub const STALL_MS: f64 = 50.0;
/// A rAF gap over this is **paused**, not stalled: the browser stops rAF for a
/// hidden tab and a suspended device, and a two-minute "stall" on returning to
/// the tab would be the monitor lying about a thread that was never busy.
pub const PAUSED_MS: f64 = 5_000.0;
/// How often the sampler rolls its accumulators into history.
pub const ROLL_MS: f64 = 1_000.0;
/// One frame at 60 Hz. A faster display only makes the figure more lenient,
/// never inflated.
pub const FRAME_BUDGET_MS: f64 = 1000.0 / 60.0;
/// A rAF gap past this is time the thread was held: one frame plus the jitter a
/// healthy loop shows on a clock Firefox rounds to 1 ms, so a smooth tab reads
/// as frozen for no time at all rather than for a sprinkle of rounding.
pub const FROZEN_AFTER_MS: f64 = FRAME_BUDGET_MS + 2.0;
/// An app that has not reported for this long is shown as *not reporting* —
/// a report that stopped is not a report of zero.
pub const APP_REPORT_STALE_S: usize = 3;

#[derive(Debug, Default)]
struct WindowAcc {
    render_ms: f64,
    rebuilds: u32,
}

/// What an app said about itself this second (summed over its reports).
#[derive(Debug, Default)]
struct AppAcc {
    busy_ms: f64,
    span_ms: f64,
    instructions: f64,
    memory_bytes: Option<f64>,
    reports: u32,
}

#[derive(Debug, Default)]
struct State {
    holders: usize,
    // -- the current second
    frames: u32,
    frame_work_ms: f64,
    longest_gap_ms: f64,
    stalls: u32,
    frozen_ms: f64,
    frozen_drawing_ms: f64,
    windows: BTreeMap<WindowId, WindowAcc>,
    apps: BTreeMap<WindowId, AppAcc>,
    last_frame_start: Option<f64>,
    last_frame_elapsed: f64,
    last_roll: Option<f64>,
    // -- history
    history: History,
}

/// Everything the sampler remembers — what the window renders from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct History {
    /// How many seconds have rolled — a window repaints when this moves, which
    /// works however many monitors share the sampler.
    pub rolls: u64,
    /// Frames per second.
    pub fps: Ring,
    /// Milliseconds per second inside `frame()`.
    pub frame_work: Ring,
    /// The longest rAF gap in each second.
    pub longest_gap: Ring,
    /// Milliseconds per second the thread was held past one frame — the time
    /// taps and keys waited.
    pub frozen: Ring,
    /// The part of [`Self::frozen`] spent in `frame()`: this app drawing.
    pub frozen_drawing: Ring,
    /// Stalls since the monitor opened.
    pub stalls_total: u64,
    /// Seconds the tab was paused (rAF stopped) since the monitor opened.
    pub paused_total: u64,
    /// Per open window: its render milliseconds per second, and what it is.
    pub windows: Vec<WindowHistory>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowHistory {
    pub id: WindowId,
    pub type_name: String,
    pub title: String,
    /// The app running inside this window, if it hosts one — its name.
    pub app: Option<String>,
    pub render: Ring,
    pub rebuilds_last: u32,
    /// The app's own busy milliseconds per second, as it reported them; a
    /// second with no report pushes nothing (see [`Self::app_quiet_s`]).
    pub app_busy: Ring,
    /// Instructions per second, for an app that counts them (a VM).
    pub app_ips: Ring,
    /// The memory the app says it holds, bytes — its latest report.
    pub app_memory: Option<f64>,
    /// Seconds since the app last reported; `None` if it never has.
    pub app_quiet_s: Option<usize>,
    /// Bytes the host has handed this app since the monitor opened.
    pub data_bytes: f64,
    /// Of those, bytes the host first had to download.
    pub downloaded_bytes: f64,
    /// Bytes this window fetched for itself — catalogs, bundles, pages — since
    /// the monitor opened ([`super::CountingSource`]).
    pub fetched_bytes: f64,
}

impl WindowHistory {
    /// Whether the app's own report is current — reported, and recently.
    pub fn app_reporting(&self) -> bool {
        self.app_quiet_s.is_some_and(|q| q < APP_REPORT_STALE_S)
    }
}

/// One open window, as the renderer hands it to [`note_open_windows`].
pub struct OpenWindow<'a> {
    pub id: WindowId,
    pub type_name: &'a str,
    pub title: String,
    pub app: Option<String>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// While one of these exists, the hooks record. Dropping the last one clears
/// the history, so a monitor opened later starts clean rather than showing a
/// graph with a hole where it was closed.
#[derive(Debug)]
pub struct MonitorHold(());

pub fn hold() -> MonitorHold {
    STATE.with(|s| s.borrow_mut().holders += 1);
    MonitorHold(())
}

impl Drop for MonitorHold {
    fn drop(&mut self) {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.holders = s.holders.saturating_sub(1);
            if s.holders == 0 {
                *s = State::default();
            }
        });
    }
}

/// Whether any monitor is open — for a hook whose ARGUMENTS cost something to
/// build (the window list allocates titles).
pub fn active() -> bool {
    STATE.with(|s| s.try_borrow().map(|s| s.holders > 0).unwrap_or(false))
}

fn with_active(f: impl FnOnce(&mut State)) {
    STATE.with(|s| {
        // `try_borrow_mut`: a hook must never be the thing that panics a frame.
        if let Ok(mut s) = s.try_borrow_mut() {
            if s.holders > 0 {
                f(&mut s);
            }
        }
    });
}

/// One rAF callback: when it started and how long `frame()` took.
pub fn note_frame(start_ms: f64, elapsed_ms: f64) {
    with_active(|s| {
        s.frames += 1;
        s.frame_work_ms += elapsed_ms.max(0.0);
        if let Some(prev) = s.last_frame_start {
            let gap = start_ms - prev;
            if gap > PAUSED_MS {
                s.history.paused_total += (gap / 1000.0) as u64;
            } else {
                if gap > STALL_MS {
                    s.stalls += 1;
                }
                s.longest_gap_ms = s.longest_gap_ms.max(gap);
                // The gap held the previous frame's own work plus whatever else
                // ran before the browser could paint again. Only the excess over
                // one frame is time anyone waited; the previous `frame()` is the
                // part of it this app can account for.
                let excess = (gap - FROZEN_AFTER_MS).max(0.0);
                s.frozen_ms += excess;
                s.frozen_drawing_ms += excess.min(s.last_frame_elapsed);
            }
        }
        s.last_frame_start = Some(start_ms);
        s.last_frame_elapsed = elapsed_ms.max(0.0);
    });
}

/// The DOM renderer's per-window rebuild timings for one frame.
pub fn note_sections(timings: &[(String, WindowId, f64)]) {
    with_active(|s| {
        for (_, id, ms) in timings {
            let acc = s.windows.entry(*id).or_default();
            acc.render_ms += ms.max(0.0);
            acc.rebuilds += 1;
        }
    });
}

/// The open windows, once per frame. A window not in this list is dropped from
/// history — closing it is what removes its row.
pub fn note_open_windows<'a>(open: impl Iterator<Item = OpenWindow<'a>>) {
    with_active(|s| {
        let mut seen = Vec::new();
        for w in open {
            seen.push(w.id);
            match s.history.windows.iter_mut().find(|h| h.id == w.id) {
                Some(h) => {
                    if h.app != w.app {
                        // A different app in the same window: its history is not
                        // the new one's.
                        h.app_busy = Ring::default();
                        h.app_ips = Ring::default();
                        h.app_memory = None;
                        h.app_quiet_s = None;
                        h.app = w.app;
                    }
                    h.title = w.title;
                }
                None => s.history.windows.push(WindowHistory {
                    id: w.id,
                    type_name: w.type_name.to_string(),
                    title: w.title,
                    app: w.app,
                    ..Default::default()
                }),
            }
        }
        s.history.windows.retain(|w| seen.contains(&w.id));
        s.apps.retain(|id, _| seen.contains(id));
    });
}

/// An app's own report (`x-stats`): `busy_ms` of work over the `span_ms` since
/// its last report, instructions executed in that span, the memory it holds.
pub fn note_app_stats(window: WindowId, busy_ms: f64, span_ms: f64, instructions: Option<f64>, memory_bytes: Option<f64>) {
    // A report is untrusted input from a sandboxed page: nothing negative, NaN
    // or absurd reaches a graph.
    let finite = |v: f64| v.is_finite() && v >= 0.0;
    if !finite(busy_ms) || !finite(span_ms) || span_ms <= 0.0 || span_ms > 60_000.0 {
        return;
    }
    with_active(|s| {
        let acc = s.apps.entry(window).or_default();
        acc.busy_ms += busy_ms.min(span_ms);
        acc.span_ms += span_ms;
        acc.instructions += instructions.filter(|v| finite(*v)).unwrap_or(0.0);
        if let Some(m) = memory_bytes.filter(|v| finite(*v)) {
            acc.memory_bytes = Some(m);
        }
        acc.reports += 1;
    });
}

/// Bytes the host handed an app, and bytes it had to download first to do so.
/// Two figures rather than a flag, because the two happen at different moments:
/// the download completes, then the answer is posted.
pub fn note_app_data(window: WindowId, handed: f64, downloaded: f64) {
    let ok = |v: f64| v.is_finite() && v >= 0.0;
    if !ok(handed) || !ok(downloaded) {
        return;
    }
    with_active(|s| {
        if let Some(w) = s.history.windows.iter_mut().find(|w| w.id == window) {
            w.data_bytes += handed;
            w.downloaded_bytes += downloaded;
        }
    });
}

/// A window's own fetch completed with `bytes` ([`super::CountingSource`]).
pub fn note_window_fetch(window: WindowId, bytes: f64) {
    if !(bytes.is_finite() && bytes >= 0.0) {
        return;
    }
    with_active(|s| {
        if let Some(w) = s.history.windows.iter_mut().find(|w| w.id == window) {
            w.fetched_bytes += bytes;
        }
    });
}

/// Roll the current second into history if a second has passed. `true` when it
/// rolled — the window repaints on that and not on every frame.
pub fn roll(now_ms: f64) -> bool {
    let mut rolled = false;
    with_active(|s| {
        let Some(last) = s.last_roll else {
            s.last_roll = Some(now_ms);
            return;
        };
        if now_ms - last < ROLL_MS {
            return;
        }
        // Normalise to per-second rates over the real interval, so a late roll
        // does not read as a busier second.
        let secs = ((now_ms - last) / 1000.0).max(1.0);
        s.history.fps.push(s.frames as f64 / secs);
        s.history.frame_work.push(s.frame_work_ms / secs);
        s.history.longest_gap.push(s.longest_gap_ms);
        s.history.frozen.push((s.frozen_ms / secs).min(1000.0));
        s.history.frozen_drawing.push((s.frozen_drawing_ms / secs).min(1000.0));
        s.history.stalls_total += s.stalls as u64;
        let accs = std::mem::take(&mut s.windows);
        let apps = std::mem::take(&mut s.apps);
        for w in &mut s.history.windows {
            let acc = accs.get(&w.id);
            w.render.push(acc.map(|a| a.render_ms / secs).unwrap_or(0.0));
            w.rebuilds_last = acc.map(|a| a.rebuilds).unwrap_or(0);
            match apps.get(&w.id).filter(|a| a.reports > 0) {
                Some(a) => {
                    // Per second of the app's OWN span, so a report arriving a
                    // little late does not read as a busier second.
                    let per_s = 1000.0 / a.span_ms.max(1.0);
                    w.app_busy.push((a.busy_ms * per_s).min(1000.0));
                    w.app_ips.push(a.instructions * per_s);
                    if a.memory_bytes.is_some() {
                        w.app_memory = a.memory_bytes;
                    }
                    w.app_quiet_s = Some(0);
                }
                None => {
                    if let Some(q) = w.app_quiet_s.as_mut() {
                        *q += 1;
                    }
                }
            }
        }
        s.frames = 0;
        s.frame_work_ms = 0.0;
        s.longest_gap_ms = 0.0;
        s.stalls = 0;
        s.frozen_ms = 0.0;
        s.frozen_drawing_ms = 0.0;
        s.last_roll = Some(now_ms);
        s.history.rolls += 1;
        rolled = true;
    });
    rolled
}

/// How many seconds have rolled, without cloning the history.
pub fn rolls() -> u64 {
    STATE.with(|s| s.try_borrow().map(|s| s.history.rolls).unwrap_or(0))
}

/// The history, for rendering.
pub fn history() -> History {
    STATE.with(|s| s.try_borrow().map(|s| s.history.clone()).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(id: WindowId, type_name: &'static str, app: Option<&str>) -> OpenWindow<'static> {
        OpenWindow { id, type_name, title: type_name.to_string(), app: app.map(String::from) }
    }

    fn frames(from: f64, step: f64, n: usize, work: f64) -> f64 {
        let mut t = from;
        for _ in 0..n {
            note_frame(t, work);
            t += step;
        }
        t
    }

    #[test]
    fn with_no_monitor_open_nothing_is_recorded() {
        note_frame(0.0, 5.0);
        note_sections(&[("Shell".into(), 1, 3.0)]);
        assert!(!roll(0.0) && !roll(5_000.0));
        assert_eq!(history(), History::default());
    }

    #[test]
    fn a_second_of_frames_becomes_one_sample_per_signal() {
        let _hold = hold();
        roll(0.0); // arms the first interval
        let t = frames(0.0, 1000.0 / 60.0, 60, 2.0);
        note_frame(t + 120.0, 2.0); // one 136 ms gap: a stall
        assert!(roll(1_000.0));
        let h = history();
        assert_eq!(h.fps.last(), Some(61.0));
        assert_eq!(h.frame_work.last(), Some(122.0));
        assert!(h.longest_gap.last().unwrap() > STALL_MS);
        assert_eq!(h.stalls_total, 1);
        assert!(!roll(1_500.0), "not a second yet");
    }

    /// A hidden tab stops rAF. Returning to it is a long gap, and calling that a
    /// stall would be the monitor accusing a thread that was asleep.
    #[test]
    fn a_long_gap_is_paused_not_a_stall() {
        let _hold = hold();
        roll(0.0);
        note_frame(0.0, 1.0);
        note_frame(120_000.0, 1.0);
        roll(121_000.0);
        let h = history();
        assert_eq!(h.stalls_total, 0);
        assert_eq!(h.paused_total, 120);
        assert_eq!(h.longest_gap.last(), Some(0.0));
    }

    #[test]
    fn a_window_is_charged_for_its_own_rebuilds_and_leaves_when_it_closes() {
        let _hold = hold();
        roll(0.0);
        note_open_windows([open(1, "Shell", None), open(2, "Apps", None)].into_iter());
        note_sections(&[("Apps".into(), 2, 30.0), ("Apps".into(), 2, 10.0)]);
        roll(1_000.0);
        let h = history();
        let apps = h.windows.iter().find(|w| w.id == 2).unwrap();
        let shell = h.windows.iter().find(|w| w.id == 1).unwrap();
        assert_eq!(apps.render.last(), Some(40.0));
        assert_eq!(apps.rebuilds_last, 2);
        assert_eq!(shell.render.last(), Some(0.0), "a window that did not rebuild costs nothing");
        note_open_windows([open(1, "Shell", None)].into_iter());
        assert!(history().windows.iter().all(|w| w.id != 2), "a closed window's row goes");
    }

    /// The figure a person can feel: how long taps waited, and how much of that
    /// this app's own drawing accounts for. A thread held by something else — an
    /// app frame in Firefox — shows up as frozen time that drawing does not
    /// explain, which is the only honest way a page can see it.
    #[test]
    fn frozen_time_is_split_into_drawing_and_everything_else() {
        let _hold = hold();
        roll(0.0);
        // Healthy frames: 16.7 ms apart, 2 ms of work. Nobody waited.
        // (and a 1 ms jitter on top, as Firefox's rounded clock shows)
        let t = frames(0.0, FRAME_BUDGET_MS + 1.0, 10, 2.0);
        // One frame whose own work took 100 ms: the next gap is ~117 ms, all of
        // the excess explained by drawing.
        note_frame(t, 100.0);
        let t = t + 100.0 + FRAME_BUDGET_MS;
        // Then a 300 ms gap after a 1 ms frame: something else held the thread.
        note_frame(t, 1.0);
        note_frame(t + 300.0, 1.0);
        roll(1_000.0);
        let h = history();
        let frozen = h.frozen.last().unwrap();
        let drawing = h.frozen_drawing.last().unwrap();
        let first = 100.0 + FRAME_BUDGET_MS - FROZEN_AFTER_MS;
        assert!((frozen - (first + (300.0 - FROZEN_AFTER_MS))).abs() < 0.01, "frozen {frozen}");
        assert!((drawing - (first + 1.0)).abs() < 0.01, "drawing explains its own long frame and the 1 ms one: {drawing}");
        assert!(frozen - drawing > 280.0, "the rest is someone else's: {frozen} - {drawing}");
    }

    #[test]
    fn a_healthy_tab_is_frozen_for_no_time_at_all() {
        let _hold = hold();
        roll(0.0);
        frames(0.0, FRAME_BUDGET_MS, 60, 3.0);
        roll(1_000.0);
        assert_eq!(history().frozen.last(), Some(0.0));
    }

    /// An app's report becomes a per-second figure over its own span, and a
    /// report that stops reads as *not reporting* — never as an idle app.
    #[test]
    fn an_app_report_is_charged_to_its_window_and_goes_stale_when_it_stops() {
        let _hold = hold();
        roll(0.0);
        note_open_windows([open(3, "Apps", Some("Alpine"))].into_iter());
        note_app_stats(3, 400.0, 500.0, Some(20e6), Some(300e6));
        note_app_stats(3, 400.0, 500.0, Some(20e6), None);
        note_app_stats(3, f64::NAN, 500.0, None, None); // hostile: ignored
        note_app_stats(3, 5.0, -1.0, None, None); // hostile: ignored
        roll(1_000.0);
        let w = history().windows.into_iter().find(|w| w.id == 3).unwrap();
        assert_eq!(w.app_busy.last(), Some(800.0));
        assert_eq!(w.app_ips.last(), Some(40e6));
        assert_eq!(w.app_memory, Some(300e6), "the latest memory figure is kept across a report without one");
        assert!(w.app_reporting());
        for i in 2..=(1 + APP_REPORT_STALE_S) {
            roll(i as f64 * 1_000.0);
        }
        let w = history().windows.into_iter().find(|w| w.id == 3).unwrap();
        assert!(!w.app_reporting(), "silence is not a report of idle: {:?}", w.app_quiet_s);
        assert_eq!(w.app_busy.count(), 1, "a quiet second pushes nothing that would draw as zero");
    }

    #[test]
    fn a_different_app_in_the_same_window_starts_its_own_history() {
        let _hold = hold();
        roll(0.0);
        note_open_windows([open(3, "Apps", Some("Alpine"))].into_iter());
        note_app_stats(3, 900.0, 1000.0, None, Some(1.0));
        roll(1_000.0);
        note_open_windows([open(3, "Apps", Some("KolibriOS"))].into_iter());
        let w = history().windows.into_iter().find(|w| w.id == 3).unwrap();
        assert_eq!(w.app.as_deref(), Some("KolibriOS"));
        assert_eq!(w.app_busy.count(), 0);
        assert_eq!(w.app_memory, None);
        assert_eq!(w.app_quiet_s, None);
    }

    #[test]
    fn data_handed_to_an_app_is_counted_and_downloads_are_told_apart() {
        let _hold = hold();
        note_open_windows([open(3, "Apps", Some("Alpine"))].into_iter());
        note_app_data(3, 1000.0, 0.0);
        note_app_data(3, 0.0, 500.0); // a download completes…
        note_app_data(3, 500.0, 0.0); // …then those bytes are handed over
        note_app_data(3, f64::INFINITY, 0.0); // hostile: ignored
        note_app_data(9, 500.0, 500.0); // no such window: dropped
        let w = history().windows.into_iter().find(|w| w.id == 3).unwrap();
        assert_eq!((w.data_bytes, w.downloaded_bytes), (1500.0, 500.0));
    }

    #[test]
    fn a_windows_own_fetches_are_counted_apart_from_data_handed_to_its_app() {
        let _hold = hold();
        note_open_windows([open(4, "Apps", None)].into_iter());
        note_window_fetch(4, 58.0);
        note_window_fetch(4, 1_000.0);
        note_window_fetch(4, f64::NAN);
        note_window_fetch(8, 5.0); // no such window
        let w = history().windows.into_iter().find(|w| w.id == 4).unwrap();
        assert_eq!((w.fetched_bytes, w.data_bytes, w.downloaded_bytes), (1058.0, 0.0, 0.0));
    }

    #[tokio::test]
    async fn a_counting_source_counts_what_its_window_fetched_and_nothing_it_failed_to() {
        use crate::content_site::http_poll::{BinSource, Freshness, PollError};
        struct Fixed;
        impl BinSource for Fixed {
            fn get(&self, url: String, _f: Freshness) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, PollError>>>> {
                Box::pin(async move { if url.ends_with("ok") { Ok(vec![0u8; 300]) } else { Err(PollError::NotFound(404)) } })
            }
        }
        let _hold = hold();
        note_open_windows([open(6, "Site Browser", None)].into_iter());
        let src = super::super::CountingSource::new(Fixed, Some(6));
        assert!(src.get("a/ok".into(), Freshness::Mutable).await.is_ok());
        assert!(src.get("a/missing".into(), Freshness::Mutable).await.is_err());
        let untracked = super::super::CountingSource::new(Fixed, None);
        let _ = untracked.get("b/ok".into(), Freshness::Mutable).await;
        let w = history().windows.into_iter().find(|w| w.id == 6).unwrap();
        assert_eq!(w.fetched_bytes, 300.0);
    }

    #[test]
    fn closing_the_last_monitor_forgets_the_history() {
        {
            let _a = hold();
            let _b = hold();
            roll(0.0);
            note_frame(0.0, 1.0);
            roll(1_000.0);
            drop(_a);
            assert_eq!(history().fps.count(), 1, "one monitor is still open");
        }
        assert_eq!(history(), History::default());
    }
}
