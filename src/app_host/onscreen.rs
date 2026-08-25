//! app_host on-screen input **source** — the iframe's pointer/touch modality,
//! presented as a floating **virtual gamepad HUD**.
//!
//! This is the SECOND input source (the first is [`super::input`]'s keyboard).
//! It translates pointer events into the SAME program-blind
//! [`InputTarget`](crate::program_host::input::InputTarget) the keyboard drives;
//! both share one `Rc<InputTarget>`, so they contribute to ONE state (one
//! held-key mask / one latest direction) — a second source, not a second code
//! path (proven natively by `key_set_shared_mask_accumulates`).
//!
//! **It is a touch HUD, so it is laid out like one** (grounded in mobile
//! virtual-controller convention, not a form):
//! - **Corner-anchored overlay, not a block.** The pad is `position:fixed` to the
//!   iframe viewport's bottom edge and floats *over* the display — it never
//!   pushes the game up or sits in an awkward centered box. In landscape the two
//!   thumbs rest at the lower corners, so the **directional control sits in one
//!   bottom corner and the action buttons in the other** (movement left / actions
//!   right by default). Pointer events pass through the gaps to the game.
//! - **Scales with the viewport.** Button sizes are `clamp()`ed to `vmin` with a
//!   ≥44px floor (touch-target minimum) and a cap so they don't balloon on a
//!   desktop/tablet — no fixed pixel grid.
//! - **Handedness.** A ⇄ chip mirrors the layout (movement ↔ actions) for
//!   left-handers. Session-scoped for now; a persistent per-user setting lands
//!   with generic app settings.
//! - **Auto visibility.** A d-pad is noise on a desktop where the keyboard
//!   already drives everything, so the pad is **shown by default on touch**
//!   (`@media (hover:none)…`) and **hidden by default on a precise-pointer
//!   desktop** — a 🎮 chip (always present) overrides either way. Unlike a
//!   device *sniff* for the whole feature, the override is always one tap away,
//!   so a wrong guess is cheap. (Earlier we showed-everywhere to dodge the
//!   headless-`pointer:none` e2e trap; the e2e now force-shows via the chip, so
//!   the real default can follow the device.)
//! - **Overflow.** Actions wrap in one cluster under the thumb today (Asteroids
//!   has one, Fire). Zoning frequent-vs-system actions is the next rung when a
//!   program declares many — the contract (`controls`) already carries the role.
//!
//! Every pointer `Closure` is returned to the caller, held in `app_host::LIVE`
//! for the document's lifetime — never `Closure::forget()` (charter D12 / AP1).

#![cfg(target_arch = "wasm32")]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::Element;

use crate::dom::util;
use crate::program_host::controls::{AXIS_DOWN, AXIS_LEFT, AXIS_RIGHT, AXIS_UP};
use crate::program_host::input::InputTarget;
use crate::program_host::shapes::{DIR_DOWN, DIR_LEFT, DIR_RIGHT, DIR_UP};

