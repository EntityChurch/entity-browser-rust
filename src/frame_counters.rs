//! Phase 0c frame-time measurement instrumentation.
//!
//! Counts L0 SDK calls (`get_entity`, `tree_listing`) per frame. Logs to
//! console at end-of-frame when counts exceed a threshold. Disabled by
//! default; enable with `--features measurement`.
//!
//! Used to validate the snapshot-cache assumption (push-based per-prefix
//! mirror) against real per-frame call counts before Phase 1 freezes the
//! cache API.

use std::cell::Cell;

thread_local! {
    static GET_ENTITY: Cell<u32> = const { Cell::new(0) };
    static TREE_LISTING: Cell<u32> = const { Cell::new(0) };
    static FRAME_NO: Cell<u32> = const { Cell::new(0) };
}

/// Log threshold: only emit when a frame has at least this many L0 calls.
/// Keeps console noise down on idle frames; surfaces the interesting ones.
const LOG_THRESHOLD: u32 = 1;

#[inline]
pub fn bump_get_entity() {
    GET_ENTITY.with(|c| c.set(c.get() + 1));
}

#[inline]
pub fn bump_tree_listing() {
    TREE_LISTING.with(|c| c.set(c.get() + 1));
}

/// Snapshot of the running per-frame counters. Pair with [`diff_since`] to
/// attribute work to a specific render call (e.g. a single window's
/// `render_dom`) without resetting the per-frame totals.
///
/// Returns `(get_entity, tree_listing)` running totals for the current
/// frame. Cheap (two thread-local reads).
pub fn snapshot() -> (u32, u32) {
    (
        GET_ENTITY.with(|c| c.get()),
        TREE_LISTING.with(|c| c.get()),
    )
}

/// Delta from a prior [`snapshot`]. Returns `(d_get_entity, d_tree_listing)`.
pub fn diff_since(prev: (u32, u32)) -> (u32, u32) {
    let now = snapshot();
    (now.0.saturating_sub(prev.0), now.1.saturating_sub(prev.1))
}

pub fn frame_start() {
    GET_ENTITY.with(|c| c.set(0));
    TREE_LISTING.with(|c| c.set(0));
    FRAME_NO.with(|c| c.set(c.get() + 1));
}

pub fn frame_end_and_log(elapsed_ms: f64) {
    let g = GET_ENTITY.with(|c| c.get());
    let t = TREE_LISTING.with(|c| c.get());
    let n = FRAME_NO.with(|c| c.get());

    if g + t >= LOG_THRESHOLD {
        let rtt_small = (g + t) as f64 * 2.0;
        let rtt_large = (g + t) as f64 * 5.0;
        tracing::info!(
            frame = n,
            get_entity = g,
            tree_listing = t,
            elapsed_ms = format!("{:.1}", elapsed_ms),
            hypothetical_rtt_2ms = format!("{:.1}ms", rtt_small),
            hypothetical_rtt_5ms = format!("{:.1}ms", rtt_large),
            "frame counters"
        );
    }
}
