//! How tall a window is — the person's choice, remembered, and the one size an
//! app can ask for: *fit my screen*.
//!
//! Windows stack in a scrolling column and used to take whatever height their
//! content happened to have. For a VM that is wrong in both directions
//! (BACKLOG B-5): the Apps player floors itself at 560 px, so a 1024×768
//! KolibriOS desktop in a wide window drew at ~0.65× with black bars either
//! side, *Expand* could only fill that same short window, and full screen was
//! the one mode that fit — after which the window was back to the height it
//! had before, with nothing a person could do about it.
//!
//! ## Three sources, one applier
//!
//! A window's height has three possible sources, in this order:
//!
//! 1. **live** — a drag in progress on the grip. Runtime only; never persisted.
//! 2. **the stored preference** for the window's [`size key`](size_key):
//!    [`SizePref::Height`] (the person dragged it) or [`SizePref::Fit`] (they
//!    asked for the app's screen to fit).
//! 3. **the app's fit** — a height the player computed from what the app said
//!    about its own screen (`x-view`). With no stored preference, an app that
//!    reports a screen is fitted by default: that is the answer to the report.
//!
//! and **one applier**: the DOM renderer's per-frame reconcile, which reads
//! [`resolve`] and writes `style.height`. The grip and the player never touch
//! the section's height themselves — two writers of one value is the drift
//! shape, and here it would also race the reconcile frame by frame.
//!
//! **A height change is never a rebuild.** The reconcile writes a style; it
//! never marks a window dirty, because rebuilding the Apps window's section
//! replaces the iframe and restarts the machine running in it (the same reason
//! maximize is a class flip).
//!
//! ## What is remembered, and where
//!
//! One entity, `app/state/window-sizes`, at
//! `app/entity-browser/settings/window-sizes` on the primary peer: a map from
//! size key to `{mode, px}`. A size is a property of the **profile**, not of a
//! window slot — two KolibriOS windows should agree, and closing one must not
//! forget it — so it is keyed by what the window *is*, never by its id (AP42's
//! reused-slot hazard does not arise by construction).
//!
//! The registry here is a projection of that entity, loaded once at boot and
//! written through on every change ([`crate::app::EntityApp`] owns both halves,
//! because both need `Peers`). A load that could not answer leaves the defaults
//! in place and **writes nothing**: a missing read is not permission to
//! overwrite the person's sizes with an empty map (AP30 corollary (a)).

use std::cell::RefCell;
use std::collections::BTreeMap;

use entity_entity::Entity;

use crate::window::WindowId;

/// App → host: *this is my screen* (`{screen_w, screen_h, extra_w, extra_h}`).
/// A local extension, like `x-stats`, sent by the v86 machines (`vm-sdk.js
/// reportView`); an app that does not send it is never fitted.
pub const MSG_VIEW: &str = "x-view";

/// Entity type of the stored map.
pub const SIZES_TYPE: &str = "app/state/window-sizes";
/// Settings suffix: `app/entity-browser/settings/window-sizes`.
pub const SIZES_SUFFIX: &str = "window-sizes";

/// The shortest a window may be dragged: the header plus a strip of content.
pub const MIN_PX: u32 = 140;
/// The tallest a stored height may be. Not a layout limit (the window area
/// scrolls) — a bound on what a malformed entity can make us draw.
pub const MAX_PX: u32 = 8_000;
/// One keyboard step on the grip.
pub const STEP_PX: u32 = 40;

/// What a person chose for one size key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizePref {
    /// This many pixels tall.
    Height(u32),
    /// As tall as the app's screen needs at the window's width.
    Fit,
}

/// What the reconcile writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// No inline height: the window takes its content's height (the old,
    /// only, behaviour).
    Natural,
    /// `height: {px}px`, and the `.sized` class so the content fills it.
    Px(u32),
}

pub fn clamp_px(px: f64) -> u32 {
    if !px.is_finite() {
        return MIN_PX;
    }
    (px.round().max(MIN_PX as f64).min(MAX_PX as f64)) as u32
}

