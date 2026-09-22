//! The status bar as a **list of segments**, not a formatted string.
//!
//! `DESIGN-2026-09-16-THE-STATUS-BAR-IS-A-SURFACE-NOT-A-STRING` is the design;
//! this is §3. Everything here is **pure** — metrics in, segments out, and a
//! layout that is a filter over them — so every width tier is a native test and
//! nothing in this file needs a DOM.
//!
//! ## Why a list rather than one string
//!
//! It was `status_summary(windows, peers, can_persist) -> String`. A string
//! cannot be reordered, hidden, themed or switched off without editing the
//! function that builds it, which is why the bar had carried the same three
//! facts since it was written. A list buys three things:
//!
//! - **Width tiers are a filter, not a branch.** [`layout`] sorts by priority
//!   and drops from the tail until it fits. A segment added later cannot break
//!   the narrow layout, because narrow is *what survives* rather than a
//!   hand-written short version somebody has to remember to update (AP44).
//! - **Settings and theming have something to name.** [`SegmentId`] is stable
//!   and is the key for *show this / hide this / colour this*. Without it a
//!   settings panel would be editing a format string.
//! - **The composition stays testable**, which is the one property the string
//!   version had and the reason it had tests at all.
//!
//! ## Cells, not pixels
//!
//! [`layout`] measures in **character cells**, not pixels: it is integer,
//! exact, and has no font metrics in it. The px→cells estimate lives at the
//! boundary in one named constant ([`APPROX_CHAR_PX`]), where being wrong moves
//! a tier boundary by a segment and can never produce a broken layout — the
//! worst case is one segment fewer than would have fitted.
//!
//! ## One character row, because the bar is 28px
//!
//! A braille cell is already four dot levels tall, so a one-row sparkline has
//! four levels of height and fits the existing bar with no layout change.
//! [`Gauge::rows`] exists so a taller bar can ask for more later
//! (`graph::braille_rows` has taken a `rows` argument since it was written) —
//! it is a field rather than a baked `1` for that reason, and **raising it
//! above 1 requires growing `#status-bar` first**, because the glyphs are
//! joined by newlines and a 28px bar will clip them.
//!
//! ## Two groups, and the fixed one is on the left
//!
//! [`SegmentGroup`] splits the bar into **`Fixed`** — windows, peers,
//! durability, the three facts the bar has always carried and which do not move
//! second to second — and **`Live`**, the sampled gauges. The fixed group is
//! built first so it sits immediately after the product name, and the live
//! group fills whatever is left. That is the operator's arrangement and it is a
//! property of the build order plus [`layout`]'s promise never to reorder, so
//! it is asserted here rather than left to the renderer.
//!
//! ## A gauge shows a shape, not a number
//!
//! The braille *is* the reading. A `0 ms` beside it is noise at the exact moment
//! the gauge is telling you everything is fine, and it is the widest part of a
//! segment that is mostly idle. So [`Gauge`] carries a **glyph** (what am I
//! looking at) and an optional **caption** (which app), and the number lives in
//! the accessible label, where a screen reader and a hover can still reach it.
//!
//! ## The busiest app must not flop
//!
//! Ranking apps by their latest sample makes two apps a few milliseconds apart
//! trade places every second, which is the single thing the operator called
//! annoying. [`rank_apps`] is the fix and it has two independent halves:
//!
//! - **Score on a trailing mean** ([`RANK_WINDOW`]), not the last sample, so one
//!   busy second does not reorder the bar.
//! - **An incumbent holds its slot** unless a challenger beats it by
//!   [`DISPLACE_RATIO`] *and* by [`DISPLACE_FLOOR_MS`] in absolute terms. The
//!   ratio alone is not enough — two apps at 1 ms and 2 ms differ by 2× and
//!   neither is doing anything.
//!
//! It is **pure and takes the previous order as an argument**; the caller (the
//! frame path) carries it. Hysteresis is state by definition, and hiding that
//! state in a `thread_local` would put the one decision in this file that
//! `make test` could not reach.
//!
//! [`Level`] is smoothed the same way ([`LEVEL_WINDOW`]) for the same reason: a
//! colour flipping green↔amber on the band boundary every second is the same
//! defect wearing a different costume. The spark still draws every raw sample,
//! so nothing is hidden — the *emphasis* is damped, not the reading.
//!
//! ## Interactivity
//!
//! Segments are **inert today** and [`Segment`] carries no action. The operator
//! asked whether interactivity is possible (a global mute was the example); it
//! is — a segment is a DOM element and `DomCtx::on_action` is the existing way
//! to make one clickable — but nothing here has a behaviour, and adding one is
//! a field plus a handler rather than a reshape. Deliberately not built: there
//! is no second contributor and no agreed behaviour yet.
//!
//! ## Off
//!
//! There is no `enabled` flag here. The bar is off when nothing holds the
//! sampler: `history()` answers empty, every gauge is omitted by construction
//! and the fixed group renders exactly as it did before any of this existed.
//! The setting drops the hold (`EntityApp::status_gauge_hold`) — so switching
//! it off stops the *sampling*, not just the drawing, and the segment model
//! needs no branch for it.

use crate::monitor::graph;
use crate::monitor::sampler::{History, STALL_MS};

/// A gauge's colour band. **The level is encoded twice** — in bar height and in
/// colour — which is what makes green/amber/red safe for a red-green colourblind
/// reader: the height is the reading and the colour is the emphasis. A palette
/// change must keep both, so this type never carries a colour, only a band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// **No judgement available.** An app reporting 400 ms of work per second
    /// is doing what somebody asked it to do — that is not a fault, and green
    /// would not be right either: a healthy mark is as much a verdict as a bad
    /// one. Which app is costing you the most is already said by the ranking
    /// (it is the leftmost), so the colour has nothing left to add and says
    /// nothing. Before 2026-09-16 the app gauge was scored on the *held-time*
    /// bands and a running VM was permanently red.
    Neutral,
    Good,
    Warn,
    Bad,
}

impl Level {
    /// The CSS class the renderer puts on the segment. Colour itself is a theme
    /// token (`REFERENCE-THEMING.md`), never a literal here.
    pub fn class(self) -> &'static str {
        match self {
            Level::Neutral => "seg-neutral",
            Level::Good => "seg-good",
            Level::Warn => "seg-warn",
            Level::Bad => "seg-bad",
        }
    }
}

/// **The band boundaries, derived rather than invented.**
///
/// `Good` ends at [`STALL_MS`] of held time per second — one stall's worth,
/// i.e. three frames lost at 60 Hz, which is the number the sampler already
/// uses to call a gap a stall. `Warn` ends at four times that. Deriving the
/// first boundary means a §12.4-style retune of `STALL_MS` moves the gauge with
/// it instead of leaving two numbers to disagree (C15).
pub fn level_for(held_ms_per_s: f64) -> Level {
    if !held_ms_per_s.is_finite() || held_ms_per_s <= STALL_MS {
        Level::Good
    } else if held_ms_per_s <= STALL_MS * 4.0 {
        Level::Warn
    } else {
        Level::Bad
    }
}