/// The control-panel stylesheet — the on-screen input pad's overlay layout,
/// responsive sizing, and auto/toggle visibility policy, PLUS the host-owned
/// chrome [`super::run_program`] mounts for every program. **Only the thumb pad
/// (`.ah-pad`) overlays the board** — the mobile gameplay convention. The META
/// chrome does NOT: the run-state controls (`.ah-hostbar`, reset/pause) and the
/// input chips (`.ah-chips`, 🎮/⇄) sit together in a slim normal-flow bar
/// (`.ah-chrome`, `space-between`) ABOVE the board, and the program-owned status
/// caption (`.ah-status`) sits below it. The chrome bar and the board share
/// `max-width:420px; margin:0 auto`, so they line up as one centred column with
/// the settings-y chrome no longer floating over the play area. Injected ONCE per
/// payload by [`super::run_program`], unconditionally (host controls exist even
/// for input-less programs). Colours use `var(--token, #lit)` so they resolve
/// from the fallbacks even though the stripped payload never installs the `:root`
/// token block; the overlay pad still uses `rgba()` so the game shows through.
///
/// Visibility: the pad carries `data-mode` — `auto` (media-query default),
/// `shown`, or `hidden` (the chip sets the explicit two). `--ah-key`/`--ah-btn`
/// are the `clamp`ed touch-target sizes (≥44px floor, `vmin`-scaled, capped).
pub const CONTROLS_CSS: &str = "\
.ah-pad{position:fixed;left:0;right:0;bottom:0;z-index:2147483000;box-sizing:border-box;\
padding:max(12px,3.2vmin);display:flex;align-items:flex-end;justify-content:space-between;\
gap:16px;pointer-events:none;user-select:none;-webkit-user-select:none;\
--ah-key:clamp(46px,12.5vmin,76px);--ah-btn:clamp(56px,15vmin,92px);}\
.ah-pad.ah-lefty{flex-direction:row-reverse;}\
.ah-pad[data-mode=\"hidden\"]{display:none;}\
@media (hover:hover) and (pointer:fine){.ah-pad[data-mode=\"auto\"]{display:none;}}\
.ah-dpad,.ah-actions{pointer-events:auto;}\
.ah-dpad{display:grid;\
grid-template-columns:repeat(3,var(--ah-key));grid-template-rows:repeat(3,var(--ah-key));gap:6px;}\
.ah-up{grid-area:1/2/2/3;}.ah-left{grid-area:2/1/3/2;}\
.ah-right{grid-area:2/3/3/4;}.ah-down{grid-area:3/2/4/3;}\
.ah-actions{display:flex;flex-wrap:wrap-reverse;gap:12px;\
max-width:min(48vw,340px);justify-content:flex-end;align-content:flex-end;}\
.ah-btn{cursor:pointer;font:inherit;font-weight:600;color:var(--text, #e2e2ea);\
background:rgba(28,28,44,0.62);border:1px solid rgba(255,255,255,0.22);\
border-radius:14px;touch-action:none;-webkit-user-select:none;user-select:none;\
-webkit-backdrop-filter:blur(3px);backdrop-filter:blur(3px);\
display:flex;align-items:center;justify-content:center;}\
.ah-dpad .ah-btn{width:var(--ah-key);height:var(--ah-key);font-size:calc(var(--ah-key)*0.44);}\
.ah-actions .ah-btn{min-width:var(--ah-btn);height:var(--ah-btn);\
padding:0 calc(var(--ah-btn)*0.2);font-size:clamp(13px,3.4vmin,18px);gap:6px;}\
.ah-btn:active{background:var(--accent, #90d0ff);color:var(--accent-text, #1a1a2e);\
border-color:var(--accent, #90d0ff);}\
.ah-chrome{display:flex;align-items:center;justify-content:space-between;gap:8px;\
max-width:420px;margin:0 auto 10px;}\
.ah-hostbar,.ah-chips{display:flex;gap:6px;}\
[data-app-host-display]{max-width:420px;margin:0 auto;}\
.ah-chip{cursor:pointer;font:inherit;font-size:clamp(11px,2.6vmin,13px);line-height:1;\
padding:6px 10px;border-radius:999px;border:1px solid var(--border, rgba(255,255,255,0.22));\
background:var(--surface-sunken, #0a0a1a);color:var(--text-muted, #9aa3b2);}\
.ah-status{display:flex;justify-content:center;max-width:420px;margin:8px auto 0;\
font-variant-numeric:tabular-nums;}";

/// One action button's presentation — the program-declared action name (the
/// press token), its label, and its (declared or standard-default) glyph.
pub struct ActionFace {
    pub name: String,
    pub label: String,
    pub glyph: Option<String>,
}

/// Which on-screen layout a port's shape gets.
/// - `Direction`: the `direction` shape's fixed 4-way **latched** d-pad.
/// - `KeySet`: the port's declared axes on a **momentary** d-pad plus its
///   declared actions as a wrapping button cluster.
pub enum Layout {
    Direction,
    KeySet {
        /// Axis positions present (`up`/`down`/`left`/`right`), any subset.
        axes: Vec<String>,
        /// Action buttons, in declaration (bit) order.
        actions: Vec<ActionFace>,
    },
}

/// The d-pad cross: fixed glyph + position class per position, in render order.
const AXIS_FACES: &[(&str, &str, &str)] = &[
    (AXIS_UP, "\u{25B2}", "ah-up"),      // ▲
    (AXIS_LEFT, "\u{25C0}", "ah-left"),  // ◀
    (AXIS_RIGHT, "\u{25B6}", "ah-right"), // ▶
    (AXIS_DOWN, "\u{25BC}", "ah-down"),  // ▼
];

