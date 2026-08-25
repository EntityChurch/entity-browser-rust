//! app_host input drivers — keyboard → shape entity → inner-peer input port.
//!
//! The L5 payload is a **focusable document running its own inner peer**, so
//! input is captured *in* the iframe and written straight to the input port via
//! [`host::input_future`] — the identical write the native oracle test drives
//! (`oracle_tests.rs`). It **never crosses ③α**: the host still sees only the
//! `state` emissions, blind to the payload (P1). This is the input mirror of the
//! display `text_driver` — **program-blind, shape-bound**: the value mapping is
//! per shape; the entity's field name is read from the program's *seed*
//! (`shapes::input_field_name`), never assumed (workbench's `EncodeKeySet`
//! diverges from its shipped seed — the seed is the authority, shapes.rs §note).
//!
//! Every installed `Closure` is returned to the caller, which holds it in
//! `app_host::LIVE` for the document's lifetime — never `Closure::forget()`
//! (charter D12 / AP1). The iframe document *is* the lifetime.

#![cfg(target_arch = "wasm32")]

use std::cell::Cell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use web_sys::KeyboardEvent;

use crate::peers::Peers;
use crate::program_host::descriptor::{ProgramPort, SHAPE_DIRECTION, SHAPE_KEY_SET};
use crate::program_host::host;
use crate::program_host::shapes::{self, DIR_DOWN, DIR_LEFT, DIR_RIGHT, DIR_UP};

/// The input shapes this host drives. `admit()` in `run_program` is gated on
/// this plus the display shapes — a program binding an unlisted input shape is
/// refused with the shape named, never mounted-then-mute.
pub const SUPPORTED_INPUT_SHAPES: &[&str] = &[SHAPE_DIRECTION, SHAPE_KEY_SET];

/// Everything an installed driver needs to write one input value: which port,
/// what entity type/field, on which peer. Cloned into the keyboard closures
/// (all `'static`).
#[derive(Clone)]
struct InputSink {
    peers: Rc<Peers>,
    peer_id: String,
    ns: String,
    port_path: String,
    type_ref: String,
    /// The seed's field name (`dir` for Snake, `keys` for Asteroids).
    field: String,
    /// D13 surface: the payload element we stamp `data-app-host-input` on so the
    /// input state has an observable surface (and the e2e can read it across the
    /// same-origin boundary). Not `state` — this never reaches the host.
    observe: web_sys::Element,
}

impl InputSink {
    /// Encode `value` as this port's entity and write it to the inner peer
    /// (awaited off-loop so a following tick's `lookup/tree` observes it), then
    /// stamp the D13 surface. Encode/write faults log loudly (never silent).
    fn write(&self, value: u64) {
        let entity = match shapes::encode_input(&self.type_ref, &self.field, value) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(port = %self.port_path, "app-host input: encode: {e}");
                return;
            }
        };
        // D13: the input state has a surface — same-origin, inside the iframe,
        // never emitted to the host (P1 stays intact).
        let _ = self
            .observe
            .set_attribute("data-app-host-input", &format!("{}:{}", self.field, value));
        let fut = host::input_future(&self.peers, &self.peer_id, &self.ns, &self.port_path, entity);
        spawn_local(async move {
            if let Err(e) = fut.await {
                tracing::error!("app-host input: write: {e}");
            }
        });
    }
}

/// Install the keyboard driver for one input port on the payload's own window,
/// returning the held `Closure`s (the caller owns them for the document's
/// lifetime — D12). An unsupported shape installs nothing and logs (admission
/// should already have refused it, so this is defence in depth).
pub fn install(
    peers: Rc<Peers>,
    peer_id: String,
    ns: String,
    port: &ProgramPort,
    field: String,
    observe: web_sys::Element,
) -> Vec<Closure<dyn FnMut(JsValue)>> {
    let sink = InputSink {
        peers,
        peer_id,
        ns,
        port_path: port.path.clone(),
        type_ref: port.type_ref.clone(),
        field,
        observe,
    };
    match port.shape.as_str() {
        SHAPE_DIRECTION => install_direction(sink),
        SHAPE_KEY_SET => install_key_set(sink, port),
        other => {
            tracing::warn!(port = %port.name, shape = other, "app-host input: unsupported shape");
            Vec::new()
        }
    }
}

/// `direction` shape — arrow keys (and WASD) map to `DIR_*`; latest press wins.
/// One `keydown` listener on the payload window.
fn install_direction(sink: InputSink) -> Vec<Closure<dyn FnMut(JsValue)>> {
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
        sink.write(dir);
    }) as Box<dyn FnMut(JsValue)>);
    let _ = window.add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref());
    vec![cb]
}

