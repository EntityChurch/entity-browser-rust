//! app_host debug overlay — a developer-facing panel that makes the running
//! compute program's mount-contract wiring and live tree state visible, plus
//! a single-step control for the paused clock. This is a LOCAL diagnostic
//! surface only: it reads the inner peer directly (the same `&Peers` the tick
//! loop already holds) and never crosses ③α — the outer host still only ever
//! sees `state` emissions (P1 untouched).
//!
//! v1 scope, deliberately minimal (no upstream `entity-compute` changes, no
//! reuse of the full window-manager Entity Tree component — `app_host` has no
//! `DomCtx`/window machinery to host it):
//! - **Topology**: the mount contract's own wiring (state → step, each output
//!   port's → source, each input port's seed) straight from the already-decoded
//!   [`ProgramDescriptor`] — free, since the shape is known before anything
//!   evaluates.
//! - **Live tree dump**: every entity under the program's namespace
//!   (`peers.tree_listing` + `get_entity`, both synchronous on the Direct arm
//!   `app_host` always runs on), with the DYNAMIC paths (state + every port)
//!   decoded inline so you can watch them change; the rest (the static bundle —
//!   IR/code/assets materialized once at F-E1) is listed by path/type/size only,
//!   not decoded, so the dump stays readable instead of a wall of IR CBOR.

#![cfg(target_arch = "wasm32")]

use std::cell::Cell;
use std::collections::BTreeSet;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::Element;

use crate::dom::util;
use crate::peers::Peers;
use crate::program_host::descriptor::ProgramDescriptor;
use crate::program_host::host;

/// Debug-panel styling, appended alongside [`super::onscreen::CONTROLS_CSS`].
/// `.ah-debug` is hidden by default (`data-mode="hidden"`, same show/hide
/// convention as the on-screen pad). ONE scroll region on the panel itself
/// (not per-`<pre>`) — nested independent scrollers were confusing and, on a
/// small/fixed-size window (Tauri's default 1280x720 with no page scroll),
/// let the panel balloon the page past the viewport with no way to reach the
/// rest of it; capping the panel's own height and letting `[data-app-host]`
/// (see CONTROLS_CSS) scroll the page is the one scroll boundary that matters.
pub const DEBUG_CSS: &str = "\
.ah-debug{max-width:420px;margin:8px auto 0;max-height:280px;overflow-y:auto;\
border:1px solid var(--border, rgba(255,255,255,0.22));\
border-radius:8px;padding:8px;background:var(--surface-sunken, #0a0a1a);}\
.ah-debug[data-mode=\"hidden\"]{display:none;}\
.ah-debug h4{margin:0 0 4px;font-size:12px;font-weight:600;color:var(--text-muted, #9aa3b2);}\
.ah-debug pre{margin:0 0 10px;white-space:pre-wrap;word-break:break-all;\
font-family:ui-monospace,monospace;font-size:11px;line-height:1.4;color:var(--text, #e2e2ea);}\
.ah-debug pre:last-child{margin-bottom:0;}";

/// What [`build`] hands back: the toggle chip (mount into the chrome bar), the
/// panel (mount below the status caption), the live-dump `<pre>` the tick loop
/// refreshes, the `shown` flag gating that refresh, and the held pointer
/// `Closure` (document lifetime, D12 — never `Closure::forget`).
pub struct DebugPanel {
    pub chip: Element,
    pub panel: Element,
    pub tree_pre: Element,
    /// Whether the panel is currently shown — the tick loop only recomputes
    /// the tree dump (a tree_listing + N get_entity reads) when this is true,
    /// so a hidden panel costs nothing per tick.
    pub shown: Rc<Cell<bool>>,
    pub closures: Vec<Closure<dyn FnMut(JsValue)>>,
}

