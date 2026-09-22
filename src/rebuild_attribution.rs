//! Which window is rebuilding — the half `DOM: HIGH REBUILD RATE` never printed.
//!
//! # Why this exists
//!
//! The 2026-09-15 crash capture (`AUDIT-2026-09-15-a`) carries **32** occurrences
//! of
//!
//! ```text
//! DOM: HIGH REBUILD RATE rebuilds_per_sec=13 total=4438
//! ```
//!
//! and not one of them names a window. Four lines further down in the same
//! function, `DOM: SLOW REBUILD` prints a full per-section breakdown — built out
//! of `section_timings`, which `render()` populates **every frame regardless**
//! and which the rate warning simply did not read. So a sustained 11–14/sec was
//! observed for the whole life of that session and stayed unattributable, and
//! the audit had to record it as *"its own defect, and a plausible amplifier for
//! anything closure-lifetime shaped"* with no way to say whose.
//!
//! *A warning that reports a rate and not a culprit is a number, not evidence* —
//! the same shape as a log-grep panel with no must-be-present control: you learn
//! that something is wrong and nothing about where.
//!
//! # The distinction that decides whether the tally can be read at all
//!
//! Two situations produce an identical per-window tally and have **opposite**
//! causes:
//!
//! - **one window rebuilding eleven times** — that window is the defect;
//! - **eleven windows rebuilt once each by a global force** — a generation
//!   counter moved (a locale switch, or `reachability::generation()`), every
//!   open window was rebuilt for that one frame by design, and no window is
//!   implicated at all.
//!
//! So forced frames are counted **separately and always printed**. Folding them
//! into the per-window counts would make one moving counter read as an
//! eleven-window churn storm and send the next session to audit eleven windows
//! that did nothing. See [`Attribution::summary`].
//!
//! # Every counted rebuild lands in a named bucket
//!
//! `DomRenderer::render` counts a rebuild when `palette_changed ||
//! any_section_changed`, and `any_section_changed` is **also** set by a section
//! being *removed* (a window closed) — which contributes no timing entry. An
//! attribution with an empty `top` and nothing else recorded would therefore be
//! indistinguishable from an instrument that was not wired. All four producers
//! are counted, and [`Attribution::summary`] says *"nothing attributed"* in so
//! many words rather than printing an empty field.
//!
//! # Native on purpose
//!
//! `src/dom/` is `cfg(target_arch = "wasm32")`, so a decision living there is
//! reachable only through Selenium. The decision is here; `dom::mod` holds only
//! the two call sites and the log line.

use std::collections::BTreeMap;

/// How many culprits the summary names before it starts counting the rest.
///
/// A cap, and therefore something that must not be silent: a profile with forty
/// open windows would otherwise print a wall, and a truncated list that does not
/// say it was truncated reads as *"these are all of them"*.
const TOP_N: usize = 6;

/// Rebuild activity accumulated over one log interval (one second).
///
/// Cleared by [`Self::take`] at every interval boundary whether or not the
/// warning fired — an unbounded tally is a leak in the thing watching for leaks.
#[derive(Debug, Default)]
pub struct RebuildTally {
    per_section: BTreeMap<String, u32>,
    forced_frames: u64,
    palette_frames: u64,
    closed_sections: u64,
}

impl RebuildTally {
    /// Record the frame-level facts for one counted rebuild frame.
    ///
    /// `forced` is a frame on which a global generation moved and **every** open
    /// window was rebuilt regardless of its own watch; `palette` is a command
    /// palette rebuild; `closed` is the number of sections removed this frame.
    pub fn note_frame(&mut self, forced: bool, palette: bool, closed: u64) {
        if forced {
            self.forced_frames += 1;
        }
        if palette {
            self.palette_frames += 1;
        }
        self.closed_sections += closed;
    }

    /// Record one window section that actually rebuilt this frame.
    ///
    /// Keyed `type#id` — the same spelling `DOM: SLOW REBUILD` already prints, so
    /// the two warnings name the same thing the same way and a reader is not
    /// asked to correlate two vocabularies.
    pub fn note_section(&mut self, type_name: &str, window_id: u64) {
        let key = format!("{type_name}#{window_id}");
        if let Some(count) = self.per_section.get_mut(&key) {
            *count += 1;
        } else {
            self.per_section.insert(key, 1);
        }
    }

    /// Consume the interval: return what it says and reset to empty.
    pub fn take(&mut self) -> Attribution {
        let mut top: Vec<(String, u32)> = std::mem::take(&mut self.per_section).into_iter().collect();
        // Most frequent first; ties broken by name so the line is stable across
        // intervals and two captures can be diffed.
        top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let named = top.len();
        top.truncate(TOP_N);
        Attribution {
            forced_frames: std::mem::take(&mut self.forced_frames),
            palette_frames: std::mem::take(&mut self.palette_frames),
            closed_sections: std::mem::take(&mut self.closed_sections),
            top,
            omitted: named.saturating_sub(TOP_N),
        }
    }
}