/// How many trailing samples the **colour** is averaged over.
///
/// The band is emphasis, not the reading — the sparkline draws every raw
/// sample — so a colour that flips on one second's excursion is noise a person
/// has to learn to ignore. Three seconds is short enough that a real stall is
/// still coloured within the width of the spark that shows it.
pub const LEVEL_WINDOW: usize = 3;

/// The mean of the last `n` samples, or `0.0` for an empty slice. Non-finite
/// samples are skipped rather than poisoning the mean — one unreadable reading
/// must not repaint the whole segment.
fn trailing_mean(samples: &[f64], n: usize) -> f64 {
    let tail = &samples[samples.len().saturating_sub(n.max(1))..];
    let (sum, count) = tail
        .iter()
        .filter(|v| v.is_finite())
        .fold((0.0, 0usize), |(s, c), v| (s + v, c + 1));
    if count == 0 {
        0.0
    } else {
        sum / count as f64
    }
}

/// The band for a series, smoothed over [`LEVEL_WINDOW`]. See the module doc:
/// the height is the reading, the colour is the emphasis, and only the
/// emphasis is damped.
pub fn level_for_samples(samples: &[f64]) -> Level {
    level_for(trailing_mean(samples, LEVEL_WINDOW))
}

/// Which half of the bar a segment belongs to. **Fixed sits left of Live**, and
/// the renderer draws a divider at the boundary. See the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SegmentGroup {
    /// Facts about the profile that do not move second to second.
    Fixed,
    /// Sampled gauges.
    Live,
}

/// Which app slot a segment is. **A closed enum, not an index**, so
/// [`Segment::selector_key`] is total: there is no out-of-range arm to fall
/// back from, and no fallback means no two slots can ever share a key. Adding a
/// fourth slot is a variant the compiler makes you name everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AppSlot {
    First,
    Second,
    Third,
}

impl AppSlot {
    /// Ranked order, most demanding first. `.len()` is how many app segments
    /// the bar can ever show — [`segments`] iterates this rather than counting.
    pub const ALL: [AppSlot; 3] = [AppSlot::First, AppSlot::Second, AppSlot::Third];

    /// Drop order within the app group: the third goes before the second.
    fn rank(self) -> u8 {
        match self {
            AppSlot::First => 0,
            AppSlot::Second => 1,
            AppSlot::Third => 2,
        }
    }
}

/// How many apps the bar shows at its widest. The operator's ask was *"the top
/// three by default"*; they drop one at a time as it narrows, so top-1 is a
/// width tier rather than a second code path.
pub const APP_SLOTS: usize = AppSlot::ALL.len();

/// A stable name for a segment. **Never positional** — settings, theming and
/// tests key off this, so reordering the bar must not rename anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SegmentId {
    /// Open window count.
    Windows,
    /// Peer count.
    Peers,
    /// Whether this profile persists.
    Durability,
    /// How much of each second the thread was held past one frame — *is this
    /// tab smooth*, which is the question the operator was opening the System
    /// Monitor to answer.
    Smoothness,
    /// The part of that which was us drawing windows.
    Drawing,
    /// One of the busiest reporting apps' own busy time (`x-stats`), by slot.
    App(AppSlot),
    // NOTE: there is deliberately no `Summary` segment. The phrase was designed
    // as the narrowest tier and is WIDER than the icons, so it is unreachable by
    // narrowing — see `summary_text`, which is the bar's aria-label instead.
}

/// **The bar's glyphs, in one place.**
///
/// Every one is BMP and **non-emoji** — Geometric Shapes / Arrows / Mathematical
/// Operators — deliberately. An emoji-presentation codepoint is rendered by a
/// colour font at an unpredictable advance width, which would break [`layout`]'s
/// cell arithmetic and clip a 28px bar. A glyph is never the only thing saying
/// what a segment is: each carries `aria-label` words and a `title`, and the
/// glyph itself is `aria-hidden`.
///
/// They are also chosen to be distinguishable *from each other* at 13px, which
/// is why the two gauges are not both squares.
pub mod glyph {
    /// Held time — a clock face. Not literally a CPU; what it measures is the
    /// time typing and clicks spent waiting.
    pub const SMOOTHNESS: &str = "◷";
    /// Drawing — a hatched square, the "fill" mark.
    pub const DRAWING: &str = "▨";
    /// A running app — a small play mark. The app's name is its caption, so
    /// this only has to say *which kind of thing this is*.
    pub const APP: &str = "▸";
    pub const WINDOWS: &str = "⊞";
    pub const PEERS: &str = "⇄";
    pub const SAVED: &str = "✓";
    pub const NOT_SAVED: &str = "⚠";
}

