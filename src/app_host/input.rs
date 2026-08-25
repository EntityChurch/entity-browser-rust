//! app_host keyboard input **source** — the iframe's keyboard modality.
//!
//! This module is ONE input source. It translates keydown/keyup on the payload
//! window into the modality-neutral verbs of a program-blind
//! [`program_host::input::InputTarget`] (`set_direction`, `press`/`release`),
//! which owns the per-shape state, encodes the shape entity, and — via the
//! delivery this module injects — writes it to the inner peer. The write is
//! captured *in* the iframe and **never crosses ③α**: the host still sees only
//! the `state` emissions, blind to the payload (P1).
//!
//! The split is the reuse seam: what lives HERE is keyboard- and iframe-specific
//! — the physical-key → control-name binding (the keyboard-position convention
//! below), the DOM listeners, and the iframe delivery (D13 stamp + `spawn_local`
//! inner-peer write). The target ([`program_host::input`]) is source- and
//! boundary-free, so a second source drives the SAME target — a second source,
//! not a second code path. That second source is real now: [`install`] builds
//! ONE `Rc<InputTarget>` per port and attaches BOTH the keyboard source here AND
//! the on-screen pointer source ([`super::onscreen`]), so a keyboard key and an
//! on-screen button feed one shared state (one held-key mask, one latest
//! direction).
//!
//! **Program-blind keyboard-position convention.** The keyboard maps physical
//! keys to controller POSITIONS, never to app semantics — arrows/WASD → the four
//! directional-axis positions (`up`/`down`/`left`/`right`), and a fixed key row
//! (Space, then Z/X/C…) → the declared actions in bit order. The program owns
//! name↔bit (its `scene.keymap`, parsed into control bindings by
//! [`controls`](crate::program_host::controls)); composing that with this fixed
//! convention yields key → bit without the host ever naming an app control. This
//! retired the old host-owned `KEY_ACTIONS` guess (no more `Space = fire` baked
//! in — Space is just action[0]'s position). The convention is provisional
//! pending arch ratification of the cross-host default (RESPONSE §5 item 4).
//!
//! Every installed `Closure` is returned to the caller, which holds it in
//! `app_host::LIVE` for the document's lifetime — never `Closure::forget()`
//! (charter D12 / AP1). The iframe document *is* the lifetime.

#![cfg(target_arch = "wasm32")]

use std::collections::HashMap;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::KeyboardEvent;

use crate::peers::Peers;
use crate::program_host::controls::{self, ControlBinding, Role, AXIS_DOWN, AXIS_LEFT, AXIS_RIGHT, AXIS_UP};
use crate::program_host::descriptor::{ProgramPort, SHAPE_DIRECTION, SHAPE_KEY_SET};
use crate::program_host::host;
use crate::program_host::input::InputTarget;
use crate::program_host::shapes::{DIR_DOWN, DIR_LEFT, DIR_RIGHT, DIR_UP};

use super::onscreen::{self, ActionFace, MomentaryGuard};

/// The input shapes this source drives. `admit()` in `run_program` is gated on
/// this plus the display shapes — a program binding an unlisted input shape is
/// refused with the shape named, never mounted-then-mute.
pub const SUPPORTED_INPUT_SHAPES: &[&str] = &[SHAPE_DIRECTION, SHAPE_KEY_SET];

/// What one input port's `install` produced: the pointer/keyboard `Closure`s the
/// caller holds for the document's lifetime (D12), plus the two on-screen
/// surfaces to mount in their separate homes — the thumb `pad` (the bottom
/// overlay) and the `chips` cluster (the meta-chrome bar). Both are `None` when
/// the port's shape has no driver.
pub struct Installed {
    pub closures: Vec<Closure<dyn FnMut(JsValue)>>,
    pub pad: Option<web_sys::Element>,
    pub chips: Option<web_sys::Element>,
}

impl Installed {
    fn empty() -> Self {
        Self {
            closures: Vec::new(),
            pad: None,
            chips: None,
        }
    }
}

