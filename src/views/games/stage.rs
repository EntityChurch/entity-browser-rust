//! The player stage's two size affordances, as rules rather than as DOM.
//!
//! A running app has **three** ways to get bigger, and until now two of them
//! looked like the same control:
//!
//! 1. **Maximize** (`▢`, the window header) — the *window* covers the viewport.
//!    Not ours; it belongs to every window and lives in `dom::mod`.
//! 2. **Expand** (`⤢`, the player bar) — the *stage* stops being a capped,
//!    centered box and fills whatever window it is in.
//! 3. **Full screen** (`⛶`, the player bar) — the player leaves the page
//!    entirely and fills the physical screen.
//!
//! The first two together still leave two bars of chrome above the app (the
//! window header and the player bar) plus the browser's own, which on a laptop
//! is most of the reason a game feels cramped. (3) is what removes them.
//!
//! **Full screen implies expanded, and that is not a convenience.** A stage
//! still capped at its 680px square, centered on a 27-inch screen, reads as a
//! broken transition rather than as a deliberate size — so entering forces the
//! stage full-bleed. The window-level expand state is *remembered* and restored
//! on the way out, because the trip through full screen is not a decision about
//! how the app should look inside its window.
//!
//! While full screen, **Expand is not offered at all**. It can only be "on", so
//! rendering it is chrome claiming to be a control — the same rule that keeps a
//! single surviving category chip off the launcher.
//!
//! Kept here, native-side, because these are the parts that can be *wrong*:
//! which label a button carries and whether it is offered. Whether the engine
//! actually grants the request is a DOM fact no native test can see, and the
//! caller observes it via `fullscreenchange` rather than by assuming its own
//! click worked.

/// The player bar's size controls for one state of the world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageChrome {
    /// Is the stage full-bleed — filling its container rather than capped and
    /// centered? (The `gm-expanded` class.)
    pub expanded: bool,
    /// Render the Expand/Collapse button at all?
    pub expand_offered: bool,
    /// i18n key for the Expand/Collapse label. Meaningless when
    /// `expand_offered` is false.
    pub expand_label_key: &'static str,
    /// i18n key for the Expand/Collapse `title`.
    pub expand_title_key: &'static str,
    /// i18n key for the full-screen button's label.
    pub full_label_key: &'static str,
    /// i18n key for the full-screen button's `title`.
    pub full_title_key: &'static str,
}

/// Resolve the player bar's controls.
///
/// `fullscreen` is what the engine reports **now** (via `:fullscreen`), never
/// what we asked for — a refused request and an Esc both have to land here.
/// `expanded_in_window` is the state the stage should return to once full
/// screen ends; it is the user's standing answer for the windowed case.
pub fn stage_chrome(fullscreen: bool, expanded_in_window: bool) -> StageChrome {
    let expanded = fullscreen || expanded_in_window;
    StageChrome {
        expanded,
        expand_offered: !fullscreen,
        expand_label_key: if expanded_in_window { "btn.collapse" } else { "btn.expand" },
        expand_title_key: if expanded_in_window {
            "tooltip.restore_size"
        } else {
            "tooltip.fill_window"
        },
        full_label_key: if fullscreen { "btn.exit_full_screen" } else { "btn.full_screen" },
        full_title_key: if fullscreen {
            "tooltip.leave_full_screen"
        } else {
            "tooltip.fill_screen"
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_screen_forces_the_stage_full_bleed_whatever_the_window_state_was() {
        // The reason this is a rule and not a nicety: a 680px-capped stage
        // centered on a full screen looks like the transition failed.
        assert!(stage_chrome(true, false).expanded);
        assert!(stage_chrome(true, true).expanded);
    }

    #[test]
    fn leaving_full_screen_returns_to_the_window_size_the_user_chose() {
        // The trip through full screen must not silently re-answer "how big
        // should this be inside its window".
        assert!(!stage_chrome(false, false).expanded, "a collapsed stage stays collapsed");
        assert!(stage_chrome(false, true).expanded, "an expanded stage stays expanded");
    }

    #[test]
    fn expand_is_not_offered_while_full_screen() {
        // It could only ever be "on" there — a button with one reachable state
        // is chrome pretending to be a control.
        assert!(!stage_chrome(true, false).expand_offered);
        assert!(!stage_chrome(true, true).expand_offered);
        assert!(stage_chrome(false, false).expand_offered);
        assert!(stage_chrome(false, true).expand_offered);
    }

    #[test]
    fn the_expand_label_tracks_the_window_state_not_the_screen_state() {
        // While full screen the stage IS expanded, but the button (when it
        // returns) must offer the way back to what the window looked like —
        // otherwise exiting full screen leaves a button labelled "Collapse"
        // over a stage that is not expanded.
        assert_eq!(stage_chrome(true, false).expand_label_key, "btn.expand");
        assert_eq!(stage_chrome(true, true).expand_label_key, "btn.collapse");
    }

    #[test]
    fn the_full_screen_button_names_the_way_out_once_we_are_in() {
        let out = stage_chrome(true, false);
        assert_eq!(out.full_label_key, "btn.exit_full_screen");
        assert_eq!(out.full_title_key, "tooltip.leave_full_screen");
        let inn = stage_chrome(false, false);
        assert_eq!(inn.full_label_key, "btn.full_screen");
        assert_eq!(inn.full_title_key, "tooltip.fill_screen");
    }

    #[test]
    fn every_key_this_module_names_exists_in_the_en_catalog() {
        // A label key with no catalog entry renders as the raw key — visible,
        // ugly, and exactly the kind of thing that ships because the surface it
        // is on is behind a click. Property over the whole reachable set, not a
        // spot check of the two states someone happened to look at.
        let keys: Vec<&'static str> = [(true, true), (true, false), (false, true), (false, false)]
            .iter()
            .flat_map(|&(fs, ex)| {
                let c = stage_chrome(fs, ex);
                [c.expand_label_key, c.expand_title_key, c.full_label_key, c.full_title_key]
            })
            .collect();
        for key in keys {
            assert!(
                crate::i18n::EN.iter().any(|(k, _)| *k == key),
                "stage chrome names `{key}`, which the EN catalog does not carry"
            );
        }
    }
}