/// A sparkline with a glyph saying what it measures, and optionally a caption
/// saying *whose* it is.
///
/// ⚠ **There is deliberately no number here.** The braille is the reading; a
/// `0 ms` beside it is the widest part of an idle segment and says nothing the
/// flat line does not. The figure is in [`Segment::label`], which is the
/// accessible name and the hover — reachable, just not painted six times across
/// a 28px bar.
#[derive(Debug, Clone, PartialEq)]
pub struct Gauge {
    /// One of [`glyph`]. What this gauge measures.
    pub glyph: &'static str,
    /// Pre-rendered braille, newest sample at the right edge.
    pub spark: String,
    pub level: Level,
    /// Whose reading this is — an app name. `None` for the two whole-tab
    /// gauges, where the glyph already says everything.
    pub caption: Option<String>,
    /// Character rows the sparkline occupies. See the module doc: 1 today.
    pub rows: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SegmentBody {
    Gauge(Gauge),
    /// A glyph and an already-localized value beside it.
    Icon { glyph: &'static str, text: String },
    Text(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub id: SegmentId,
    /// Which half of the bar this belongs to. The renderer draws a divider at
    /// the boundary; [`segments`] guarantees every `Fixed` precedes every
    /// `Live`.
    pub group: SegmentGroup,
    /// **Lower survives.** `layout` drops the highest numbers first.
    pub priority: u8,
    pub body: SegmentBody,
    /// What a screen reader gets. **Not optional**: a gauge whose braille is
    /// `aria-hidden` and whose label is empty is a segment a screen reader
    /// cannot read at all.
    pub label: String,
    pub tooltip: Option<String>,
}

impl Segment {
    /// A stable selector key — the element's `data-seg`, and what a browser
    /// gate selects on. Derived from [`SegmentId`] so it cannot drift from the
    /// model's own name, and **native** so a duplicate fails `make test`
    /// (`mod dom` is wasm-only, so a key living beside the renderer would be
    /// checked by nothing).
    pub fn selector_key(&self) -> &'static str {
        match self.id {
            SegmentId::Smoothness => "smoothness",
            SegmentId::Drawing => "drawing",
            SegmentId::App(AppSlot::First) => "app-1",
            SegmentId::App(AppSlot::Second) => "app-2",
            SegmentId::App(AppSlot::Third) => "app-3",
            SegmentId::Windows => "windows",
            SegmentId::Peers => "peers",
            SegmentId::Durability => "durability",
        }
    }

    /// Width in character cells, including the gap that follows it. An
    /// over-estimate is the safe direction — it drops a segment early rather
    /// than overflowing the bar.
    pub fn cells(&self) -> usize {
        const GAP: usize = 2;
        let body = match &self.body {
            // glyph + space + spark, then the caption if there is one. The
            // spark is fixed-width by construction.
            SegmentBody::Gauge(g) => {
                let spark = g.spark.split('\n').next().map_or(0, |l| l.chars().count());
                let caption = g.caption.as_ref().map_or(0, |c| 1 + c.chars().count());
                g.glyph.chars().count() + 1 + spark + caption
            }
            SegmentBody::Icon { glyph, text } => {
                glyph.chars().count() + 1 + text.chars().count()
            }
            SegmentBody::Text(t) => t.chars().count(),
        };
        body + GAP
    }
}

/// How wide a bar sparkline is, in characters. Twelve cells is twenty-four
/// samples — twenty-four seconds of history at 1 Hz, which is long enough to
/// see a stall arrive and short enough to fit beside the icons.
pub const SPARK_CELLS: usize = 12;

/// One character's width in the bar's 12px font, as an estimate.
///
/// **The only font metric in the feature, and being wrong is cheap by
/// construction:** [`layout`] measures in whole cells, so an over-estimate drops
/// one segment earlier than needed and an under-estimate lets one clip against
/// the host's `overflow: hidden`. Neither breaks the layout. Deliberately a
/// little generous, because dropping early is the safe direction.
///
/// Measuring it honestly would mean a `canvas` text metric per frame for a
/// number that decides a tier boundary. That trade is not worth it, and this
/// comment is here so nobody "fixes" it into one.
pub const APPROX_CHAR_PX: f64 = 7.5;

/// How many character cells a segment host of `host_px` has room for.
///
/// Saturating: a bar mid-layout reports 0, which renders nothing for one frame
/// and corrects on the next — better than guessing a width and painting a tier
/// the bar cannot hold.
pub fn cells_available(host_px: f64) -> usize {
    if !host_px.is_finite() || host_px <= 0.0 {
        return 0;
    }
    (host_px / APPROX_CHAR_PX) as usize
}

/// One reporting app's contribution to the bar.
///
/// Identified by **window id, never by name** — two windows can run the same
/// app, and a ranking that keyed on the name would have no way to tell an
/// incumbent from its twin, which is exactly the case hysteresis has to get
/// right.
#[derive(Debug, Clone, PartialEq)]
pub struct AppReading {
    pub window: crate::window::WindowId,
    pub name: String,
    /// Busy ms/s as the app reported it, oldest first.
    pub samples: Vec<f64>,
}

/// How many trailing samples an app's **rank** is scored on. Five seconds: long
/// enough that one busy frame does not reorder the bar, short enough that a VM
/// somebody just started climbs within a glance.
pub const RANK_WINDOW: usize = 5;

/// A challenger must exceed the incumbent's score by this factor to take its
/// slot.
pub const DISPLACE_RATIO: f64 = 1.25;

/// …**and** by this many ms/s in absolute terms. The ratio alone is not enough:
/// 1 ms against 2 ms is a 2× win between two apps that are both idle, and
/// ratio-only hysteresis flops hardest exactly where nothing is happening.
pub const DISPLACE_FLOOR_MS: f64 = 5.0;

/// An app's rank score: the mean of its last [`RANK_WINDOW`] samples.
pub fn rank_score(samples: &[f64]) -> f64 {
    trailing_mean(samples, RANK_WINDOW)
}

/// Whether `challenger` has earned `incumbent`'s slot. Both tests, never one.
fn clearly_beats(challenger: f64, incumbent: f64) -> bool {
    challenger > incumbent * DISPLACE_RATIO && challenger - incumbent > DISPLACE_FLOOR_MS
}

/// Order `pool` most-demanding-first, **keeping incumbents in place unless
/// clearly beaten**.
///
/// `prev` is the previous frame's order, by window id, held by the caller —
/// hysteresis is state, and this function takes it as an argument rather than
/// hiding it in a `thread_local`, so every combination is a `make test` case.
///
/// The rule, applied repeatedly to what is left: the best-scoring candidate
/// takes the next slot **unless** the highest-ranked surviving incumbent is
/// still in the pool and the best does not [`clearly_beats`] it. An app `prev`
/// does not name is a newcomer and is ranked on its score alone — a window that
/// just opened is not somebody jostling for position.
///
/// Returns the **whole** ranked list, not just the slots that will be shown:
/// the caller carries all of it back, so an app that sits at rank four keeps
/// its incumbency and does not re-enter the bar as a newcomer every second.
pub fn rank_apps(prev: &[crate::window::WindowId], pool: Vec<AppReading>) -> Vec<AppReading> {
    let mut pool = pool;
    let mut out = Vec::with_capacity(pool.len());
    while !pool.is_empty() {
        let score = |a: &AppReading| rank_score(&a.samples);
        let best = (0..pool.len())
            .max_by(|&a, &b| {
                score(&pool[a])
                    .partial_cmp(&score(&pool[b]))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(0);
        // The incumbent with the best previous rank that is still in the pool.
        let incumbent = pool
            .iter()
            .enumerate()
            .filter_map(|(i, a)| prev.iter().position(|p| *p == a.window).map(|r| (r, i)))
            .min_by_key(|(r, _)| *r)
            .map(|(_, i)| i);
        let pick = match incumbent {
            Some(inc) if inc != best && !clearly_beats(score(&pool[best]), score(&pool[inc])) => inc,
            _ => best,
        };
        out.push(pool.remove(pick));
    }
    out
}

/// Everything the bar needs, already read. Pure input so the whole bar is a
/// native test: nothing here touches the sampler, the DOM or a clock.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Metrics {
    pub windows: usize,
    pub peers: usize,
    pub can_persist: bool,
    /// Held-past-a-frame milliseconds per second, oldest first. Empty when no
    /// sampling is running — which is a real state and **not** a row of zeros
    /// (a flat green line for a tab nobody measured is a lie).
    pub frozen: Vec<f64>,
    /// The part of `frozen` that was us drawing.
    pub frozen_drawing: Vec<f64>,
    /// Reporting apps, **most demanding first and already stabilised** by
    /// [`rank_apps`]. The whole ranked list; [`segments`] shows the first
    /// [`APP_SLOTS`] and the caller carries all of it into the next frame.
    pub apps: Vec<AppReading>,
}

impl Metrics {
    /// Read the bar's signals out of a sampler [`History`] plus the two counts
    /// the bar has always carried.
    ///
    /// Only windows whose report is current are eligible
    /// ([`crate::monitor::sampler::WindowHistory::app_reporting`]) — an app
    /// that has gone quiet must not keep its slot showing stale work. `prev` is
    /// the previous frame's order; see [`rank_apps`].
    pub fn from_history(
        h: &History,
        windows: usize,
        peers: usize,
        can_persist: bool,
        prev: &[crate::window::WindowId],
    ) -> Self {
        let pool: Vec<AppReading> = h
            .windows
            .iter()
            .filter(|w| w.app_reporting())
            .filter_map(|w| {
                let name = w.app.clone()?;
                w.app_busy.last()?;
                Some(AppReading {
                    window: w.id,
                    name,
                    samples: w.app_busy.tail(SPARK_CELLS * 2),
                })
            })
            .collect();
        Self {
            windows,
            peers,
            can_persist,
            frozen: h.frozen.tail(SPARK_CELLS * 2),
            frozen_drawing: h.frozen_drawing.tail(SPARK_CELLS * 2),
            apps: rank_apps(prev, pool),
        }
    }

    /// The order to carry into the next frame. See [`rank_apps`].
    pub fn app_order(&self) -> Vec<crate::window::WindowId> {
        self.apps.iter().map(|a| a.window).collect()
    }
}

/// ⭐ **What a held-time sparkline fills against — and it is NOT one second.**
///
/// Measured while testing: at a 1000 ms scale, four braille dot levels put the
/// `Good`/`Warn` boundary (50 ms) at *one fifth of the first dot*, so the whole
/// Good band and most of Warn drew as the baseline — **indistinguishable from
/// an idle tab.** The module doc's claim that the level is encoded twice, in
/// height *and* colour, was therefore false across exactly the range the bands
/// discriminate, which is the half that matters to a red-green colourblind
/// reader: they had the colour and nothing else.
///
/// Scaling to the `Warn`/`Bad` boundary instead makes the geometry *be* the
/// bands. `ceil` to four levels puts the Good ceiling at exactly one dot — the
/// baseline — so **good is a flat line and the first dot above it is the Warn
/// boundary, to the millisecond.** Derived from [`STALL_MS`] like the bands
/// themselves, so a retune moves all of it together (C15).
///
/// A tab held longer than this clips at full height. That is the right failure:
/// past the Bad boundary the question is no longer *how much*.
const HELD_SCALE_MS: f64 = STALL_MS * 4.0;

/// What an **app's own** busy time fills against. A different kind of number
/// from [`HELD_SCALE_MS`]: not a fault threshold but a share of the second, so
/// one second is genuinely the full scale — an app busy 900 ms of every second
/// is near its ceiling and should draw that way.
const APP_SCALE_MS: f64 = 1000.0;

/// A whole-tab gauge: held time, banded.
fn tab_gauge(mark: &'static str, samples: &[f64]) -> Gauge {
    Gauge {
        glyph: mark,
        spark: graph::braille_axis(samples, HELD_SCALE_MS, SPARK_CELLS, 1),
        level: level_for_samples(samples),
        caption: None,
        rows: 1,
    }
}

/// An app's own gauge: a share of the second, and [`Level::Neutral`] — see that
/// variant for why an app is not graded.
fn app_gauge(samples: &[f64], name: String) -> Gauge {
    Gauge {
        glyph: glyph::APP,
        spark: graph::braille_axis(samples, APP_SCALE_MS, SPARK_CELLS, 1),
        level: Level::Neutral,
        caption: Some(name),
        rows: 1,
    }
}

/// Build every segment the bar could show, widest-first. [`layout`] decides
/// which survive.
///
/// **The fixed group is built first and therefore sits left** — windows, peers,
/// durability, immediately after the product name, where the operator asked for
/// them because they are the three facts that do not move. The gauges fill what
/// is left and drop from the tail.
///
/// **A gauge with no samples is omitted, not drawn empty.** No sampling running
/// and a perfectly smooth tab are different facts, and a blank or zeroed gauge
/// renders the first as the second.
pub fn segments(m: &Metrics) -> Vec<Segment> {
    let mut out = Vec::new();

    // ── Fixed: the three facts the bar has always carried. ──────────────────
    let windows = crate::i18n::t_plural("window.count", m.windows as i64, &[("n", &m.windows.to_string())]);
    out.push(Segment {
        id: SegmentId::Windows,
        group: SegmentGroup::Fixed,
        priority: 5,
        body: SegmentBody::Icon { glyph: glyph::WINDOWS, text: m.windows.to_string() },
        label: windows,
        tooltip: None,
    });
    let peers = crate::i18n::t_plural("peer.count", m.peers as i64, &[("n", &m.peers.to_string())]);
    out.push(Segment {
        id: SegmentId::Peers,
        group: SegmentGroup::Fixed,
        priority: 5,
        body: SegmentBody::Icon { glyph: glyph::PEERS, text: m.peers.to_string() },
        label: peers,
        tooltip: None,
    });
    // ⚠ **The two durability cases are deliberately ASYMMETRIC.** A healthy
    // profile is a bare ✓ — the same instinct as dropping `0 ms` from a gauge:
    // a mark that only ever says "fine" does not need five characters to say
    // it, and the word is still in the label for a hover and a screen reader.
    // The NOT-saved case keeps its word at every width, because an icon that
    // renders a storage failure as a quiet mark is worse than the text it
    // replaced. Do not "tidy" these into one arm.
    let durability = crate::i18n::t(
        if m.can_persist { "statusbar.saved" } else { "statusbar.not_saved" },
        &[],
    );
    out.push(Segment {
        id: SegmentId::Durability,
        group: SegmentGroup::Fixed,
        priority: if m.can_persist { 5 } else { 0 },
        body: SegmentBody::Icon {
            glyph: if m.can_persist { glyph::SAVED } else { glyph::NOT_SAVED },
            text: if m.can_persist { String::new() } else { durability.clone() },
        },
        label: durability,
        tooltip: None,
    });

    // ── Live: the gauges, in drop order from the tail. ──────────────────────
    if !m.frozen.is_empty() {
        let last = m.frozen.last().copied().unwrap_or(0.0);
        out.push(Segment {
            id: SegmentId::Smoothness,
            group: SegmentGroup::Live,
            priority: 10,
            body: SegmentBody::Gauge(tab_gauge(glyph::SMOOTHNESS, &m.frozen)),
            label: crate::i18n::t("statusbar.smoothness", &[("ms", &format!("{last:.0}"))]),
            tooltip: Some(crate::i18n::t("statusbar.smoothness.tip", &[])),
        });
    }
    if !m.frozen_drawing.is_empty() {
        let last = m.frozen_drawing.last().copied().unwrap_or(0.0);
        out.push(Segment {
            id: SegmentId::Drawing,
            group: SegmentGroup::Live,
            priority: 20,
            body: SegmentBody::Gauge(tab_gauge(glyph::DRAWING, &m.frozen_drawing)),
            label: crate::i18n::t("statusbar.drawing", &[("ms", &format!("{last:.0}"))]),
            tooltip: Some(crate::i18n::t("statusbar.drawing.tip", &[])),
        });
    }
    // The apps drop last-first, one slot at a time: top-1 is a width tier of
    // top-3 rather than a second arrangement somebody has to maintain.
    for (slot, app) in AppSlot::ALL.iter().zip(m.apps.iter()) {
        if app.samples.is_empty() {
            continue;
        }
        let last = app.samples.last().copied().unwrap_or(0.0);
        out.push(Segment {
            id: SegmentId::App(*slot),
            group: SegmentGroup::Live,
            priority: 30 + slot.rank(),
            body: SegmentBody::Gauge(app_gauge(&app.samples, app.name.clone())),
            label: crate::i18n::t(
                "statusbar.top_app",
                &[("app", &app.name), ("ms", &format!("{last:.0}"))],
            ),
            tooltip: Some(crate::i18n::t("statusbar.top_app.tip", &[])),
        });
    }
    out
}

/// The bar's original phrase — **the whole bar's accessible name**, and the one
/// expression of this wording (`app::status_summary` delegates here).
///
/// ⚠ **It is NOT the narrowest tier, and the design said it was.** Measured
/// while testing: the phrase is ~32 cells and the three icons are ~19, so a
/// narrowing bar reaches the icons *first* and can never fall through to the
/// sentence. A "minimum tier" wider than the tier above it is unreachable by
/// construction — the tier list in `DESIGN-2026-09-16` §3 had it backwards, and
/// this is the correction.
///
/// So the phrase kept its job by changing it: it is the container's
/// `aria-label`, which is **better** than what it would have been as a tier. A
/// screen reader gets one sentence — *"2 windows · 1 peer · Saved"* — instead of
/// three isolated icon labels it has to reassemble, and that is true at every
/// width rather than only the narrowest one.
pub fn summary_text(windows: usize, peers: usize, can_persist: bool) -> String {
    let win = crate::i18n::t_plural("window.count", windows as i64, &[("n", &windows.to_string())]);
    let peer = crate::i18n::t_plural("peer.count", peers as i64, &[("n", &peers.to_string())]);
    let durability = crate::i18n::t(
        if can_persist { "statusbar.saved" } else { "statusbar.not_saved" },
        &[],
    );
    format!("{win} · {peer} · {durability}") // i18n-ignore — slot-only composition; parts localized above
}

/// Fit `all` into `cells` columns by dropping the highest `priority` first.
///
/// **A pure filter — it drops, it never substitutes and never truncates a
/// segment's own text.** There is no "fallback phrase" arm: see
/// [`summary_text`] for why the sentence cannot be a narrow tier. A bar too
/// narrow for even the durability mark renders nothing, which is honest; a
/// clipped reading is a wrong reading.
///
/// Ties keep source order, so the caller controls the left-to-right
/// arrangement and this function never reorders for its own convenience.
pub fn layout(all: &[Segment], cells: usize) -> Vec<Segment> {
    let mut kept: Vec<&Segment> = all.iter().collect();
    // Stable, so equal priorities stay in the order the caller built them.
    kept.sort_by_key(|s| s.priority);
    while !kept.is_empty() && kept.iter().map(|s| s.cells()).sum::<usize>() > cells {
        kept.pop();
    }
    // Restore the caller's order for the survivors.
    let survivors: std::collections::BTreeSet<SegmentId> = kept.iter().map(|s| s.id).collect();
    all.iter().filter(|s| survivors.contains(&s.id)).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::WindowId;

    fn metrics(windows: usize, peers: usize, persist: bool) -> Metrics {
        Metrics { windows, peers, can_persist: persist, ..Default::default() }
    }

    fn app(window: WindowId, name: &str, samples: &[f64]) -> AppReading {
        AppReading { window, name: name.into(), samples: samples.to_vec() }
    }

    fn with_gauges() -> Metrics {
        Metrics {
            windows: 2,
            peers: 1,
            can_persist: true,
            frozen: vec![10.0, 20.0, 30.0],
            frozen_drawing: vec![5.0, 5.0, 8.0],
            apps: vec![
                app(1, "Alpine", &[100.0, 200.0, 400.0]),
                app(2, "KolibriOS", &[50.0, 60.0, 70.0]),
                app(3, "Tiny Core", &[10.0, 12.0, 14.0]),
            ],
            ..Default::default()
        }
    }

    fn ids(v: &[Segment]) -> Vec<SegmentId> {
        v.iter().map(|s| s.id).collect()
    }

    /// The narrowest tier is the string that shipped, character for character.
    /// If this ever diverges the fallback has become a new invention.
    ///
    /// The counts route through `t_plural`, which **bidi-isolates** the `{n}`
    /// argument (U+2068 … U+2069) so a number renders correctly in an RTL
    /// locale. `app::status_summary_tests` asserts the same bytes from the
    /// other side of the delegation, which is what makes this a no-change port
    /// rather than a rewrite that happens to look similar.
    #[test]
    fn the_narrowest_tier_is_todays_exact_phrase() {
        assert_eq!(
            summary_text(2, 1, true),
            "\u{2068}2\u{2069} windows · \u{2068}1\u{2069} peer · Saved"
        );
        assert_eq!(
            summary_text(1, 1, false),
            "\u{2068}1\u{2069} window · \u{2068}1\u{2069} peer · Not saved"
        );
    }

    /// ⭐ **The operator's arrangement, asserted rather than left to the build
    /// order it happens to come from.** Windows · peers · durability sit
    /// immediately after the product name because they are the facts that do
    /// not move; every gauge is to their right. `layout` promises never to
    /// reorder, so this plus that promise is the whole guarantee.
    #[test]
    fn the_fixed_group_is_left_of_every_gauge_at_every_width() {
        let all = segments(&with_gauges());
        let total: usize = all.iter().map(|s| s.cells()).sum();
        // Every width from "nothing fits" to "everything fits": the boundary
        // must hold at each of them, not only at the three a test would pick.
        for cells in 0..=total {
            let shown = layout(&all, cells);
            let first_live = shown.iter().position(|s| s.group == SegmentGroup::Live);
            let last_fixed = shown.iter().rposition(|s| s.group == SegmentGroup::Fixed);
            if let (Some(l), Some(f)) = (first_live, last_fixed) {
                assert!(f < l, "a gauge came before a fixed segment at {cells} cells: {:?}", ids(&shown));
            }
        }
        // Anti-vacuity: the full-width row really does contain both groups, so
        // the loop above is not passing because one side is always empty.
        let full = layout(&all, total);
        assert!(full.iter().any(|s| s.group == SegmentGroup::Fixed));
        assert!(full.iter().any(|s| s.group == SegmentGroup::Live));
    }

    /// ⭐ **No numbers on the bar.** The braille is the reading; a `0 ms` beside
    /// it is the widest part of an idle segment. The figure stays reachable in
    /// the accessible label, which is what makes this a move rather than a
    /// loss — so this asserts BOTH halves, or a later "tidy" that drops the
    /// label passes the half that was easy.
    #[test]
    fn a_gauge_paints_no_number_and_still_says_the_number_to_a_screen_reader() {
        let all = segments(&with_gauges());
        let gauges: Vec<&Segment> = all
            .iter()
            .filter(|s| matches!(s.body, SegmentBody::Gauge(_)))
            .collect();
        assert_eq!(gauges.len(), 5, "two tab gauges and three apps: {:?}", ids(&all));
        for s in &gauges {
            let SegmentBody::Gauge(g) = &s.body else { unreachable!() };
            // A caption is an app NAME. Nothing painted may carry a digit.
            let painted = format!("{}{}", g.glyph, g.caption.clone().unwrap_or_default());
            assert!(
                !painted.chars().any(|c| c.is_ascii_digit()),
                "{:?} paints a number: {painted:?}",
                s.id
            );
            assert!(
                s.label.chars().any(|c| c.is_ascii_digit()),
                "{:?} dropped the reading instead of moving it: {:?}",
                s.id,
                s.label
            );
        }
    }

    /// Wide keeps everything; the apps go first from the tail, then drawing,
    /// then smoothness. Asserted as a LADDER rather than at three separate
    /// widths, so a priority that stops being monotone fails here instead of at
    /// whichever single width a test happened to pick.
    #[test]
    fn segments_drop_by_priority_as_the_bar_narrows() {
        let m = with_gauges();
        let all = segments(&m);
        let total: usize = all.iter().map(|s| s.cells()).sum();
        let cells_of = |id: SegmentId| all.iter().find(|s| s.id == id).unwrap().cells();

        let wide = layout(&all, total);
        assert_eq!(ids(&wide).len(), all.len(), "everything fits at full width");

        // The apps leave one at a time, third first.
        let mut budget = total;
        for slot in [AppSlot::Third, AppSlot::Second, AppSlot::First] {
            budget -= cells_of(SegmentId::App(slot));
            let out = layout(&all, budget);
            assert!(
                !ids(&out).contains(&SegmentId::App(slot)),
                "{slot:?} should have dropped at {budget} cells: {:?}",
                ids(&out)
            );
        }
        assert!(ids(&layout(&all, budget)).contains(&SegmentId::Drawing), "drawing outlives the apps");

        budget -= cells_of(SegmentId::Drawing);
        let narrow = layout(&all, budget);
        assert!(!ids(&narrow).contains(&SegmentId::Drawing), "drawing drops after the apps");
        assert!(ids(&narrow).contains(&SegmentId::Smoothness), "smoothness is the last gauge");
    }

    /// The counts outrank every gauge: a bar too narrow for a sparkline still
    /// answers the question the bar answered before any of this existed.
    #[test]
    fn the_counts_outrank_every_gauge() {
        let m = with_gauges();
        let all = segments(&m);
        let icons: usize = all
            .iter()
            .filter(|s| matches!(s.body, SegmentBody::Icon { .. }))
            .map(|s| s.cells())
            .sum();
        let out = layout(&all, icons);
        assert_eq!(
            ids(&out),
            vec![SegmentId::Windows, SegmentId::Peers, SegmentId::Durability],
            "the three counts survive alone"
        );
    }

    /// ⚠ **The phrase is WIDER than the icons, so it cannot be a narrow tier.**
    /// `DESIGN-2026-09-16` §3 listed it as the minimum, below icons-only; at
    /// that width the icons already fit and the sentence is never reached. Pinned
    /// as an inequality rather than as two numbers, because the *relation* is
    /// the finding and the cell counts will move as wording changes.
    #[test]
    fn the_shipped_phrase_is_wider_than_the_icons_it_would_have_replaced() {
        let m = metrics(2, 1, true);
        let icons: usize = segments(&m).iter().map(|s| s.cells()).sum();
        let phrase = summary_text(2, 1, true).chars().count();
        assert!(
            phrase > icons,
            "the phrase ({phrase} cells) must be wider than the icons ({icons}) — if this \
             ever inverts, a phrase tier becomes reachable and the design note is stale"
        );
    }

    /// Too narrow for even the durability mark: nothing, rather than a clipped
    /// reading. A truncated number is a wrong number.
    #[test]
    fn a_bar_too_narrow_for_anything_shows_nothing_rather_than_a_clipped_reading() {
        assert!(layout(&segments(&metrics(2, 1, true)), 3).is_empty());
    }

    /// ⭐ **The warning keeps its word; the healthy mark does not.** An icon
    /// alone for a storage FAILURE is quieter than the text it replaced — but a
    /// mark that only ever says "fine" does not need five characters, and the
    /// word is still in its label. Asserted as the PAIR, because either half
    /// alone reads as an inconsistency somebody would tidy away.
    ///
    /// ⚠ **This test was LOST and restored.** Its ancestor
    /// (`a_profile_that_cannot_persist_keeps_the_words_and_outranks_everything`)
    /// did not survive a whole-module rewrite of this `mod tests`, and nothing
    /// said so: `make test` went **up**, because the rewrite added more than it
    /// dropped. *A rising test count hides a deletion* — the repo's own rule is
    /// that a count which did not move is a claim, and the mirror is that a
    /// count which moved by the wrong amount is one too. Check the delta
    /// against what you expected, not against the previous number.
    #[test]
    fn the_storage_warning_keeps_its_word_and_the_healthy_mark_does_not() {
        let bad = segments(&metrics(2, 1, false));
        let d = bad.iter().find(|s| s.id == SegmentId::Durability).unwrap();
        assert_eq!(d.priority, 0, "the warning is the last thing to drop");
        match &d.body {
            SegmentBody::Icon { glyph, text } => {
                assert_eq!(*glyph, glyph::NOT_SAVED);
                assert_eq!(text, "Not saved", "the word survives, not just a mark");
            }
            other => panic!("expected an icon with its word, got {other:?}"),
        }

        let good = segments(&metrics(2, 1, true));
        let d = good.iter().find(|s| s.id == SegmentId::Durability).unwrap();
        match &d.body {
            SegmentBody::Icon { glyph, text } => {
                assert_eq!(*glyph, glyph::SAVED);
                assert!(text.is_empty(), "a healthy profile is a bare mark, got {text:?}");
            }
            other => panic!("expected an icon, got {other:?}"),
        }
        assert_eq!(d.label, "Saved", "…and the word is still there for a reader");
    }

    /// The durability warning is the single last thing standing.
    #[test]
    fn the_last_segment_to_survive_is_the_storage_warning() {
        let m = metrics(2, 1, false);
        let all = segments(&m);
        let warn = all.iter().find(|s| s.id == SegmentId::Durability).unwrap();
        assert_eq!(ids(&layout(&all, warn.cells())), vec![SegmentId::Durability]);
    }

    /// **No sampling and a perfectly smooth tab are different facts.** A gauge
    /// with no history is omitted; a zeroed one would claim a measurement. This
    /// is also the off-switch: with nothing holding the sampler the history is
    /// empty and the bar is exactly what it was before this feature.
    #[test]
    fn a_gauge_with_no_samples_is_omitted_rather_than_drawn_empty() {
        let out = segments(&metrics(2, 1, true));
        assert_eq!(ids(&out), vec![SegmentId::Windows, SegmentId::Peers, SegmentId::Durability]);
    }

    /// The `Good` boundary is `STALL_MS`, derived rather than typed — so a
    /// retune of the sampler's stall threshold moves the gauge with it.
    #[test]
    fn the_good_band_ends_at_one_stalls_worth_of_held_time() {
        assert_eq!(level_for(0.0), Level::Good);
        assert_eq!(level_for(STALL_MS), Level::Good, "exactly one stall is still good");
        assert_eq!(level_for(STALL_MS + 1.0), Level::Warn);
        assert_eq!(level_for(STALL_MS * 4.0), Level::Warn);
        assert_eq!(level_for(STALL_MS * 4.0 + 1.0), Level::Bad);
        assert_eq!(level_for(f64::NAN), Level::Good, "an unreadable sample is not an alarm");
    }

    /// ⭐ **The colour is smoothed and the sparkline is not.** One bad second
    /// must not repaint the segment, and the raw sample is still drawn — so the
    /// reading is never hidden, only the emphasis is damped. Both halves, or a
    /// smoothing that quietly damped the spark too would pass.
    #[test]
    fn one_bad_second_does_not_repaint_the_segment_but_is_still_drawn() {
        // Just over the instantaneous boundary, once, in an idle window.
        let over = STALL_MS + 2.0;
        let mut calm = vec![0.0; LEVEL_WINDOW * 4];
        calm.push(over);
        // Anti-vacuity: the fixture's LAST sample really is over the raw
        // boundary, so the assertion below is about the smoothing and not
        // about a sample that was never going to trip anything.
        assert_eq!(level_for(over), Level::Warn);
        assert_eq!(
            level_for_samples(&calm),
            Level::Good,
            "one second over the line does not repaint the segment"
        );
        assert_eq!(
            level_for_samples(&vec![over; LEVEL_WINDOW]),
            Level::Warn,
            "a sustained one does — hysteresis that never yields is a gauge that stopped reporting"
        );
        // ⚠ Stated bound: the smoothing ATTENUATES, it does not suppress. A big
        // enough single excursion still reaches a band, because a 400 ms freeze
        // is worth an amber mark however calm the seconds around it were.
        let mut spike = vec![0.0; LEVEL_WINDOW * 4];
        spike.push(STALL_MS * 8.0);
        assert_ne!(level_for_samples(&spike), Level::Good);
        // …and the excursion is visible: the spark is built from every sample,
        // so its tallest cell differs from a flat run's. **This is what caught
        // the scale defect** — at a 1000 ms scale these two were byte-identical,
        // i.e. the height said nothing across the whole Good/low-Warn range.
        let spark = graph::braille_axis(&calm, HELD_SCALE_MS, SPARK_CELLS, 1);
        let flat = graph::braille_axis(&vec![0.0; calm.len()], HELD_SCALE_MS, SPARK_CELLS, 1);
        assert_ne!(spark, flat, "the smoothing must not reach the drawing");
        assert_eq!(level_for_samples(&[]), Level::Good, "no samples is not an alarm");
    }

    /// ⭐ **The geometry IS the bands.** Scaling a held-time spark to the
    /// Warn/Bad boundary puts the Good ceiling at exactly one dot — the
    /// baseline — so a good tab draws a flat line and the first dot above it is
    /// the Warn boundary to the millisecond. That is what makes the level
    /// double-encoded (height *and* colour) rather than colour-only, which is
    /// the half a red-green colourblind reader depends on.
    ///
    /// Asserted from **both sides of the boundary**, because a scale that
    /// happened to draw everything tall would pass a one-sided check.
    #[test]
    fn a_good_tab_draws_a_flat_line_and_the_first_dot_above_it_is_the_warn_boundary() {
        let spark = |v: f64| graph::braille_axis(&vec![v; SPARK_CELLS * 2], HELD_SCALE_MS, SPARK_CELLS, 1);
        let idle = spark(0.0);
        assert_eq!(spark(STALL_MS), idle, "the top of Good is still the flat line");
        assert_ne!(spark(STALL_MS + 1.0), idle, "the first millisecond of Warn lifts off it");
        // …and the Bad boundary is the top, so past it the height stops being
        // the question rather than growing forever.
        assert_eq!(
            spark(STALL_MS * 4.0),
            spark(STALL_MS * 40.0),
            "past the Bad boundary the gauge is pinned, not scaled"
        );
    }

    /// An app is not graded: `Neutral`, at every level of work. Both ends, so a
    /// band that crept back in at one of them is caught.
    #[test]
    fn an_app_is_never_coloured_by_the_held_time_bands() {
        for busy in [0.0, 400.0, 999.0] {
            let m = Metrics { apps: vec![app(1, "VM", &[busy; 8])], ..Default::default() };
            let seg = segments(&m).into_iter().find(|s| matches!(s.id, SegmentId::App(_))).unwrap();
            let SegmentBody::Gauge(g) = seg.body else { panic!("expected a gauge") };
            assert_eq!(g.level, Level::Neutral, "an app busy {busy} ms/s was graded");
        }
        // Anti-vacuity: the same numbers really do reach three different bands
        // when they ARE a held-time reading, so `Neutral` is a decision rather
        // than the only answer this fixture could have produced.
        assert_eq!(level_for_samples(&[0.0; 8]), Level::Good);
        assert_eq!(level_for_samples(&[150.0; 8]), Level::Warn);
        assert_eq!(level_for_samples(&[999.0; 8]), Level::Bad);
    }

    /// Every segment is readable by a screen reader. The braille is
    /// `aria-hidden` at the renderer, so an empty label is a segment that
    /// cannot be read at all.
    #[test]
    fn every_segment_carries_words_for_a_screen_reader() {
        let all = segments(&with_gauges());
        assert!(all.len() >= 8, "the gauge population is not vacuous: {}", all.len());
        for s in &all {
            assert!(!s.label.trim().is_empty(), "{:?} has no label", s.id);
        }
    }

    /// The px→cells estimate saturates and is roughly right. **Lives here, not
    /// beside the renderer** — `mod dom` is wasm-only, so the first version of
    /// this test compiled on no target and ran on none.
    #[test]
    fn cells_available_is_saturating_and_roughly_right() {
        assert_eq!(cells_available(750.0), 100);
        assert_eq!(cells_available(0.0), 0, "a bar mid-layout is zero, not a panic");
        assert_eq!(cells_available(-10.0), 0);
        assert_eq!(cells_available(f64::NAN), 0);
    }

    /// Every segment id has its own selector key — a duplicate would make a
    /// browser gate silently assert about the wrong segment. `AppSlot` is a
    /// closed enum precisely so this match has no fallback arm to collide in.
    #[test]
    fn every_segment_id_has_a_distinct_selector_key() {
        let segs = segments(&with_gauges());
        let keys: Vec<&str> = segs.iter().map(|s| s.selector_key()).collect();
        let unique: std::collections::BTreeSet<&&str> = keys.iter().collect();
        assert_eq!(keys.len(), unique.len(), "duplicate selector key in {keys:?}");
        // Anti-vacuity: this population really does contain every kind, so the
        // uniqueness claim is about all eight and not about the three that a
        // gauge-less fixture would have produced.
        assert_eq!(keys.len(), 3 + 2 + APP_SLOTS, "expected every segment kind, got {keys:?}");
        assert_eq!(segs.iter().map(|s| s.id).collect::<std::collections::BTreeSet<_>>().len(), keys.len());
    }

    /// The bar shows at most [`APP_SLOTS`] apps however many report.
    #[test]
    fn a_fourth_reporting_app_does_not_get_a_segment() {
        let m = Metrics {
            apps: (1..=6).map(|i| app(i, &format!("App {i}"), &[100.0])).collect(),
            ..with_gauges()
        };
        let app_segs = segments(&m).iter().filter(|s| matches!(s.id, SegmentId::App(_))).count();
        assert_eq!(app_segs, APP_SLOTS);
    }

    // ── Ranking and hysteresis ──────────────────────────────────────────────

    fn order(v: &[AppReading]) -> Vec<&str> {
        v.iter().map(|a| a.name.as_str()).collect()
    }

    /// With nobody incumbent, the busiest app leads.
    #[test]
    fn a_first_ranking_is_simply_busiest_first() {
        let out = rank_apps(&[], vec![app(1, "quiet", &[10.0]), app(2, "loud", &[900.0])]);
        assert_eq!(order(&out), vec!["loud", "quiet"]);
    }

    /// ⭐ **The operator's complaint, as a test: two apps a few milliseconds
    /// apart must not trade places.** Ranked, then handed a frame where the
    /// challenger is genuinely ahead but not by the margin — the order holds.
    #[test]
    fn a_narrow_win_does_not_take_the_incumbents_slot() {
        let first = rank_apps(&[], vec![app(1, "a", &[500.0]), app(2, "b", &[400.0])]);
        assert_eq!(order(&first), vec!["a", "b"]);
        // b is now ahead — by 2 ms. Without hysteresis this flips every second.
        let second = rank_apps(
            &first.iter().map(|x| x.window).collect::<Vec<_>>(),
            vec![app(1, "a", &[500.0]), app(2, "b", &[502.0])],
        );
        assert_eq!(order(&second), vec!["a", "b"], "a 2 ms lead is not a new leader");
    }

    /// …and a real winner does take it, or the bar would be frozen rather than
    /// stable. Both directions, because hysteresis that never yields is a
    /// gauge that stopped reporting.
    #[test]
    fn a_clear_win_does_take_the_incumbents_slot() {
        let prev = vec![1u64, 2];
        let out = rank_apps(&prev, vec![app(1, "a", &[100.0]), app(2, "b", &[900.0])]);
        assert_eq!(order(&out), vec!["b", "a"]);
    }

    /// ⭐ **The absolute floor is why the ratio is not enough on its own.** 1 ms
    /// against 2 ms is a 2× win between two apps that are both doing nothing,
    /// and a ratio-only rule flops hardest exactly where nothing is happening.
    #[test]
    fn two_idle_apps_do_not_swap_on_a_ratio_neither_of_them_earned() {
        let prev = vec![1u64, 2];
        let out = rank_apps(&prev, vec![app(1, "a", &[1.0]), app(2, "b", &[2.5])]);
        assert_eq!(
            order(&out),
            vec!["a", "b"],
            "2.5x of nothing is still nothing — DISPLACE_FLOOR_MS is what refuses it"
        );
    }

    /// Ranking is on a trailing mean, so one busy second does not reorder the
    /// bar. Constructed so the LAST sample alone would invert the answer —
    /// otherwise the test passes for an implementation that never smoothed.
    #[test]
    fn one_busy_second_does_not_reorder_the_bar() {
        let steady = app(1, "steady", &[400.0; RANK_WINDOW]);
        let spiky = app(2, "spiky", &[0.0, 0.0, 0.0, 0.0, 900.0]);
        assert!(
            spiky.samples.last() > steady.samples.last(),
            "the fixture must invert on the last sample or it measures nothing"
        );
        assert_eq!(order(&rank_apps(&[], vec![steady, spiky])), vec!["steady", "spiky"]);
    }

    /// A window that just opened is not somebody jostling for position: a
    /// newcomer is ranked on its score, with no margin to clear.
    #[test]
    fn a_newcomer_is_ranked_on_its_score_and_not_held_behind_an_incumbent() {
        let out = rank_apps(&[1], vec![app(1, "old", &[100.0]), app(2, "new", &[900.0])]);
        assert_eq!(order(&out), vec!["new", "old"]);
    }

    /// Two windows running the same app are two rows, told apart by window id.
    /// Keying incumbency on the name would make this case unrepresentable.
    #[test]
    fn two_windows_of_one_app_keep_their_own_slots() {
        let prev = vec![7u64, 4];
        let out = rank_apps(&prev, vec![app(4, "Alpine", &[500.0]), app(7, "Alpine", &[480.0])]);
        assert_eq!(
            out.iter().map(|a| a.window).collect::<Vec<_>>(),
            vec![7, 4],
            "the incumbent is window 7, not 'the one called Alpine'"
        );
    }

    /// The order handed back covers every candidate, not only the shown slots —
    /// so an app sitting at rank four keeps its incumbency instead of
    /// re-entering as a newcomer every second.
    #[test]
    fn the_carried_order_is_the_whole_ranking_not_just_the_visible_slots() {
        let m = Metrics {
            apps: rank_apps(&[], (1..=5).map(|i| app(i, &format!("App {i}"), &[100.0])).collect()),
            ..Default::default()
        };
        assert_eq!(m.app_order().len(), 5);
        assert!(m.app_order().len() > APP_SLOTS, "the fixture must exceed the slots");
    }

    /// An app that has gone quiet loses its slot rather than showing stale
    /// work, and the ranking is read out of a real `History`.
    #[test]
    fn a_quiet_app_loses_its_slot() {
        use crate::monitor::sampler::WindowHistory;
        let ring = |vals: &[f64]| {
            let mut r = graph::Ring::default();
            for v in vals {
                r.push(*v);
            }
            r
        };
        let loud = WindowHistory {
            id: 1,
            app: Some("Alpine".into()),
            // Sustained, not a spike: ranking is on a trailing mean, so a
            // fixture that leads only on its last sample measures the wrong
            // thing here (this test is about REPORTING, not about ordering).
            app_busy: ring(&[880.0, 900.0]),
            app_quiet_s: Some(0),
            ..Default::default()
        };
        let quiet = WindowHistory {
            id: 2,
            app: Some("Chess".into()),
            app_busy: ring(&[6.0, 5.0]),
            app_quiet_s: Some(0),
            ..Default::default()
        };
        let h = History { windows: vec![quiet.clone(), loud.clone()], ..Default::default() };
        let m = Metrics::from_history(&h, 2, 1, true, &[]);
        assert_eq!(order(&m.apps), vec!["Alpine", "Chess"]);

        // The same two windows, neither reporting: no app segment at all rather
        // than one filled with the work they last did.
        let h2 = History {
            windows: h.windows.iter().map(|w| WindowHistory { app_quiet_s: None, ..w.clone() }).collect(),
            ..Default::default()
        };
        assert!(Metrics::from_history(&h2, 2, 1, true, &[]).apps.is_empty());
    }
}