/// Build the iframe delivery for a port: stamp the D13 surface
/// (`data-app-host-input = "<field>:<value>"`, same-origin, inside the iframe,
/// never emitted to the host — P1) and spawn the inner-peer write off-loop
/// (awaited so a following tick's `lookup/tree` observes it). Encode already
/// happened in the target; here is only the iframe-specific delivery.
fn iframe_deliver(
    peers: Rc<Peers>,
    peer_id: String,
    ns: String,
    port_path: String,
    field: String,
    observe: web_sys::Element,
) -> Box<dyn Fn(u64, entity_entity::Entity)> {
    Box::new(move |value, entity| {
        let _ = observe.set_attribute("data-app-host-input", &format!("{field}:{value}"));
        let fut = host::input_future(&peers, &peer_id, &ns, &port_path, entity);
        spawn_local(async move {
            if let Err(e) = fut.await {
                tracing::error!("app-host input: write: {e}");
            }
        });
    })
}

/// Install ALL input sources for one input port: build ONE shared
/// `Rc<InputTarget>`, attach the keyboard source (this module) AND the on-screen
/// pointer source ([`super::onscreen`]) to it, and hand back both the held
/// `Closure`s (owned by the caller for the document's lifetime — D12) and the
/// on-screen control panel to mount. Sharing one target is what makes the two
/// sources ONE state, not two racing copies. An unsupported shape installs
/// nothing and logs (admission should already have refused it — defence in depth).
///
/// `min_hold_ms` is the program's own tick period (the same value the tick
/// loop sleeps by, from `desc.tick.rate_hint`): a `key-set` momentary
/// press/release (keyboard OR on-screen) delays its release by that long
/// ([`onscreen::MomentaryGuard`]) so a tap shorter than one tick still holds
/// long enough for the program's `step` to observe it — otherwise a fast tap
/// landing entirely inside one tick's gap is invisible to the program (the
/// "button sometimes does nothing" race between a momentary tap and the
/// discrete clock).
pub fn install(
    peers: Rc<Peers>,
    peer_id: String,
    ns: String,
    port: &ProgramPort,
    field: String,
    observe: web_sys::Element,
    min_hold_ms: i32,
) -> Installed {
    let deliver = iframe_deliver(
        peers,
        peer_id,
        ns,
        port.path.clone(),
        field.clone(),
        observe,
    );
    // Build the shared target + its two sources, per shape.
    let (target, mut closures, layout) = match port.shape.as_str() {
        SHAPE_DIRECTION => {
            let target = Rc::new(InputTarget::direction(port.type_ref.clone(), field, deliver));
            let closures = install_direction(target.clone());
            (target, closures, onscreen::Layout::Direction)
        }
        SHAPE_KEY_SET => {
            // name→bit is the PROGRAM's declaration (control ROLES, parsed by the
            // reference parser). A parse error surfaces (a declared-but-broken
            // control must not vanish silently); an empty keymap means the
            // program declared no usable controls — install nothing (never guess).
            let bindings = match controls::parse_keymap(port.scene.as_ref()) {
                Ok(b) => b,
                Err(e) => {
                    tracing::error!(port = %port.name, "app-host input: key-set keymap parse: {e}");
                    return Installed::empty();
                }
            };
            if bindings.is_empty() {
                tracing::warn!(port = %port.name, "app-host input: key-set port has no usable keymap scene");
                return Installed::empty();
            }
            let action_bit = controls::action_bit_map(&bindings);
            // Program-declared per-binding: which bits latch (flip-on-press) vs
            // hold. `0` for every all-momentary program (all bundled ones today).
            let toggle_bits = controls::toggle_bit_mask(&bindings);
            let target = Rc::new(InputTarget::key_set(
                port.type_ref.clone(),
                field,
                action_bit,
                toggle_bits,
                deliver,
            ));
            let closures = install_key_set(target.clone(), &bindings, min_hold_ms);
            // The standard controller: directional axes → the d-pad, discrete
            // actions → buttons (label/glyph from the program's declaration).
            let axes: Vec<String> = bindings
                .iter()
                .filter(|b| b.role == Role::Axis)
                .map(|b| b.axis.clone())
                .collect();
            let actions: Vec<ActionFace> = bindings
                .iter()
                .filter(|b| b.role == Role::Action)
                .map(|b| ActionFace {
                    name: b.action.clone(),
                    label: b.label.clone(),
                    glyph: b.effective_glyph(),
                })
                .collect();
            (target, closures, onscreen::Layout::KeySet { axes, actions })
        }
        other => {
            tracing::warn!(port = %port.name, shape = other, "app-host input: unsupported shape");
            return Installed::empty();
        }
    };
    // Attach the on-screen source to the same target.
    let onscreen::OnscreenControls { pad, chips, closures: mut pointer_closures } =
        onscreen::build(target, layout, min_hold_ms);
    closures.append(&mut pointer_closures);
    Installed {
        closures,
        pad: Some(pad),
        chips: Some(chips),
    }
}