/// Build the debug chip + panel. `topology` is rendered once (the wiring
/// never changes at runtime — only the values flowing through it do); the
/// second `<pre>` (`tree_pre`) starts empty and is filled by the caller on the
/// first refresh.
pub fn build(topology: &str) -> DebugPanel {
    let chip = util::create_element("button");
    util::set_attr(&chip, "type", "button");
    util::set_attr(&chip, "class", "ah-chip");
    util::set_attr(&chip, "data-debug-toggle", "");
    util::set_attr(&chip, "aria-label", "debug panel");
    util::set_text(&chip, "\u{1F41E}"); // 🐞

    let panel = util::create_element("div");
    util::set_attr(&panel, "class", "ah-debug");
    util::set_attr(&panel, "data-mode", "hidden");
    util::set_attr(&panel, "data-app-host-debug", "");

    let topo_h = util::create_element("h4");
    util::set_text(&topo_h, "Compute topology");
    util::append(&panel, &topo_h);
    let topo_pre = util::create_element("pre");
    util::set_attr(&topo_pre, "data-app-host-debug-topology", "");
    util::set_text(&topo_pre, topology);
    util::append(&panel, &topo_pre);

    let tree_h = util::create_element("h4");
    util::set_text(&tree_h, "Entity tree (live)");
    util::append(&panel, &tree_h);
    let tree_pre = util::create_element("pre");
    util::set_attr(&tree_pre, "data-app-host-debug-tree", "");
    util::append(&panel, &tree_pre);

    let shown = Rc::new(Cell::new(false));
    let cb = {
        let shown = shown.clone();
        let panel = panel.clone();
        Closure::wrap(Box::new(move |e: JsValue| {
            if let Ok(ev) = e.dyn_into::<web_sys::Event>() {
                ev.prevent_default();
            }
            let next = !shown.get();
            shown.set(next);
            let _ = panel.set_attribute("data-mode", if next { "shown" } else { "hidden" });
        }) as Box<dyn FnMut(JsValue)>)
    };
    let _ = chip.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref());

    DebugPanel {
        chip,
        panel,
        tree_pre,
        shown,
        closures: vec![cb],
    }
}

/// The mount contract's static wiring, straight from the descriptor — no
/// evaluation, no store read. `state --step--> state`, each output port's
/// `--source-->`, each input port's seed.
pub fn topology_text(desc: &ProgramDescriptor) -> String {
    let mut out = format!(
        "state   {}\n  <- step {}  ({} @ {}Hz)\n",
        desc.state_path, desc.step, desc.tick.mode, desc.tick.rate_hint
    );
    for p in &desc.output_ports {
        match &p.source {
            Some(src) => out.push_str(&format!(
                "{:<8}{}  [{}]\n  <- source {src}\n",
                p.name, p.path, p.shape
            )),
            None => out.push_str(&format!("{:<8}{}  [{}]\n", p.name, p.path, p.shape)),
        }
    }
    for p in &desc.input_ports {
        out.push_str(&format!(
            "{:<8}{}  [{}]  seed {}\n",
            p.name,
            p.path,
            p.shape,
            p.initial.as_deref().unwrap_or("?")
        ));
    }
    out
}

/// A best-effort, truncated CBOR debug string for one entity's raw data —
/// good enough to eyeball a state/port value at a glance; not a real
/// inspector (no per-type pretty-printing, no click-to-expand — `app_host`
/// has no `DomCtx` to wire that with).
fn decode_preview(data: &[u8]) -> String {
    match ciborium::from_reader::<ciborium::Value, _>(data) {
        Ok(v) => {
            let s = format!("{v:?}");
            if s.len() > 220 {
                format!("{}…", &s[..220])
            } else {
                s
            }
        }
        Err(_) => format!("<{} raw bytes, not CBOR>", data.len()),
    }
}

/// A live dump of every entity under the program's namespace: path, entity
/// type, byte size, short content hash — with the DYNAMIC paths (state +
/// every port) decoded inline. Direct-arm only (`app_host` always runs
/// `Peers::new_direct()`), so `tree_listing`/`get_entity` are synchronous,
/// cheap, in-memory reads; call this only while the panel is visible
/// ([`DebugPanel::shown`]) to avoid paying it every tick for nothing.
pub fn tree_dump_text(peers: &Peers, peer_id: &str, ns: &str, desc: &ProgramDescriptor) -> String {
    let mut live: BTreeSet<String> = BTreeSet::new();
    live.insert(host::qualify(ns, &desc.state_path));
    for p in desc.output_ports.iter().chain(desc.input_ports.iter()) {
        live.insert(host::qualify(ns, &p.path));
    }

    let prefix = format!("/{ns}/");
    let mut entries = peers.tree_listing(peer_id, &prefix);
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    let mut out = format!("{} entities under {prefix}\n", entries.len());
    for entry in &entries {
        let Some(entity) = peers.get_entity(peer_id, &entry.path) else {
            out.push_str(&format!("  {}  <unreadable>\n", entry.path));
            continue;
        };
        let hash = entry.hash.to_string();
        let hash = hash.get(..hash.len().min(20)).unwrap_or(&hash);
        if live.contains(&entry.path) {
            out.push_str(&format!(
                "* {}  [{}, {}B, {hash}]\n    = {}\n",
                entry.path,
                entity.entity_type,
                entity.data.len(),
                decode_preview(&entity.data),
            ));
        } else {
            out.push_str(&format!(
                "  {}  [{}, {}B, {hash}]\n",
                entry.path,
                entity.entity_type,
                entity.data.len(),
            ));
        }
    }
    out
}