/// The two on-screen surfaces one input port produces: the thumb `pad` (mounted
/// as the `position:fixed` bottom overlay) and the `chips` cluster (🎮 show/hide
/// + ⇄ handedness — mounted into the host chrome bar, NOT over the board). They
/// mount in different places now, so they come back separately; the pointer
/// `Closure`s are held by the caller for the document's lifetime (D12).
pub struct OnscreenControls {
    pub pad: Element,
    pub chips: Element,
    pub closures: Vec<Closure<dyn FnMut(JsValue)>>,
}

/// Build the on-screen control panel for one input port and wire its pointer
/// events to `target`. `min_hold_ms` is the program's own tick period (from
/// `desc.tick.rate_hint`) — a momentary button's release is delayed by that
/// long ([`MomentaryGuard`]) so a fast tap still holds long enough for one
/// tick to observe it. Returns the overlay `pad` and the `chips` cluster as
/// two separately-mountable elements (the caller overlays the pad and drops
/// the chips into the meta-chrome bar) plus the pointer `Closure`s to hold for
/// the document's lifetime.
pub fn build(target: Rc<InputTarget>, layout: Layout, min_hold_ms: i32) -> OnscreenControls {
    let mut closures: Vec<Closure<dyn FnMut(JsValue)>> = Vec::new();

    // The pad — the show/hide target (`data-app-host-controls` = shape tag +
    // e2e/observability hook, `data-mode` = auto by default).
    let pad = util::create_element("div");
    util::set_attr(&pad, "class", "ah-pad");
    util::set_attr(&pad, "data-mode", "auto");

    match layout {
        Layout::Direction => {
            util::set_attr(&pad, "data-app-host-controls", "direction");
            let dpad = util::create_element("div");
            util::set_attr(&dpad, "class", "ah-dpad");
            // Latched cross: Up / Left / Right / Down.
            for (glyph, dir, pos_class) in [
                ("\u{25B2}", DIR_UP, "ah-up"),
                ("\u{25C0}", DIR_LEFT, "ah-left"),
                ("\u{25B6}", DIR_RIGHT, "ah-right"),
                ("\u{25BC}", DIR_DOWN, "ah-down"),
            ] {
                let btn = dpad_latched_button(glyph, dir, pos_class, &target, &mut closures);
                util::append(&dpad, &btn);
            }
            util::append(&pad, &dpad);
        }
        Layout::KeySet { axes, actions } => {
            util::set_attr(&pad, "data-app-host-controls", "key-set");
            // Directional axes → the momentary d-pad (only the positions present).
            if !axes.is_empty() {
                let dpad = util::create_element("div");
                util::set_attr(&dpad, "class", "ah-dpad");
                for (pos, glyph, pos_class) in AXIS_FACES {
                    if axes.iter().any(|a| a == pos) {
                        let btn = momentary_button(
                            pos, glyph, pos_class, pos, &target, min_hold_ms, &mut closures,
                        );
                        util::append(&dpad, &btn);
                    }
                }
                util::append(&pad, &dpad);
            }
            // Discrete actions → a wrapping cluster under the (other) thumb.
            if !actions.is_empty() {
                let cluster = util::create_element("div");
                util::set_attr(&cluster, "class", "ah-actions");
                for a in &actions {
                    let face = match &a.glyph {
                        Some(g) => format!("{g} {}", a.label),
                        None => a.label.clone(),
                    };
                    let btn = momentary_button(
                        &a.name, &face, "", &a.label, &target, min_hold_ms, &mut closures,
                    );
                    util::append(&cluster, &btn);
                }
                util::append(&pad, &cluster);
            }
        }
    }

    // The always-present chip cluster — 🎮 show/hide + ⇄ handedness swap. It
    // rides the meta-chrome bar (not the play area) but still drives the pad by a
    // held `Element` reference, so it stays reachable whether the pad is shown or
    // hidden regardless of where the two sit in the DOM.
    let chips = util::create_element("div");
    util::set_attr(&chips, "class", "ah-chips");
    let (show_chip, show_cb) = show_hide_chip(&pad);
    let (swap_chip, swap_cb) = swap_chip(&pad);
    util::append(&chips, &show_chip);
    util::append(&chips, &swap_chip);
    closures.push(show_cb);
    closures.push(swap_cb);

    OnscreenControls { pad, chips, closures }
}