/// `direction` source — arrow keys (and WASD) map to `DIR_*`; the target keeps
/// latest-wins. One `keydown` listener on the payload window.
fn install_direction(target: Rc<InputTarget>) -> Vec<Closure<dyn FnMut(JsValue)>> {
    let Some(window) = web_sys::window() else {
        return Vec::new();
    };
    let cb = Closure::wrap(Box::new(move |e: JsValue| {
        let Ok(ev) = e.dyn_into::<KeyboardEvent>() else {
            return;
        };
        let dir = match ev.key().as_str() {
            "ArrowUp" | "w" | "W" => DIR_UP,
            "ArrowRight" | "d" | "D" => DIR_RIGHT,
            "ArrowDown" | "s" | "S" => DIR_DOWN,
            "ArrowLeft" | "a" | "A" => DIR_LEFT,
            _ => return,
        };
        ev.prevent_default();
        target.set_direction(dir);
    }) as Box<dyn FnMut(JsValue)>);
    let _ = window.add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref());
    vec![cb]
}

/// `key-set` source — `keydown` presses a key's control name, `keyup` releases
/// it; the target maintains the shared held-mask and writes on change. This
/// source owns only the physical-key → position convention ([`keyboard_map`]);
/// name→bit is the program's (in the target). A key mapped to a name the program
/// didn't declare simply no-ops in the target.
///
/// Each declared name gets its own [`MomentaryGuard`] so a keyboard tap
/// (e.g. a quick Space) is subject to the same `min_hold_ms` release delay as
/// the on-screen buttons — the tap-vs-tick-clock race is a listener-agnostic
/// property of the transport, not an on-screen-only concern.
///
/// **Stuck-key guard (charter/design §200):** a held key's `keyup` only arrives
/// if this window still has focus. When focus leaves — window `blur`, or the
/// document going hidden (tab switch, window minimize) — the release is lost and
/// the bit would latch forever (the "ship keeps rotating" bug). So both events
/// `release_all()` the shared mask directly (bypassing any pending guard delay —
/// losing focus must clear immediately, never wait out a release timer).
/// `visibilitychange` fires on the *document*.
fn install_key_set(
    target: Rc<InputTarget>,
    bindings: &[ControlBinding],
    min_hold_ms: i32,
) -> Vec<Closure<dyn FnMut(JsValue)>> {
    let Some(window) = web_sys::window() else {
        return Vec::new();
    };
    // key → control name, built program-blind from the declared roles.
    let key_name = Rc::new(keyboard_map(bindings));
    // One guard per declared name (not per key — several keys, e.g. Arrow +
    // WASD, can name the same axis and must share one release timer).
    let mut guards: HashMap<String, Rc<MomentaryGuard>> = HashMap::new();
    for (_, name) in key_name.iter() {
        guards
            .entry(name.clone())
            .or_insert_with(|| MomentaryGuard::new(target.clone(), name.clone()));
    }
    let guards = Rc::new(guards);

    let mk = |set: bool| {
        let key_name = key_name.clone();
        let guards = guards.clone();
        Closure::wrap(Box::new(move |e: JsValue| {
            let Ok(ev) = e.dyn_into::<KeyboardEvent>() else {
                return;
            };
            // Ignore auto-repeat keydowns: a held key must count as ONE press so
            // a `toggle` action flips exactly once (a momentary action is
            // unaffected — re-pressing a set bit was already a no-op). `keyup`
            // (`set == false`) never repeats, so only guard the down path.
            if set && ev.repeat() {
                return;
            }
            let key = ev.key();
            let Some((_, name)) = key_name.iter().find(|(k, _)| *k == key) else {
                return;
            };
            let Some(guard) = guards.get(name) else {
                return;
            };
            ev.prevent_default();
            if set {
                guard.press();
            } else {
                guard.release(min_hold_ms);
            }
        }) as Box<dyn FnMut(JsValue)>)
    };
    let on_down = mk(true);
    let on_up = mk(false);
    let _ = window.add_event_listener_with_callback("keydown", on_down.as_ref().unchecked_ref());
    let _ = window.add_event_listener_with_callback("keyup", on_up.as_ref().unchecked_ref());

    // Stuck-key guard: clear the whole held mask when we can no longer see
    // releases. `blur` on the window, `visibilitychange` on the document.
    let mut closures = vec![on_down, on_up];
    let on_blur = {
        let target = target.clone();
        Closure::wrap(Box::new(move |_e: JsValue| target.release_all()) as Box<dyn FnMut(JsValue)>)
    };
    let _ = window.add_event_listener_with_callback("blur", on_blur.as_ref().unchecked_ref());
    closures.push(on_blur);
    if let Some(document) = window.document() {
        let on_hide = {
            let target = target.clone();
            Closure::wrap(Box::new(move |_e: JsValue| {
                // Clear on hide; on re-show there is nothing held to restore.
                if web_sys::window()
                    .and_then(|w| w.document())
                    .map(|d| d.hidden())
                    .unwrap_or(false)
                {
                    target.release_all();
                }
            }) as Box<dyn FnMut(JsValue)>)
        };
        let _ =
            document.add_event_listener_with_callback("visibilitychange", on_hide.as_ref().unchecked_ref());
        closures.push(on_hide);
    }
    closures
}