/// What one interval of rebuild activity is attributable to.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Attribution {
    /// Frames on which a global generation forced every open window to rebuild.
    /// **Read this before the per-window counts** — see the module header.
    pub forced_frames: u64,
    /// Frames on which the command palette rebuilt.
    pub palette_frames: u64,
    /// Sections removed (windows closed) across the interval.
    pub closed_sections: u64,
    /// The busiest sections, most frequent first, capped at [`TOP_N`].
    pub top: Vec<(String, u32)>,
    /// Sections left out of `top` by the cap. Printed, never silent.
    pub omitted: usize,
}

impl Attribution {
    /// The one-line culprit report.
    ///
    /// Always names every non-zero bucket, so *"eleven windows once each,
    /// forced"* and *"one window eleven times"* cannot render alike. When
    /// nothing at all was recorded it says so — an empty field would be
    /// indistinguishable from an unwired instrument.
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.forced_frames > 0 {
            // Named first and worded as a cause, because it is the one bucket
            // that EXPLAINS the per-window counts beside it instead of adding
            // to them.
            parts.push(format!(
                "forced={} (every open window, not a per-window fault)",
                self.forced_frames
            ));
        }
        for (name, count) in &self.top {
            parts.push(format!("{name}={count}"));
        }
        if self.omitted > 0 {
            parts.push(format!("+{} more not shown", self.omitted));
        }
        if self.palette_frames > 0 {
            parts.push(format!("palette={}", self.palette_frames));
        }
        if self.closed_sections > 0 {
            parts.push(format!("closed={}", self.closed_sections));
        }
        if parts.is_empty() {
            return "nothing attributed".to_string();
        }
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn busy_window(n: u32) -> Attribution {
        let mut t = RebuildTally::default();
        for _ in 0..n {
            t.note_frame(false, false, 0);
            t.note_section("Apps", 7);
        }
        t.take()
    }

    #[test]
    fn one_window_rebuilding_repeatedly_is_named_with_its_count() {
        let a = busy_window(12);
        assert_eq!(a.top, vec![("Apps#7".to_string(), 12)]);
        assert!(a.summary().contains("Apps#7=12"), "{}", a.summary());
    }

    /// The distinction the module exists for. Both intervals below carry the
    /// same number of rebuilt sections; only one of them implicates a window.
    #[test]
    fn eleven_windows_forced_once_does_not_read_like_one_window_eleven_times() {
        let mut forced = RebuildTally::default();
        forced.note_frame(true, false, 0);
        for id in 0..11u64 {
            forced.note_section("Chat", id);
        }
        let forced = forced.take();

        let churning = busy_window(11);

        assert_eq!(forced.forced_frames, 1);
        assert_eq!(churning.forced_frames, 0);
        assert!(
            forced.summary().contains("forced=1"),
            "a forced sweep must say so: {}",
            forced.summary()
        );
        assert!(
            !churning.summary().contains("forced"),
            "a real churn must not be excused as a forced sweep: {}",
            churning.summary()
        );
        // And the two lines are not the same sentence, which is the whole point.
        assert_ne!(forced.summary(), churning.summary());
    }

    #[test]
    fn the_busiest_section_is_named_first() {
        let mut t = RebuildTally::default();
        for _ in 0..3 {
            t.note_section("Chat", 1);
        }
        for _ in 0..9 {
            t.note_section("Apps", 7);
        }
        let a = t.take();
        assert_eq!(a.top.first().map(|(n, c)| (n.as_str(), *c)), Some(("Apps#7", 9)));
    }

    /// A cap that does not announce itself reads as "these are all of them".
    #[test]
    fn a_truncated_list_says_how_many_it_left_out() {
        let mut t = RebuildTally::default();
        for id in 0..(TOP_N as u64 + 3) {
            t.note_section("Chat", id);
        }
        let a = t.take();
        assert_eq!(a.top.len(), TOP_N);
        assert_eq!(a.omitted, 3);
        assert!(a.summary().contains("+3 more not shown"), "{}", a.summary());
    }

    /// `any_section_changed` is set by a *removal* too, which contributes no
    /// timing entry — so this interval is real and has an empty `top`.
    #[test]
    fn a_window_closing_is_attributed_rather_than_leaving_an_empty_line() {
        let mut t = RebuildTally::default();
        t.note_frame(false, false, 1);
        let a = t.take();
        assert!(a.top.is_empty());
        assert!(a.summary().contains("closed=1"), "{}", a.summary());
        assert_ne!(a.summary(), "nothing attributed");
    }

    /// *We could not tell* must never render as *nothing was happening* — and
    /// an empty field would render as neither.
    #[test]
    fn an_interval_with_nothing_recorded_says_so_in_words() {
        assert_eq!(RebuildTally::default().take().summary(), "nothing attributed");
    }

    /// The tally is per-interval. A second that stops churning must not inherit
    /// the previous second's culprits — the warning would then keep naming a
    /// window that has gone quiet.
    #[test]
    fn taking_an_interval_clears_it() {
        let mut t = RebuildTally::default();
        t.note_frame(true, true, 2);
        t.note_section("Apps", 7);
        let _ = t.take();
        let second = t.take();
        assert_eq!(second, Attribution::default());
        assert_eq!(second.summary(), "nothing attributed");
    }
}
