//! Rendering the status bar's segments into `#mode-display`.
//!
//! **This file is the DOM and nothing else.** The model, the px→cells estimate
//! and the selector keys all live in [`crate::status_bar`], which is native —
//! `mod dom` is `#[cfg(target_arch = "wasm32")]`, so anything with a decision in
//! it that sits here is a decision `make test` cannot see. The first cut of this
//! file had `cells_available` and `seg_key` in it with two tests that never
//! compiled; they moved rather than being deleted.
//!
//! What is left is genuinely untestable natively: element creation, `aria`
//! attributes and append order. `make e2e-worker T=the_status_bar` is what
//! measures it.

use crate::dom::util;
use crate::status_bar::{Segment, SegmentBody, SegmentGroup};

/// Render `segments` into the bar, replacing whatever was there.
///
/// `summary` is the bar's accessible name — one sentence for a screen reader
/// rather than the segment labels read in a row (see
/// [`crate::status_bar::summary_text`]).
#[cfg(target_arch = "wasm32")]
pub fn render(segments: &[Segment], summary: &str) {
    let Some(host) = util::get_element_by_id("mode-display") else { return };
    host.set_inner_html("");
    // The braille is aria-hidden per segment, so the bar as a whole needs a
    // name of its own or a screen reader gets the icon labels with no frame.
    util::set_attr(&host, "aria-label", summary);
    util::set_attr(&host, "role", "status");

    let mut last_group: Option<SegmentGroup> = None;
    for seg in segments {
        // One divider where the fixed facts stop and the live gauges start, so
        // the bar reads as two groups rather than one run of marks. The MODEL
        // decides where the boundary is (`Segment::group`); this only draws it.
        if last_group.is_some_and(|g| g != seg.group) {
            let div = util::create_element_with_class("span", "status-div");
            util::set_attr(&div, "aria-hidden", "true");
            let _ = host.append_child(&div);
        }
        last_group = Some(seg.group);

        let el = util::create_element_with_class("span", "status-seg");
        if let Some(tip) = &seg.tooltip {
            util::set_attr(&el, "title", tip);
        }
        // Stable id on the element so a browser gate can select a segment by
        // what it IS, never by its position in the row.
        util::set_attr(&el, "data-seg", seg.selector_key());

        match &seg.body {
            SegmentBody::Gauge(g) => {
                // The glyph says what this measures; the app's caption (if any)
                // says whose it is. Neither is ever the ONLY thing saying so —
                // `seg.label` carries the words, including the number the bar
                // deliberately no longer paints.
                let mark = util::create_element_with_class("span", "status-glyph");
                util::set_attr(&mark, "aria-hidden", "true");
                util::set_text(&mark, g.glyph);
                let _ = el.append_child(&mark);

                let spark = util::create_element_with_class("span", "status-spark");
                util::set_attr(&spark, "class", &format!("status-spark {}", g.level.class()));
                // ⚠ A screen reader reads U+2800–U+28FF as braille cells, so the
                // glyphs are hidden and `seg.label` carries the reading in words.
                // Dropping this attribute does not break the display, which is
                // exactly why it is easy to lose — the gate asserts it.
                util::set_attr(&spark, "aria-hidden", "true");
                util::set_text(&spark, &g.spark);
                let _ = el.append_child(&spark);

                if let Some(caption) = &g.caption {
                    let words = util::create_element_with_class("span", "status-seg-label");
                    util::set_text(&words, caption);
                    let _ = el.append_child(&words);
                }
            }
            SegmentBody::Icon { glyph, text } => {
                let mark = util::create_element_with_class("span", "status-glyph");
                // The glyph is decoration: `seg.label` already says "2 windows".
                util::set_attr(&mark, "aria-hidden", "true");
                util::set_text(&mark, glyph);
                let _ = el.append_child(&mark);

                let words = util::create_element_with_class("span", "status-seg-label");
                util::set_text(&words, text);
                let _ = el.append_child(&words);
            }
            SegmentBody::Text(t) => util::set_text(&el, t),
        }
        util::set_attr(&el, "aria-label", &seg.label);
        let _ = host.append_child(&el);
    }
}