/// `key-set` shape — a held-key bitmask snapshot. `keydown` sets a key's bit,
/// `keyup` clears it; the current mask is written on every change. The key→bit
/// map comes from the port's `scene.keymap` (Asteroids ships one); an absent or
/// unrecognised keymap installs nothing and logs (admission passed the shape,
/// but we cannot map keys without the program's own table — never guess).
fn install_key_set(sink: InputSink, port: &ProgramPort) -> Vec<Closure<dyn FnMut(JsValue)>> {
    let Some(window) = web_sys::window() else {
        return Vec::new();
    };
    let keymap = decode_keymap(port);
    if keymap.is_empty() {
        tracing::warn!(port = %port.name, "app-host input: key-set port has no usable keymap scene");
        return Vec::new();
    }

    // The live held-key bitmask, shared by the keydown/keyup closures.
    let mask = Rc::new(Cell::new(0u64));

    let mk = |set: bool| {
        let keymap = keymap.clone();
        let mask = mask.clone();
        let sink = sink.clone();
        Closure::wrap(Box::new(move |e: JsValue| {
            let Ok(ev) = e.dyn_into::<KeyboardEvent>() else {
                return;
            };
            let Some(bit) = keymap.iter().find(|(k, _)| *k == ev.key()).map(|(_, b)| *b) else {
                return;
            };
            ev.prevent_default();
            let cur = mask.get();
            let next = if set { cur | bit } else { cur & !bit };
            if next != cur {
                mask.set(next);
                sink.write(next);
            }
        }) as Box<dyn FnMut(JsValue)>)
    };

    let on_down = mk(true);
    let on_up = mk(false);
    let _ = window.add_event_listener_with_callback("keydown", on_down.as_ref().unchecked_ref());
    let _ = window.add_event_listener_with_callback("keyup", on_up.as_ref().unchecked_ref());
    vec![on_down, on_up]
}

/// Physical-key → semantic action bindings. The PROGRAM owns bit↔action (its
/// `scene.keymap`); the HOST owns which keyboard key means which action (a UI
/// choice). Composing the two yields key → bit. An action the program doesn't
/// declare is simply unbound. WASD mirrors the arrows; Space fires.
const KEY_ACTIONS: &[(&str, &str)] = &[
    ("ArrowLeft", "left"),
    ("a", "left"),
    ("A", "left"),
    ("ArrowRight", "right"),
    ("d", "right"),
    ("D", "right"),
    ("ArrowUp", "thrust"),
    ("w", "thrust"),
    ("W", "thrust"),
    (" ", "fire"),
    ("Spacebar", "fire"),
];

/// Build the `keydown`/`keyup` key → bitmask table for a `key-set` port.
///
/// The port's `scene.keymap` is `{ "<bit_index>": "<action>" }` (Asteroids:
/// `{"0":"left","1":"right","2":"thrust","3":"fire"}`) — it names each BIT, it
/// is NOT a keyboard binding. Bit `i` in the held mask is `1 << i` (confirmed
/// against the program's own oracle input schedule: `keys=2`=bit1=right,
/// `4`=thrust, `8`=fire). We invert it to action → bit, then compose with
/// [`KEY_ACTIONS`] to get key → bit. An empty result installs no listener
/// (logged) — we never guess a binding the program didn't declare.
fn decode_keymap(port: &ProgramPort) -> Vec<(String, u64)> {
    let Some(scene) = port.scene.as_ref() else {
        return Vec::new();
    };
    let Some(map) = scene.as_map() else {
        return Vec::new();
    };
    let Some(entries) = map
        .iter()
        .find(|(k, _)| k.as_text() == Some("keymap"))
        .and_then(|(_, v)| v.as_map())
    else {
        return Vec::new();
    };
    // action → bit value (`1 << bit_index`).
    let action_bit: Vec<(&str, u64)> = entries
        .iter()
        .filter_map(|(k, v)| {
            let bit_index: u32 = k.as_text()?.parse().ok()?;
            let action = v.as_text()?;
            (bit_index < 64).then(|| (action, 1u64 << bit_index))
        })
        .collect();
    // Compose physical key → bit through the shared action name.
    KEY_ACTIONS
        .iter()
        .filter_map(|(key, action)| {
            action_bit
                .iter()
                .find(|(a, _)| a == action)
                .map(|(_, bit)| (key.to_string(), *bit))
        })
        .collect()
}