/// A LATCHED d-pad button (`direction` shape): `pointerdown` sets the latest
/// direction (latest-wins, so there is no release). `data-control` carries the
/// encoded value for the e2e to target.
fn dpad_latched_button(
    glyph: &str,
    dir: u64,
    pos_class: &str,
    target: &Rc<InputTarget>,
    closures: &mut Vec<Closure<dyn FnMut(JsValue)>>,
) -> Element {
    let btn = util::create_element("button");
    util::set_attr(&btn, "type", "button");
    util::set_attr(&btn, "class", &format!("ah-btn {pos_class}"));
    util::set_attr(&btn, "data-control", &format!("dir:{dir}"));
    util::set_attr(&btn, "aria-label", &format!("direction {dir}"));
    util::set_text(&btn, glyph);

    let target = target.clone();
    let cb = Closure::wrap(Box::new(move |e: JsValue| {
        if let Ok(ev) = e.dyn_into::<web_sys::Event>() {
            ev.prevent_default();
        }
        target.set_direction(dir);
    }) as Box<dyn FnMut(JsValue)>);
    let _ = btn.add_event_listener_with_callback("pointerdown", cb.as_ref().unchecked_ref());
    closures.push(cb);
    btn
}

/// A `key-set` momentary control's press/release guard: cancels a pending
/// delayed release when pressed again, and delays the actual `release()` call
/// by `delay_ms` so a tap **shorter than the host's tick period** still holds
/// the bit long enough for at least one tick's `step` eval to observe it.
///
/// Without this, a press+release that both land inside one tick's gap (the
/// tick reads the mask once every ~`rate_hint` period; a human tap is the same
/// order of magnitude) is invisible to the program — the button "just does
/// nothing" some fraction of the time, indistinguishable from a broken
/// listener. The write path itself is fine; this closes the race between a
/// momentary tap and the discrete clock. Shared by both input sources
/// (on-screen buttons here, keyboard `key-set` in [`super::input`]) so the fix
/// lives once.
pub(super) struct MomentaryGuard {
    target: Rc<InputTarget>,
    name: String,
    timer_id: Cell<Option<i32>>,
    // Keeps the pending timeout's closure alive until it fires or is replaced.
    timer_cb: RefCell<Option<Closure<dyn FnMut()>>>,
}

impl MomentaryGuard {
    pub(super) fn new(target: Rc<InputTarget>, name: String) -> Rc<Self> {
        Rc::new(Self {
            target,
            name,
            timer_id: Cell::new(None),
            timer_cb: RefCell::new(None),
        })
    }

    fn cancel_pending(&self) {
        if let Some(id) = self.timer_id.take() {
            if let Some(win) = web_sys::window() {
                win.clear_timeout_with_handle(id);
            }
        }
    }

    /// Press now; cancel any release still waiting out its delay (a re-press
    /// before the delayed release fired must never drop the hold).
    pub(super) fn press(&self) {
        self.cancel_pending();
        self.target.press(&self.name);
    }

    /// Release after `delay_ms` unless preempted by another `press`/`release`.
    /// A release already pending is left alone (pointerup/pointerleave/pointercancel
    /// commonly fire together for one physical release).
    pub(super) fn release(self: &Rc<Self>, delay_ms: i32) {
        if self.timer_id.get().is_some() {
            return;
        }
        let Some(win) = web_sys::window() else {
            self.target.release(&self.name);
            return;
        };
        let this = self.clone();
        let cb = Closure::wrap(Box::new(move || {
            this.timer_id.set(None);
            this.target.release(&this.name);
        }) as Box<dyn FnMut()>);
        if let Ok(id) =
            win.set_timeout_with_callback_and_timeout_and_arguments_0(cb.as_ref().unchecked_ref(), delay_ms)
        {
            self.timer_id.set(Some(id));
        }
        *self.timer_cb.borrow_mut() = Some(cb);
    }
}