/// The size key: what a window *is*, for the purpose of remembering its size.
///
/// A window type, or `{type}/{app}` while a window is running an app — Alpine's
/// terminal and KolibriOS's desktop want different heights, and both live in
/// the Apps window.
pub fn size_key(type_name: &str, running_app_key: Option<&str>) -> String {
    match running_app_key {
        Some(app) if !app.is_empty() => format!("{type_name}/{app}"),
        _ => type_name.to_string(),
    }
}

/// The one precedence rule.
///
/// `fit` is the height the player computed for the app's screen, if it has
/// one; with a stored [`SizePref::Fit`] and no computed height yet (the app has
/// not reported its screen) the window stays natural rather than guessing.
pub fn resolve(live: Option<u32>, stored: Option<SizePref>, fit: Option<u32>) -> Applied {
    if let Some(px) = live {
        return Applied::Px(px);
    }
    match (stored, fit) {
        (Some(SizePref::Height(px)), _) => Applied::Px(px),
        (Some(SizePref::Fit), Some(px)) | (None, Some(px)) => Applied::Px(px),
        (Some(SizePref::Fit), None) | (None, None) => Applied::Natural,
    }
}

/// What an app said about its screen (`x-view`): the screen's own size, and
/// how much of the frame the app's page spends on things that are not the
/// screen (its toolbar), in CSS pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AppView {
    pub screen_w: f64,
    pub screen_h: f64,
    pub extra_w: f64,
    pub extra_h: f64,
}

impl AppView {
    /// `None` for a report that cannot describe a screen, so a malformed or
    /// hostile message fits nothing rather than drawing a zero-height window.
    pub fn new(screen_w: f64, screen_h: f64, extra_w: f64, extra_h: f64) -> Option<Self> {
        let sane = |v: f64, max: f64| v.is_finite() && v >= 0.0 && v <= max;
        if !(sane(screen_w, 16_384.0) && sane(screen_h, 16_384.0) && screen_w >= 1.0 && screen_h >= 1.0) {
            return None;
        }
        if !(sane(extra_w, 4_096.0) && sane(extra_h, 4_096.0)) {
            return None;
        }
        Some(Self { screen_w, screen_h, extra_w, extra_h })
    }
}

/// The section height that shows the app's whole screen at the frame's width.
///
/// - `section_h` / `frame_w` / `frame_h`: measured now. `section_h - frame_h`
///   is everything in the window that is not the frame (header, player bar,
///   stage padding) and does not change with the window's height.
/// - `room_h`: the visible height of the window area. A fitted window never
///   asks to be taller than what can be seen at once — past that the screen is
///   letterboxed at the sides, which is the best a fixed width allows, and
///   still better than a window that has to be scrolled to reach its bottom.
pub fn fit_section_height(view: AppView, section_h: f64, frame_w: f64, frame_h: f64, room_h: f64) -> u32 {
    let chrome = (section_h - frame_h).max(0.0);
    let screen_room_w = (frame_w - view.extra_w).max(1.0);
    let frame_want = view.extra_h + screen_room_w * view.screen_h / view.screen_w;
    let want = chrome + frame_want;
    let cap = if room_h.is_finite() && room_h > 0.0 { room_h } else { want };
    clamp_px(want.min(cap))
}

// ── the stored map ──────────────────────────────────────────────────────────

const K_MODE: &str = "mode";
const K_PX: &str = "px";
const MODE_HEIGHT: &str = "height";
const MODE_FIT: &str = "fit";

pub fn encode(prefs: &BTreeMap<String, SizePref>) -> Entity {
    use entity_ecf::Value;
    let rows = prefs
        .iter()
        .map(|(k, p)| {
            let body = match p {
                SizePref::Height(px) => Value::Map(vec![
                    (Value::Text(K_MODE.into()), Value::Text(MODE_HEIGHT.into())),
                    (Value::Text(K_PX.into()), Value::Integer((*px).into())),
                ]),
                SizePref::Fit => Value::Map(vec![(Value::Text(K_MODE.into()), Value::Text(MODE_FIT.into()))]),
            };
            (Value::Text(k.clone()), body)
        })
        .collect();
    Entity::new(SIZES_TYPE, entity_ecf::to_ecf(&Value::Map(rows))).expect("window sizes entity is well-formed")
}