/// The physical-key → control-name map for a `key-set` port, built program-blind
/// from the declared control ROLES. Directional axes bind to the arrows + WASD
/// (their fixed positions); discrete actions bind to a fixed key row in bit
/// order (Space, then Z/X/C…). The host names no app control — a key resolves to
/// a position/action-slot, and the PROGRAM's `scene.keymap` names the bit. This
/// is the keyboard-position convention (RESPONSE §5 item 4), provisional pending
/// arch's cross-host default; actions beyond the row are simply keyboard-unbound
/// (still reachable on-screen — the overflow policy, item 5, is arch's).
fn keyboard_map(bindings: &[ControlBinding]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut action_idx = 0usize;
    for b in bindings {
        match b.role {
            Role::Axis => {
                for k in axis_keys(&b.axis) {
                    out.push((k.to_string(), b.axis.clone()));
                }
            }
            Role::Action => {
                if let Some(keys) = ACTION_KEY_ROW.get(action_idx) {
                    for k in *keys {
                        out.push((k.to_string(), b.action.clone()));
                    }
                }
                action_idx += 1;
            }
        }
    }
    out
}

/// The arrow + WASD keys for a directional-axis position.
fn axis_keys(axis: &str) -> &'static [&'static str] {
    match axis {
        AXIS_UP => &["ArrowUp", "w", "W"],
        AXIS_DOWN => &["ArrowDown", "s", "S"],
        AXIS_LEFT => &["ArrowLeft", "a", "A"],
        AXIS_RIGHT => &["ArrowRight", "d", "D"],
        _ => &[],
    }
}

/// The fixed action key row, indexed by an action's bit order. Slot 0 is Space
/// (with the legacy `Spacebar` alias some engines still emit). No host-owned
/// "Space = fire" — Space is simply action[0]'s position, whatever that action
/// is. Actions past the row's length are keyboard-unbound (on-screen only).
const ACTION_KEY_ROW: &[&[&str]] = &[
    &[" ", "Spacebar"], // i18n-ignore — keyboard key name, not prose
    &["z", "Z"],
    &["x", "X"],
    &["c", "C"],
    &["v", "V"],
];