/// A MOMENTARY button (`key-set` shape — both a d-pad axis and an action
/// button): `pointerdown` presses the named token, and any of
/// `pointerup`/`pointerleave`/`pointercancel` releases it — so a pointer that
/// slides off the button (or a cancelled touch) never sticks the held bit on.
/// The release is delayed by `min_hold_ms` via [`MomentaryGuard`] (the
/// tap-vs-tick-clock race). `name` is the program-declared press token (an
/// axis position like `right`, or an action name like `fire`); `face` is what
/// the button shows; `extra_class` places a d-pad button on the cross (`""`
/// for a free-flowing action button).
fn momentary_button(
    name: &str,
    face: &str,
    extra_class: &str,
    aria: &str,
    target: &Rc<InputTarget>,
    min_hold_ms: i32,
    closures: &mut Vec<Closure<dyn FnMut(JsValue)>>,
) -> Element {
    let btn = util::create_element("button");
    util::set_attr(&btn, "type", "button");
    let class = if extra_class.is_empty() {
        "ah-btn".to_string()
    } else {
        format!("ah-btn {extra_class}")
    };
    util::set_attr(&btn, "class", &class);
    util::set_attr(&btn, "data-control", &format!("press:{name}"));
    util::set_attr(&btn, "aria-label", aria);
    util::set_text(&btn, face);

    let guard = MomentaryGuard::new(target.clone(), name.to_string());

    // Press.
    let down = {
        let guard = guard.clone();
        Closure::wrap(Box::new(move |e: JsValue| {
            if let Ok(ev) = e.dyn_into::<web_sys::Event>() {
                ev.prevent_default();
            }
            guard.press();
        }) as Box<dyn FnMut(JsValue)>)
    };
    let _ = btn.add_event_listener_with_callback("pointerdown", down.as_ref().unchecked_ref());

    // Release (shared across the three "pointer left / lifted / cancelled" events).
    let up = {
        let guard = guard.clone();
        Closure::wrap(Box::new(move |_e: JsValue| {
            guard.release(min_hold_ms);
        }) as Box<dyn FnMut(JsValue)>)
    };
    for ev in ["pointerup", "pointerleave", "pointercancel"] {
        let _ = btn.add_event_listener_with_callback(ev, up.as_ref().unchecked_ref());
    }
    closures.push(down);
    closures.push(up);
    btn
}

/// Whether an element currently renders (its computed `display` is not `none`) —
/// the *effective* visibility, so the show/hide chip flips the real state
/// whether the pad is `auto` (media-decided), `shown`, or `hidden`.
fn is_visible(el: &Element) -> bool {
    web_sys::window()
        .and_then(|w| w.get_computed_style(el).ok().flatten())
        .and_then(|s| s.get_property_value("display").ok())
        .map(|d| d != "none")
        .unwrap_or(true)
}

/// The 🎮 show/hide chip: reads the pad's *effective* visibility and flips it to
/// the explicit opposite (`shown`/`hidden`), overriding the auto default.
fn show_hide_chip(pad: &Element) -> (Element, Closure<dyn FnMut(JsValue)>) {
    let chip = util::create_element("button");
    util::set_attr(&chip, "type", "button");
    util::set_attr(&chip, "class", "ah-chip");
    util::set_attr(&chip, "data-controls-toggle", "");
    util::set_text(&chip, "\u{1F3AE}"); // 🎮

    let pad = pad.clone();
    let cb = Closure::wrap(Box::new(move |e: JsValue| {
        if let Ok(ev) = e.dyn_into::<web_sys::Event>() {
            ev.prevent_default();
        }
        let next = if is_visible(&pad) { "hidden" } else { "shown" };
        let _ = pad.set_attribute("data-mode", next);
        let _ = pad.set_attribute("data-controls-mode", next); // observability
    }) as Box<dyn FnMut(JsValue)>);
    let _ = chip.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
    (chip, cb)
}

/// The ⇄ handedness chip: mirrors the pad (movement ↔ actions) by toggling the
/// `ah-lefty` class. Session-scoped; persistence lands with generic settings.
fn swap_chip(pad: &Element) -> (Element, Closure<dyn FnMut(JsValue)>) {
    let chip = util::create_element("button");
    util::set_attr(&chip, "type", "button");
    util::set_attr(&chip, "class", "ah-chip");
    util::set_attr(&chip, "data-controls-swap", "");
    util::set_attr(&chip, "aria-label", "swap control sides");
    util::set_text(&chip, "\u{21C4}"); // ⇄

    let pad = pad.clone();
    let cb = Closure::wrap(Box::new(move |e: JsValue| {
        if let Ok(ev) = e.dyn_into::<web_sys::Event>() {
            ev.prevent_default();
        }
        let cls = pad.get_attribute("class").unwrap_or_default();
        let lefty = cls.split_whitespace().any(|c| c == "ah-lefty");
        let next = if lefty {
            cls.split_whitespace().filter(|c| *c != "ah-lefty").collect::<Vec<_>>().join(" ")
        } else {
            format!("{cls} ah-lefty")
        };
        let _ = pad.set_attribute("class", &next);
    }) as Box<dyn FnMut(JsValue)>);
    let _ = chip.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());
    (chip, cb)
}