/// Decode the stored map. A row we cannot read is **skipped**, not fatal: one
/// unreadable size must not cost a person every other size they set, and a
/// skipped row only means that window is natural until it is sized again.
/// `None` for an entity that is not ours at all.
pub fn decode(entity: &Entity) -> Option<BTreeMap<String, SizePref>> {
    if entity.entity_type != SIZES_TYPE {
        return None;
    }
    let value: ciborium::Value = ciborium::from_reader(entity.data.as_slice()).ok()?;
    let map = value.as_map()?;
    let mut out = BTreeMap::new();
    for (k, v) in map {
        let (Some(key), Some(row)) = (k.as_text(), v.as_map()) else { continue };
        let field = |name: &str| row.iter().find(|(k, _)| k.as_text() == Some(name)).map(|(_, v)| v);
        let pref = match field(K_MODE).and_then(|m| m.as_text()) {
            Some(MODE_FIT) => SizePref::Fit,
            Some(MODE_HEIGHT) => {
                let Some(px) = field(K_PX).and_then(|v| v.as_integer()).and_then(|i| u32::try_from(i128::from(i)).ok())
                else {
                    continue;
                };
                SizePref::Height(px.clamp(MIN_PX, MAX_PX))
            }
            _ => continue,
        };
        out.insert(key.to_string(), pref);
    }
    Some(out)
}

// ── the runtime registry ────────────────────────────────────────────────────

#[derive(Default)]
struct Registry {
    prefs: BTreeMap<String, SizePref>,
    /// A drag in progress: `(window, px)`.
    live: Option<(WindowId, u32)>,
    /// The player's fitted height per window, tagged with the size key it was
    /// computed for, so a window that went back to its launcher does not keep
    /// the machine's height.
    fit: BTreeMap<WindowId, (String, u32)>,
    /// Whether the boot read has been installed.
    loaded: bool,
}

thread_local! {
    static REG: RefCell<Registry> = RefCell::new(Registry::default());
}

/// Install what boot read. Called once; a later call replaces the map only if
/// nothing was changed in the meantime — a size the person set while the read
/// was in flight is newer than the read.
pub fn install_loaded(prefs: BTreeMap<String, SizePref>) {
    REG.with(|r| {
        let mut r = r.borrow_mut();
        if !r.loaded {
            // Merge rather than assign: anything set before the load wins.
            for (k, v) in prefs {
                r.prefs.entry(k).or_insert(v);
            }
            r.loaded = true;
        }
    });
}

/// Record a person's choice. Returns the whole map to persist.
pub fn set_pref(key: &str, pref: Option<SizePref>) -> BTreeMap<String, SizePref> {
    REG.with(|r| {
        let mut r = r.borrow_mut();
        match pref {
            Some(p) => {
                r.prefs.insert(key.to_string(), p);
            }
            None => {
                r.prefs.remove(key);
            }
        }
        // A choice made before the boot read lands is still a choice; the
        // late read merges under it (`install_loaded`).
        r.prefs.clone()
    })
}

pub fn pref(key: &str) -> Option<SizePref> {
    REG.with(|r| r.borrow().prefs.get(key).copied())
}

pub fn set_live(id: WindowId, px: Option<u32>) {
    REG.with(|r| r.borrow_mut().live = px.map(|p| (id, p)));
}

pub fn live(id: WindowId) -> Option<u32> {
    REG.with(|r| r.borrow().live.filter(|(w, _)| *w == id).map(|(_, p)| p))
}

/// The player's fitted height for `id`, computed for `key`. Returns whether it
/// changed, so the caller repaints only on a change (a `ResizeObserver` fires
/// on the height this produces, and must not loop).
pub fn set_fit(id: WindowId, key: &str, px: u32) -> bool {
    REG.with(|r| {
        let mut r = r.borrow_mut();
        let new = (key.to_string(), px);
        if r.fit.get(&id) == Some(&new) {
            return false;
        }
        r.fit.insert(id, new);
        true
    })
}

pub fn fit(id: WindowId, key: &str) -> Option<u32> {
    REG.with(|r| r.borrow().fit.get(&id).filter(|(k, _)| k == key).map(|(_, p)| *p))
}

pub fn forget_window(id: WindowId) {
    REG.with(|r| {
        let mut r = r.borrow_mut();
        r.fit.remove(&id);
        if r.live.is_some_and(|(w, _)| w == id) {
            r.live = None;
        }
    });
}

/// Everything the reconcile needs for one window, in one borrow.
pub fn applied(id: WindowId, key: &str) -> Applied {
    resolve(live(id), pref(key), fit(id, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_outranks_everything_and_a_stored_height_outranks_the_apps_fit() {
        assert_eq!(resolve(Some(300), Some(SizePref::Height(900)), Some(700)), Applied::Px(300));
        assert_eq!(resolve(None, Some(SizePref::Height(900)), Some(700)), Applied::Px(900));
        assert_eq!(resolve(None, Some(SizePref::Fit), Some(700)), Applied::Px(700));
    }

    #[test]
    fn an_app_that_reports_a_screen_is_fitted_by_default_and_one_that_does_not_is_left_alone() {
        assert_eq!(resolve(None, None, Some(640)), Applied::Px(640));
        assert_eq!(resolve(None, None, None), Applied::Natural);
        // Asked for fit, nothing reported yet: natural, not a guess.
        assert_eq!(resolve(None, Some(SizePref::Fit), None), Applied::Natural);
    }

    #[test]
    fn the_key_separates_two_apps_in_one_window_type() {
        assert_eq!(size_key("Apps", Some("apps/kolibri")), "Apps/apps/kolibri");
        assert_eq!(size_key("Apps", None), "Apps");
        assert_eq!(size_key("Apps", Some("")), "Apps");
        assert_ne!(size_key("Apps", Some("apps/kolibri")), size_key("Apps", Some("apps/alpine")));
    }

    #[test]
    fn a_4_by_3_screen_in_a_wide_window_is_capped_at_what_can_be_seen() {
        // KolibriOS 1024x768, page toolbar 34 px, window chrome 90 px.
        let v = AppView::new(1024.0, 768.0, 0.0, 34.0).unwrap();
        // A 1400 px-wide frame wants 90 + 34 + 1050 = 1174; the area shows 900.
        assert_eq!(fit_section_height(v, 650.0, 1400.0, 560.0, 900.0), 900);
        // With room, the whole screen at the frame's width.
        assert_eq!(fit_section_height(v, 650.0, 1400.0, 560.0, 2000.0), 1174);
    }

    #[test]
    fn a_4_by_3_screen_on_a_phone_shrinks_the_window_instead_of_letterboxing_it() {
        let v = AppView::new(1024.0, 768.0, 0.0, 34.0).unwrap();
        // 400 px wide frame: 400 * 0.75 = 300 + 34 + chrome 60 = 394, well under 70vh.
        assert_eq!(fit_section_height(v, 620.0, 400.0, 560.0, 800.0), 394);
    }

    #[test]
    fn the_fit_is_a_fixed_point_so_the_observer_it_triggers_does_not_loop() {
        let v = AppView::new(800.0, 600.0, 10.0, 40.0).unwrap();
        let first = fit_section_height(v, 700.0, 900.0, 600.0, 5000.0);
        // After applying it the frame is `first - chrome` tall; recompute.
        let chrome = 100.0;
        let again = fit_section_height(v, first as f64, 900.0, first as f64 - chrome, 5000.0);
        assert_eq!(first, again);
    }

    #[test]
    fn a_report_that_cannot_describe_a_screen_fits_nothing() {
        assert!(AppView::new(0.0, 768.0, 0.0, 0.0).is_none());
        assert!(AppView::new(1024.0, f64::NAN, 0.0, 0.0).is_none());
        assert!(AppView::new(1024.0, 768.0, -5.0, 0.0).is_none());
        assert!(AppView::new(1e9, 768.0, 0.0, 0.0).is_none());
        assert!(AppView::new(1024.0, 768.0, 0.0, 34.0).is_some());
    }

    #[test]
    fn heights_are_clamped_from_both_sides() {
        assert_eq!(clamp_px(10.0), MIN_PX);
        assert_eq!(clamp_px(MIN_PX as f64), MIN_PX);
        assert_eq!(clamp_px(1e9), MAX_PX);
        assert_eq!(clamp_px(f64::INFINITY), MIN_PX);
        assert_eq!(clamp_px(512.4), 512);
    }

    #[test]
    fn the_stored_map_round_trips_and_skips_a_row_it_cannot_read() {
        let mut m = BTreeMap::new();
        m.insert("Apps/apps/kolibri".to_string(), SizePref::Fit);
        m.insert("Shell".to_string(), SizePref::Height(420));
        let e = encode(&m);
        assert_eq!(decode(&e), Some(m.clone()));

        use entity_ecf::Value;
        let bad = Entity::new(
            SIZES_TYPE,
            entity_ecf::to_ecf(&Value::Map(vec![
                (Value::Text("Shell".into()), Value::Map(vec![
                    (Value::Text("mode".into()), Value::Text("height".into())),
                    (Value::Text("px".into()), Value::Integer(420.into())),
                ])),
                (Value::Text("Chat".into()), Value::Map(vec![(Value::Text("mode".into()), Value::Text("height".into()))])),
                (Value::Text("Odd".into()), Value::Map(vec![(Value::Text("mode".into()), Value::Text("sideways".into()))])),
            ])),
        )
        .unwrap();
        let got = decode(&bad).unwrap();
        assert_eq!(got.len(), 1, "only the readable row survives: {got:?}");
        assert_eq!(got.get("Shell"), Some(&SizePref::Height(420)));
    }

    #[test]
    fn an_entity_of_another_type_is_not_a_size_map() {
        let e = Entity::new("app/state/shell", entity_ecf::to_ecf(&entity_ecf::Value::Map(vec![]))).unwrap();
        assert_eq!(decode(&e), None);
    }

    #[test]
    fn a_stored_height_out_of_range_is_clamped_on_the_way_in() {
        use entity_ecf::Value;
        let e = Entity::new(
            SIZES_TYPE,
            entity_ecf::to_ecf(&Value::Map(vec![(
                Value::Text("Shell".into()),
                Value::Map(vec![
                    (Value::Text("mode".into()), Value::Text("height".into())),
                    (Value::Text("px".into()), Value::Integer(3.into())),
                ]),
            )])),
        )
        .unwrap();
        assert_eq!(decode(&e).unwrap().get("Shell"), Some(&SizePref::Height(MIN_PX)));
    }

    #[test]
    fn a_size_set_before_the_boot_read_lands_is_not_overwritten_by_it() {
        set_pref("Shell", Some(SizePref::Height(500)));
        let mut loaded = BTreeMap::new();
        loaded.insert("Shell".to_string(), SizePref::Height(300));
        loaded.insert("Chat".to_string(), SizePref::Height(250));
        install_loaded(loaded);
        assert_eq!(pref("Shell"), Some(SizePref::Height(500)));
        assert_eq!(pref("Chat"), Some(SizePref::Height(250)));
    }

    #[test]
    fn a_fit_computed_for_one_app_is_not_applied_after_the_window_changes_app() {
        assert!(set_fit(7, "Apps/apps/kolibri", 800));
        assert!(!set_fit(7, "Apps/apps/kolibri", 800), "an unchanged fit reports no change");
        assert_eq!(fit(7, "Apps/apps/kolibri"), Some(800));
        assert_eq!(fit(7, "Apps"), None);
        forget_window(7);
        assert_eq!(fit(7, "Apps/apps/kolibri"), None);
    }
}
